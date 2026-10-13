//! Word search. Every photo gets a CLIP description computed from Shoebox's own
//! preview (so photos on unplugged disks are indexed too). Descriptions live in
//! `data\search.db` - a separate file, not the catalog, because they can always
//! be rebuilt and the catalog's backup must stay small. Searching is a dot
//! product over all of them in memory: ~20 ms for 100k photos.

use crate::ai::{self, CLIP_DIM, CLIP_MODEL, ClipText, ClipVisual};
use crate::catalog::Catalog;
use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

pub struct SearchIndex {
    db: Mutex<Connection>,
    /// Photo ids and their vectors, one flat row of CLIP_DIM per id.
    ids: RwLock<Vec<i64>>,
    vecs: RwLock<Vec<f32>>,
    visual: Mutex<Option<ClipVisual>>,
    text: Mutex<Option<ClipText>>,
}

#[derive(Clone, Default, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AiProgress {
    /// "search" for now; "faces" later.
    pub task: String,
    pub done: u64,
    pub total: u64,
    pub gpu: bool,
    pub finished: bool,
    pub error: Option<String>,
}

impl SearchIndex {
    pub fn open(cat: &Catalog) -> Result<Self> {
        let conn = Connection::open(cat.data.root.join("search.db"))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS clip (photo_id INTEGER PRIMARY KEY, model TEXT NOT NULL, vec BLOB NOT NULL);",
        )?;
        let mut ids = Vec::new();
        let mut vecs = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT photo_id, vec FROM clip WHERE model = ?1")?;
            let mut rows = stmt.query([CLIP_MODEL])?;
            while let Some(r) = rows.next()? {
                let blob: Vec<u8> = r.get(1)?;
                if blob.len() == CLIP_DIM * 4 {
                    ids.push(r.get(0)?);
                    vecs.extend(blob.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
                }
            }
        }
        Ok(Self {
            db: Mutex::new(conn),
            ids: RwLock::new(ids),
            vecs: RwLock::new(vecs),
            visual: Mutex::new(None),
            text: Mutex::new(None),
        })
    }

    /// Lets go of CLIP (it is being deleted). Waits for describing in
    /// progress: cancel it first.
    pub fn unload(&self) {
        *self.visual.lock().unwrap() = None;
        *self.text.lock().unwrap() = None;
    }

    pub fn indexed(&self) -> usize {
        self.ids.read().unwrap().len()
    }

    /// Describes every photo that has a preview and no description yet, and
    /// forgets descriptions of photos that left the catalog.
    pub fn index_pending(&self, cat: &Catalog, cancel: &AtomicBool, progress: &dyn Fn(&AiProgress)) -> Result<()> {
        let ready = cat.preview_ready_ids()?;
        let ready_set: HashSet<i64> = ready.iter().copied().collect();
        self.forget_missing(&ready_set)?;
        let have: HashSet<i64> = self.ids.read().unwrap().iter().copied().collect();
        let todo: Vec<i64> = ready.into_iter().filter(|id| !have.contains(id)).collect();
        if todo.is_empty() {
            return Ok(());
        }

        let mut p = AiProgress { task: "search".into(), total: todo.len() as u64, ..Default::default() };
        let mut visual = self.visual.lock().unwrap();
        if visual.is_none() {
            *visual = Some(ClipVisual::load()?);
        }
        let model = visual.as_ref().unwrap();
        p.gpu = model.on_gpu;
        progress(&p);
        log::info!("search: describing {} photos ({})", todo.len(), if p.gpu { "GPU" } else { "CPU" });

        let started = std::time::Instant::now();
        let mut batch: Vec<(i64, Vec<f32>)> = Vec::new();
        for (n, id) in todo.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            match image::open(cat.data.preview_path(*id)) {
                Ok(img) => match model.embed(&img) {
                    Ok(v) if v.len() == CLIP_DIM => batch.push((*id, v)),
                    Ok(v) => log::warn!("search: photo {id}: unexpected vector length {}", v.len()),
                    Err(e) => log::warn!("search: photo {id}: {e:#}"),
                },
                Err(e) => log::warn!("search: photo {id}: preview unreadable: {e}"),
            }
            if batch.len() >= 64 || n + 1 == todo.len() {
                self.store(&mut batch)?;
                p.done = n as u64 + 1;
                progress(&p);
            }
        }
        self.store(&mut batch)?;
        log::info!("search: {} photos described in {:.1} s", p.done, started.elapsed().as_secs_f64());
        Ok(())
    }

    fn store(&self, batch: &mut Vec<(i64, Vec<f32>)>) -> Result<()> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction()?;
        for (id, v) in batch.iter() {
            let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            tx.execute(
                "INSERT OR REPLACE INTO clip (photo_id, model, vec) VALUES (?1, ?2, ?3)",
                params![id, CLIP_MODEL, blob],
            )?;
        }
        tx.commit()?;
        let mut ids = self.ids.write().unwrap();
        let mut vecs = self.vecs.write().unwrap();
        for (id, v) in batch.drain(..) {
            ids.push(id);
            vecs.extend(v);
        }
        Ok(())
    }

    fn forget_missing(&self, keep: &HashSet<i64>) -> Result<()> {
        let gone: Vec<i64> = self.ids.read().unwrap().iter().copied().filter(|id| !keep.contains(id)).collect();
        if gone.is_empty() {
            return Ok(());
        }
        {
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction()?;
            for id in &gone {
                tx.execute("DELETE FROM clip WHERE photo_id = ?1", [id])?;
            }
            tx.commit()?;
        }
        let gone: HashSet<i64> = gone.into_iter().collect();
        let mut ids = self.ids.write().unwrap();
        let mut vecs = self.vecs.write().unwrap();
        let (mut new_ids, mut new_vecs) = (Vec::new(), Vec::new());
        for (i, id) in ids.iter().enumerate() {
            if !gone.contains(id) {
                new_ids.push(*id);
                new_vecs.extend_from_slice(&vecs[i * CLIP_DIM..(i + 1) * CLIP_DIM]);
            }
        }
        *ids = new_ids;
        *vecs = new_vecs;
        Ok(())
    }

    /// Photos matching `query`, best first, with their scores, and the score
    /// above which a match is *clear*. CLIP scores every photo, and on any
    /// catalog most land close together, so a match has to stand out from how
    /// this whole catalog scores against this phrase (mean + 2.5 standard
    /// deviations), and stay near the best one. Weaker but still above-average
    /// matches (mean + 1 sd) come back too, for "show more".
    pub fn search(&self, query: &str, limit: usize) -> Result<(Vec<(i64, f32)>, f32)> {
        let q = {
            let mut text = self.text.lock().unwrap();
            if text.is_none() {
                ai::runtime()?;
                *text = Some(ClipText::load()?);
            }
            text.as_ref().unwrap().embed(query)?
        };
        let ids = self.ids.read().unwrap();
        let vecs = self.vecs.read().unwrap();
        let mut scored: Vec<(i64, f32)> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let v = &vecs[i * CLIP_DIM..(i + 1) * CLIP_DIM];
                (*id, v.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>())
            })
            .collect();
        if scored.is_empty() {
            return Ok((scored, 0.0));
        }
        let n = scored.len() as f32;
        let mean = scored.iter().map(|s| s.1).sum::<f32>() / n;
        let sd = (scored.iter().map(|s| (s.1 - mean).powi(2)).sum::<f32>() / n).sqrt();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        let best = scored[0].1;
        let clear = (mean + 2.5 * sd).max(best - 0.06).min(best);
        let possible = (mean + sd).min(clear);
        scored.retain(|s| s.1 >= possible);
        scored.truncate(limit);
        Ok((scored, clear))
    }
}
