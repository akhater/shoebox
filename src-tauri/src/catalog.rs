//! The catalog: database + settings + which disks are online right now.
//! No UI code here, so the CLI and the app share it.

use crate::db::{self, FILE_MISSING, FILE_PRESENT, PREVIEW_OK, under_sql};
use crate::gear::{self, Kind};
use crate::paths::{self, DataDir};
use crate::settings::Settings;
use crate::volumes::{self, MountedVolume};
use anyhow::{Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock};

pub struct Catalog {
    pub data: DataDir,
    db: Mutex<Connection>,
    /// Idle reading connections (see [`Catalog::read`]).
    readers: Mutex<Vec<Connection>>,
    /// What every connection may use to read the catalog (from the settings).
    memory: RwLock<db::Memory>,
    pub settings: RwLock<Settings>,
    /// volume id -> current mount point, for every known disk that is plugged in.
    online: RwLock<HashMap<i64, String>>,
    /// Catalog folders whose disk is plugged in but which aren't there
    /// (renamed, moved, deleted): (volume id, relative path). Their photos are
    /// treated like an unplugged disk's - unreachable, not missing.
    unreachable: RwLock<Vec<(i64, String)>>,
    /// Notices every commit to the catalog, by Shoebox or another program
    /// (shoebox-cli): a connection that never writes, its `data_version` last
    /// seen, and how many times it changed. See [`Catalog::generation`].
    watch: Mutex<(Connection, i64, u64)>,
    /// How many times the disks plugged in, or the catalog folders found, changed.
    disks_changed: AtomicU64,
    /// Answers kept until the catalog changes: asked again, they come back at once.
    kept_overview: Kept<Overview>,
    kept_browse: Kept<Browse>,
    kept_lists: Kept<PhotoList>,
    kept_facets: Kept<Facets>,
}

impl Catalog {
    pub fn open(data: DataDir) -> Result<Self> {
        data.ensure()?;
        let settings = Settings::load(&data.settings_path());
        let memory = memory_for(&settings, &data);
        let conn = db::open(&data.db_path(), memory)?;
        gear::tidy_if_new_rules(&conn)?;
        let watch = db::open_reader(&data.db_path(), db::Memory { cache_mb: 1, map_mb: 0 })?;
        let version: i64 = watch.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        let cat = Self {
            data,
            db: Mutex::new(conn),
            readers: Mutex::new(Vec::new()),
            memory: RwLock::new(memory),
            settings: RwLock::new(settings),
            online: RwLock::new(HashMap::new()),
            unreachable: RwLock::new(Vec::new()),
            watch: Mutex::new((watch, version, 0)),
            disks_changed: AtomicU64::new(0),
            kept_overview: Kept::new(1),
            kept_browse: Kept::new(1),
            // A list of 190,000 photos is about 20 MB: the last three views.
            kept_lists: Kept::new(3),
            kept_facets: Kept::new(8),
        };
        cat.refresh_volumes()?;
        cat.refresh_roots()?;
        Ok(cat)
    }

    /// Checks that every catalog folder on a plugged-in disk is still there.
    /// Called at start, when disks come and go, and around scans - not on a
    /// timer, so idle USB disks are left asleep. Returns true if that changed.
    pub fn refresh_roots(&self) -> Result<bool> {
        let roots: Vec<(i64, String)> = self
            .db()
            .prepare("SELECT volume_id, rel_path FROM roots")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let online = self.online.read().unwrap().clone();
        let gone: Vec<(i64, String)> = roots
            .into_iter()
            .filter(|(vid, rel)| online.get(vid).is_some_and(|m| !Path::new(&volumes::join(m, rel)).is_dir()))
            .collect();
        let mut cur = self.unreachable.write().unwrap();
        let changed = *cur != gone;
        *cur = gone;
        if changed {
            self.disks_changed.fetch_add(1, Ordering::SeqCst);
        }
        Ok(changed)
    }

    fn is_unreachable(&self, volume_id: i64, rel: &str) -> bool {
        self.unreachable.read().unwrap().iter().any(|(v, r)| *v == volume_id && db::is_under(rel, r))
    }

    pub fn db(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A connection for queries that only read. Unlike [`Catalog::db`] it is
    /// never held by a scan, the AI or another query: what the window shows
    /// comes back while they work. It sees what the writer has committed.
    pub fn read(&self) -> Result<Reader<'_>> {
        let idle = self.readers.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let conn = match idle {
            Some(c) => c,
            None => db::open_reader(&self.data.db_path(), self.memory())?,
        };
        Ok(Reader { cat: self, conn: Some(conn) })
    }

    /// Changes whenever the catalog does: anything committed to it, by Shoebox
    /// or another program, or a disk or catalog folder coming or going. An
    /// answer worked out at one generation is good for as long as it lasts.
    pub fn generation(&self) -> Result<Generation> {
        let mut watch = self.watch.lock().unwrap_or_else(|e| e.into_inner());
        let version: i64 = watch.0.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        if version != watch.1 {
            watch.1 = version;
            watch.2 += 1;
        }
        Ok((watch.2, self.disks_changed.load(Ordering::SeqCst)))
    }

    /// Forgets the answers kept, so the next ones are worked out again (to time them).
    pub fn forget_kept(&self) {
        self.kept_overview.clear();
        self.kept_browse.clear();
        self.kept_lists.clear();
        self.kept_facets.clear();
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn save_settings(&self, s: Settings) -> Result<Settings> {
        let s = s.sanitized();
        s.save(&self.data)?;
        *self.settings.write().unwrap() = s.clone();
        self.set_memory(memory_for(&s, &self.data))?;
        Ok(s)
    }

    /// What each connection may use to read the catalog now.
    pub fn memory(&self) -> db::Memory {
        *self.memory.read().unwrap()
    }

    /// Uses this much memory from now on: the writer at once, readers as they
    /// are opened again (the idle ones are closed).
    pub fn set_memory(&self, memory: db::Memory) -> Result<()> {
        memory.apply(&self.db())?;
        *self.memory.write().unwrap() = memory;
        self.readers.lock().unwrap_or_else(|e| e.into_inner()).clear();
        Ok(())
    }

    pub fn mount_of(&self, volume_id: i64) -> Option<String> {
        self.online.read().unwrap().get(&volume_id).cloned()
    }

    pub fn online_ids(&self) -> Vec<i64> {
        self.online.read().unwrap().keys().copied().collect()
    }

    /// Re-reads which known disks are plugged in. Returns true if that changed.
    pub fn refresh_volumes(&self) -> Result<bool> {
        let mounted = volumes::mounted();
        // Test hook: `SHOEBOX_TEST_UNPLUG=1,3` treats those volume ids as unplugged.
        let unplugged: Vec<i64> = std::env::var("SHOEBOX_TEST_UNPLUG")
            .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
            .unwrap_or_default();
        let mut now_online = HashMap::new();
        {
            let db = self.db();
            for mv in &mounted {
                if let Some(id) = find_volume(&db, mv)?
                    && !unplugged.contains(&id)
                {
                    // A disk mounted twice (letter and folder) keeps its first place.
                    if now_online.contains_key(&id) {
                        continue;
                    }
                    db.execute(
                        "UPDATE volumes SET last_mount = ?1, last_seen = ?2 WHERE id = ?3",
                        params![mv.mount, db::now(), id],
                    )?;
                    now_online.insert(id, mv.mount.clone());
                }
            }
        }
        let mut online = self.online.write().unwrap();
        let changed = *online != now_online;
        *online = now_online;
        if changed {
            self.disks_changed.fetch_add(1, Ordering::SeqCst);
        }
        Ok(changed)
    }

    /// Tests only: `folder` stands for a disk of its own called `name`,
    /// plugged in until the catalog next looks at which disks are (as
    /// [`Catalog::open`] does). Returns its disk id, the same for the same name.
    #[doc(hidden)]
    pub fn plug_in_test_disk(&self, name: &str, folder: &Path) -> Result<i64> {
        let serial = i64::from(u32::from_le_bytes(blake3::hash(name.as_bytes()).as_bytes()[..4].try_into()?));
        let id = {
            let db = self.db();
            db.execute(
                "INSERT OR IGNORE INTO volumes (serial, fs, size_bytes, label) VALUES (?1, 'test', 0, ?2)",
                params![serial, name],
            )?;
            db.query_row(
                "SELECT id FROM volumes WHERE serial = ?1 AND fs = 'test' AND size_bytes = 0",
                [serial],
                |r| r.get(0),
            )?
        };
        self.online.write().unwrap().insert(id, folder.to_string_lossy().into_owned());
        self.disks_changed.fetch_add(1, Ordering::SeqCst);
        self.refresh_roots()?;
        Ok(id)
    }

    /// Adds a folder to the catalog and returns its root id.
    /// A folder inside an existing root is refused; roots inside the new folder
    /// are absorbed into it.
    pub fn add_root(&self, path: &Path) -> Result<i64> {
        if !path.is_dir() {
            bail!("{} is not a folder", path.display());
        }
        let (mv, rel) = volumes::locate(path)?;
        let db = self.db();
        let vid = upsert_volume(&db, &mv)?;
        let existing: Vec<(i64, String)> = db
            .prepare("SELECT id, rel_path FROM roots WHERE volume_id = ?1")?
            .query_map([vid], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, r) in &existing {
            if db::is_under(&rel, r) {
                if *r == rel {
                    return Ok(*id);
                }
                bail!("This folder is already in the catalog, inside {}", volumes::join(&mv.mount, r));
            }
        }
        let tx = db.unchecked_transaction()?;
        for (id, r) in &existing {
            if db::is_under(r, &rel) {
                tx.execute("DELETE FROM roots WHERE id = ?1", [id])?;
            }
        }
        tx.execute(
            "INSERT INTO roots (volume_id, rel_path, added_at) VALUES (?1, ?2, ?3)",
            params![vid, rel, db::now()],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.online.write().unwrap().insert(vid, mv.mount.clone());
        self.disks_changed.fetch_add(1, Ordering::SeqCst);
        Ok(id)
    }

    /// Takes a folder out of the catalog. **The photo files are not touched.**
    /// Photos that live only in that folder leave the catalog with their
    /// previews; photos with a copy elsewhere stay.
    pub fn remove_root(&self, root_id: i64) -> Result<RemoveReport> {
        let root = self.root(root_id)?;
        let mut db = self.db();
        let tx = db.transaction()?;
        let files: Vec<(i64, Option<i64>)> = tx
            .prepare(&format!(
                "SELECT f.id, f.photo_id FROM files f JOIN folders fo ON fo.id = f.folder_id
                  WHERE fo.volume_id = ?1 AND {}",
                under_sql("fo", 2)
            ))?
            .query_map(params![root.volume_id, root.rel_path], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (fid, _) in &files {
            tx.execute("DELETE FROM files WHERE id = ?1", [fid])?;
        }
        let mut gone = Vec::new();
        for pid in files.iter().filter_map(|(_, p)| *p) {
            let still_used: bool =
                tx.query_row("SELECT EXISTS (SELECT 1 FROM files WHERE photo_id = ?1)", [pid], |r| r.get(0))?;
            if !still_used && !gone.contains(&pid) {
                tx.execute("DELETE FROM faces WHERE photo_id = ?1", [pid])?;
                tx.execute("DELETE FROM face_scan WHERE photo_id = ?1", [pid])?;
                tx.execute("DELETE FROM photos WHERE id = ?1", [pid])?;
                gone.push(pid);
            }
        }
        tx.execute(
            &format!(
                "DELETE FROM folders WHERE volume_id = ?1 AND {} AND NOT EXISTS (SELECT 1 FROM files f WHERE f.folder_id = folders.id)",
                under_sql("folders", 2)
            ),
            params![root.volume_id, root.rel_path],
        )?;
        tx.execute("DELETE FROM roots WHERE id = ?1", [root_id])?;
        // People left without a single face.
        tx.execute("DELETE FROM people WHERE NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = people.id)", [])?;
        tx.commit()?;
        drop(db);
        for pid in &gone {
            let _ = self.data.remove(&self.data.preview_path(*pid));
            let _ = self.data.remove(&self.data.thumb_path(*pid));
        }
        let _ = self.refresh_roots();
        Ok(RemoveReport { files: files.len() as i64, photos: gone.len() as i64 })
    }

    pub fn stats(&self) -> Result<Stats> {
        let (photos, raw, no_preview, files, missing_files, roots, disks): (i64, i64, i64, i64, i64, i64, i64) =
            self.db().query_row(
                &format!(
                    "SELECT (SELECT COUNT(*) FROM photos), (SELECT COUNT(*) FROM photos WHERE kind = 'raw'),
                            (SELECT COUNT(*) FROM photos WHERE preview_state <> {PREVIEW_OK}),
                            (SELECT COUNT(*) FROM files), (SELECT COUNT(*) FROM files WHERE state = {FILE_MISSING}),
                            (SELECT COUNT(*) FROM roots), (SELECT COUNT(DISTINCT volume_id) FROM roots)"
                ),
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )?;
        let db_bytes = ["catalog.db", "catalog.db-wal"]
            .iter()
            .filter_map(|f| self.data.root.join(f).metadata().ok())
            .map(|m| m.len())
            .sum();
        let (_, preview_bytes) = self.data.usage("previews");
        let (_, thumb_bytes) = self.data.usage("thumbs");
        Ok(Stats {
            photos,
            raw,
            other: photos - raw,
            no_preview,
            files,
            missing_files,
            folders: roots,
            disks,
            db_bytes,
            preview_bytes,
            thumb_bytes,
            total_bytes: db_bytes + preview_bytes + thumb_bytes,
            memory: self.memory(),
        })
    }

    pub fn root(&self, root_id: i64) -> Result<RootRow> {
        self.db()
            .query_row(
                "SELECT r.id, r.volume_id, r.rel_path, v.label FROM roots r JOIN volumes v ON v.id = r.volume_id WHERE r.id = ?1",
                [root_id],
                |r| Ok(RootRow { id: r.get(0)?, volume_id: r.get(1)?, rel_path: r.get(2)?, volume_label: r.get(3)? }),
            )
            .optional()?
            .ok_or_else(|| anyhow!("Unknown folder {root_id}"))
    }

    pub fn root_ids(&self) -> Result<Vec<i64>> {
        let db = self.db();
        let ids = db
            .prepare("SELECT id FROM roots ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    pub fn rename_volume(&self, volume_id: i64, label: &str) -> Result<()> {
        let label = label.trim();
        if label.is_empty() {
            bail!("A disk name can't be empty");
        }
        self.db().execute("UPDATE volumes SET label = ?1 WHERE id = ?2", params![label, volume_id])?;
        Ok(())
    }

    /// SQL condition on folder alias `fo`: its disk is plugged in and its
    /// catalog folder is where it should be.
    fn online_sql(&self) -> String {
        let ids = self.online_ids();
        if ids.is_empty() {
            return "0".into();
        }
        let mut cond = format!("fo.volume_id IN ({})", ids.iter().map(i64::to_string).collect::<Vec<_>>().join(","));
        for (vid, rel) in self.unreachable.read().unwrap().iter() {
            let r = rel.replace('\'', "''");
            cond.push_str(&format!(
                " AND NOT (fo.volume_id = {vid} AND ('{r}' = '' OR fo.rel_path = '{r}' OR substr(fo.rel_path, 1, length('{r}') + 1) = '{r}' || '\\'))"
            ));
        }
        cond
    }

    pub fn overview(&self) -> Result<Overview> {
        self.kept_overview.get(self, String::new(), || self.overview_now())
    }

    fn overview_now(&self) -> Result<Overview> {
        let online = self.online.read().unwrap().clone();
        let state = state_sql(&self.online_sql());
        let db = self.read()?;

        let mut volumes: Vec<VolumeInfo> = db
            .prepare(
                "SELECT v.id, v.label, v.last_mount,
                        (SELECT COUNT(DISTINCT f.photo_id) FROM files f JOIN folders fo ON fo.id = f.folder_id
                          WHERE fo.volume_id = v.id AND f.photo_id IS NOT NULL)
                   FROM volumes v
                  WHERE EXISTS (SELECT 1 FROM roots r WHERE r.volume_id = v.id)
                     OR EXISTS (SELECT 1 FROM folders fo WHERE fo.volume_id = v.id)
                  ORDER BY v.label COLLATE NOCASE",
            )?
            .query_map([], |r| {
                let id: i64 = r.get(0)?;
                Ok(VolumeInfo {
                    id,
                    label: r.get(1)?,
                    last_mount: r.get(2)?,
                    mount: online.get(&id).cloned(),
                    online: online.contains_key(&id),
                    photos: r.get(3)?,
                    roots: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;

        let mut root_stmt = db.prepare(&format!(
            "SELECT r.id, r.rel_path, r.last_scan,
                    (SELECT COUNT(DISTINCT f.photo_id) FROM files f JOIN folders fo ON fo.id = f.folder_id
                      WHERE fo.volume_id = r.volume_id AND f.photo_id IS NOT NULL AND {})
               FROM roots r WHERE r.volume_id = ?1 ORDER BY r.rel_path COLLATE NOCASE",
            under_sql("fo", 2).replace("?2", "r.rel_path")
        ))?;
        for v in &mut volumes {
            let shown_mount = v.mount.clone().or_else(|| v.last_mount.clone()).unwrap_or_else(|| "?:\\".into());
            v.roots = root_stmt
                .query_map([v.id], |r| {
                    let rel: String = r.get(1)?;
                    Ok(RootInfo {
                        id: r.get(0)?,
                        found: !self.is_unreachable(v.id, &rel),
                        name: rel.rsplit('\\').next().filter(|s| !s.is_empty()).unwrap_or("Whole disk").to_string(),
                        path: volumes::join(&shown_mount, &rel),
                        rel_path: rel,
                        last_scan: r.get(2)?,
                        photos: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
        }

        // Materialized: each photo's state is looked up once, not once per sum.
        let (total, on, off, missing, marked): (i64, i64, i64, i64, i64) = db.query_row(
            &format!(
                "WITH st AS MATERIALIZED (SELECT {state} AS s, p.marked FROM photos p)
                 SELECT COUNT(*), COALESCE(SUM(s = 2), 0), COALESCE(SUM(s = 1), 0),
                        COALESCE(SUM(s = 0), 0), COALESCE(SUM(marked), 0)
                   FROM st"
            ),
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;

        Ok(Overview {
            volumes,
            total,
            online: on,
            offline: off,
            missing,
            marked,
            data_dir: self.data.root.to_string_lossy().into_owned(),
        })
    }

    /// Photos in a scope narrowed by `filters`, in `sort` order, as parallel
    /// arrays (compact for 100k+).
    pub fn list_photos(&self, scope: &Scope, sort: Sort, filters: &Filters) -> Result<PhotoList> {
        let asked = serde_json::to_string(&(scope, sort, filters))?;
        self.kept_lists.get(self, asked, || self.list_among(scope, sort, filters, None))
    }

    /// [`Catalog::list_photos`], only among the photos `only` (when given).
    fn list_among(&self, scope: &Scope, sort: Sort, filters: &Filters, only: Option<&[i64]>) -> Result<PhotoList> {
        let db = self.read()?;
        let state = state_sql(&self.online_sql());
        let (filter, p1, p2) = self.scope_sql(&db, scope, &state)?;
        // Same-second bursts follow the camera's 1/100 s (in the capture key),
        // then the file name; photos without a date go last.
        let name = "(SELECT MIN(f.name) FROM files f WHERE f.photo_id = p.id) COLLATE NOCASE";
        let order = match sort {
            Sort::Oldest => format!("COALESCE(p.taken_ts, 0) <= 0, p.taken_ts, p.capture_key, {name}, p.id"),
            Sort::Newest => {
                format!("COALESCE(p.taken_ts, 0) <= 0, p.taken_ts DESC, p.capture_key DESC, {name} DESC, p.id DESC")
            }
            Sort::Name => format!("{name}, p.id"),
            Sort::Added => "p.added_at DESC, p.id DESC".to_string(),
        };
        let fresh = match scope {
            Scope::Person { .. } => {
                "EXISTS (SELECT 1 FROM faces fx WHERE fx.photo_id = p.id AND fx.person_id = ?1 AND fx.seen = 0)"
            }
            _ => "0",
        };
        let sql = format!(
            "SELECT p.id, COALESCE(p.taken_ts, 0), {state},
                    COALESCE(p.thumb_w, 0), COALESCE(p.thumb_h, 0), p.preview_rev, p.preview_state, p.marked,
                    COALESCE((SELECT {EXT_SQL} FROM files f WHERE f.photo_id = p.id ORDER BY f.state LIMIT 1), ''),
                    p.kind = 'raw', {fresh}
               FROM photos p
              WHERE {filter} AND (?1 = ?1 AND ?2 = ?2) AND {}
                AND (?10 IS NULL OR p.id IN (SELECT value FROM json_each(?10)))
              ORDER BY {order}",
            filters_sql(filters, &state)
        );
        let only = only.map(serde_json::to_string).transpose()?;
        let mut out = PhotoList::default();
        let mut stmt = db.prepare(&sql)?;
        let [d, c, l, t, s, pe, tg] = filters.params(None);
        let mut rows = stmt.query(params![p1, p2, d, c, l, t, s, pe, tg, only])?;
        while let Some(r) = rows.next()? {
            out.ids.push(r.get(0)?);
            out.taken.push(r.get(1)?);
            out.state.push(r.get(2)?);
            out.w.push(r.get(3)?);
            out.h.push(r.get(4)?);
            out.rev.push(r.get(5)?);
            out.preview.push(r.get(6)?);
            out.marked.push(r.get(7)?);
            out.ext.push(r.get(8)?);
            out.raw.push(r.get(9)?);
            out.fresh.push(r.get(10)?);
        }
        Ok(out)
    }

    /// Photos taken each day (camera local time) in a scope narrowed by
    /// `filters`, oldest day first: the window's calendar, for agents.
    pub fn days(&self, scope: &Scope, filters: &Filters) -> Result<Days> {
        let db = self.read()?;
        let state = state_sql(&self.online_sql());
        let (filter, p1, p2) = self.scope_sql(&db, scope, &state)?;
        let sql = format!(
            "SELECT CASE WHEN COALESCE(p.taken_ts, 0) > 0
                    THEN strftime('%Y-%m-%d', p.taken_ts, 'unixepoch') END AS d, COUNT(*)
               FROM photos p
              WHERE {filter} AND (?1 = ?1 AND ?2 = ?2) AND {}
              GROUP BY d ORDER BY d",
            filters_sql(filters, &state)
        );
        let mut stmt = db.prepare(&sql)?;
        let [d, c, l, t, s, pe, tg] = filters.params(None);
        let mut rows = stmt.query(params![p1, p2, d, c, l, t, s, pe, tg])?;
        let mut out = Days::default();
        while let Some(r) = rows.next()? {
            match r.get::<_, Option<String>>(0)? {
                Some(day) => out.days.push(Day { day, photos: r.get(1)? }),
                None => out.undated = r.get(1)?,
            }
        }
        Ok(out)
    }

    /// What each filter can offer in a scope, with counts. Each filter is
    /// counted with every *other* filter applied, so ticking a second camera
    /// stays possible after ticking the first. People or tags asking for all
    /// of them count with their own ticks too: only who is with them is left.
    pub fn facets(&self, scope: &Scope, filters: &Filters) -> Result<Facets> {
        let asked = serde_json::to_string(&(scope, filters))?;
        self.kept_facets.get(self, asked, || self.facets_now(scope, filters))
    }

    fn facets_now(&self, scope: &Scope, filters: &Filters) -> Result<Facets> {
        let db = self.read()?;
        let state = state_sql(&self.online_sql());
        let (filter, p1, p2) = self.scope_sql(&db, scope, &state)?;
        let where_ = format!("{filter} AND (?1 = ?1 AND ?2 = ?2) AND {}", filters_sql(filters, &state));
        let count = |value: &str, from: &str, order: &str, own: Facet| -> Result<Vec<(String, i64)>> {
            let sql = format!(
                "SELECT {value} AS v, COUNT(DISTINCT p.id) FROM {from} WHERE {where_} GROUP BY v ORDER BY {order}"
            );
            let [d, c, l, t, s, pe, tg] = filters.params(Some(own));
            let rows = db
                .prepare(&sql)?
                .query_map(params![p1, p2, d, c, l, t, s, pe, tg], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        };
        let people = {
            let sql = format!(
                "SELECT pe.id, pe.name,
                        (SELECT f2.id FROM faces f2 WHERE f2.person_id = pe.id ORDER BY f2.score DESC LIMIT 1),
                        COUNT(DISTINCT p.id)
                   FROM photos p JOIN faces fa ON fa.photo_id = p.id JOIN people pe ON pe.id = fa.person_id
                  WHERE {where_} AND pe.ignored = 0
                    AND (pe.name IS NOT NULL OR (SELECT COUNT(DISTINCT f3.photo_id) FROM faces f3 WHERE f3.person_id = pe.id) >= 2)
                  GROUP BY pe.id ORDER BY pe.name IS NULL, COUNT(DISTINCT p.id) DESC, pe.name"
            );
            let [d, c, l, t, s, pe, tg] = filters.params(Some(Facet::People));
            db.prepare(&sql)?
                .query_map(params![p1, p2, d, c, l, t, s, pe, tg], |r| {
                    Ok(PersonValue { id: r.get(0)?, name: r.get(1)?, cover: r.get(2)?, count: r.get(3)? })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        // With "all of them", counts include the tags already ticked; with "any", they don't.
        let tags = {
            let sql = format!(
                "SELECT tg.id, tg.name, COUNT(DISTINCT p.id)
                   FROM photos p JOIN photo_tags ptg ON ptg.photo_id = p.id JOIN tags tg ON tg.id = ptg.tag_id
                  WHERE {where_}
                  GROUP BY tg.id ORDER BY COUNT(DISTINCT p.id) DESC, tg.name COLLATE NOCASE"
            );
            let [d, c, l, t, s, pe, tg] = filters.params(Some(Facet::Tags));
            db.prepare(&sql)?
                .query_map(params![p1, p2, d, c, l, t, s, pe, tg], |r| {
                    Ok(crate::tags::Tag { id: r.get(0)?, name: r.get(1)?, count: r.get(2)? })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let value = |(value, count): (String, i64)| FacetValue { value, count };
        let date = "CASE WHEN COALESCE(p.taken_ts, 0) > 0 THEN strftime('%Y-%m-%d', p.taken_ts, 'unixepoch') ELSE 'unknown' END";
        Ok(Facets {
            dates: count(date, "photos p", "v", Facet::Dates)?.into_iter().map(value).collect(),
            cameras: count(Kind::Camera.stored_sql(), "photos p", "COUNT(*) DESC, v", Facet::Cameras)?
                .into_iter()
                .map(value)
                .collect(),
            lenses: count(Kind::Lens.stored_sql(), "photos p", "COUNT(*) DESC, v", Facet::Lenses)?
                .into_iter()
                .map(value)
                .collect(),
            types: count(
                EXT_SQL,
                "photos p JOIN files f ON f.photo_id = p.id",
                "COUNT(DISTINCT p.id) DESC, v",
                Facet::Types,
            )?
            .into_iter()
            .map(|(value, count)| TypeValue {
                raw: crate::formats::kind_of(&format!("x.{value}")) == Some(crate::formats::Kind::Raw),
                value,
                count,
            })
            .collect(),
            status: count(&status_sql(&state), "photos p", "v DESC", Facet::Status)?.into_iter().map(value).collect(),
            people,
            tags,
        })
    }

    /// The SQL condition (on photo alias `p` whose state is `state`) for a
    /// scope, and its two parameters.
    fn scope_sql(&self, db: &Connection, scope: &Scope, state: &str) -> Result<(String, i64, String)> {
        let in_folder = format!(
            "p.id IN (SELECT f.photo_id FROM files f JOIN folders fo ON fo.id = f.folder_id WHERE fo.volume_id = ?1 AND {})",
            under_sql("fo", 2)
        );
        let (filter, p1, p2): (String, i64, String) = match scope {
            Scope::All => ("1".into(), 0, String::new()),
            Scope::Missing => (format!("{state} = 0"), 0, String::new()),
            Scope::Volume { id } => (
                "p.id IN (SELECT f.photo_id FROM files f JOIN folders fo ON fo.id = f.folder_id WHERE fo.volume_id = ?1)".into(),
                *id,
                String::new(),
            ),
            Scope::Root { id } => {
                let (volume_id, rel_path) = db
                    .query_row("SELECT volume_id, rel_path FROM roots WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?
                    .ok_or_else(|| anyhow!("Unknown folder {id}"))?;
                (in_folder, volume_id, rel_path)
            }
            Scope::Folder { volume_id, rel_path } => (in_folder, *volume_id, rel_path.clone()),
            Scope::Date { key } if key == "unknown" => ("COALESCE(p.taken_ts, 0) <= 0".into(), 0, String::new()),
            // The year or month as a range of times, so the index on them finds the photos.
            Scope::Date { key } => (
                "p.taken_ts > 0
                 AND p.taken_ts >= unixepoch(?2 || CASE WHEN length(?2) = 4 THEN '-01-01' ELSE '-01' END)
                 AND p.taken_ts < unixepoch(?2 || CASE WHEN length(?2) = 4 THEN '-01-01' ELSE '-01' END,
                                            CASE WHEN length(?2) = 4 THEN '+1 year' ELSE '+1 month' END)"
                    .into(),
                0,
                key.clone(),
            ),
            Scope::Camera { name } => (format!("{} = ?2", Kind::Camera.stored_sql()), 0, name.clone()),
            Scope::Lens { name } => (format!("{} = ?2", Kind::Lens.stored_sql()), 0, name.clone()),
            Scope::Person { id, new_only } => (
                format!(
                    "p.id IN (SELECT photo_id FROM faces WHERE person_id = ?1{})",
                    if *new_only { " AND seen = 0" } else { "" }
                ),
                *id,
                String::new(),
            ),
            Scope::Marked => ("p.marked = 1".into(), 0, String::new()),
        };
        Ok((filter, p1, p2))
    }

    /// Photos whose preview is ready (what the AI reads), with their fingerprints.
    pub fn preview_ready(&self) -> Result<Vec<(i64, String)>> {
        let db = self.db();
        let ids = db
            .prepare(&format!(
                "SELECT id, fingerprint FROM photos WHERE preview_state = {PREVIEW_OK} ORDER BY taken_ts DESC"
            ))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    }

    /// The photos of `scope` among `ranked`, in `ranked`'s order (search results).
    pub fn ranked_in_scope(&self, ranked: &[i64], scope: &Scope, filters: &Filters) -> Result<PhotoList> {
        let all = self.list_among(scope, Sort::default(), filters, Some(ranked))?;
        let pos: HashMap<i64, usize> = all.ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let mut out = PhotoList::default();
        for id in ranked {
            if let Some(&i) = pos.get(id) {
                out.ids.push(all.ids[i]);
                out.taken.push(all.taken[i]);
                out.state.push(all.state[i]);
                out.w.push(all.w[i]);
                out.h.push(all.h[i]);
                out.rev.push(all.rev[i]);
                out.preview.push(all.preview[i]);
                out.marked.push(all.marked[i]);
                out.ext.push(all.ext[i].clone());
                out.raw.push(all.raw[i]);
                out.fresh.push(all.fresh[i]);
            }
        }
        Ok(out)
    }

    /// Ways to browse: dates, cameras and lenses, and the folder tree under each root.
    pub fn browse(&self) -> Result<Browse> {
        self.kept_browse.get(self, String::new(), || self.browse_now())
    }

    fn browse_now(&self) -> Result<Browse> {
        let db = self.read()?;

        let mut years: Vec<YearNode> = Vec::new();
        let mut unknown_date = 0;
        let mut stmt = db.prepare(
            "SELECT CASE WHEN COALESCE(taken_ts, 0) > 0 THEN strftime('%Y-%m', taken_ts, 'unixepoch') END AS ym, COUNT(*)
               FROM photos GROUP BY ym ORDER BY ym",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let ym: Option<String> = r.get(0)?;
            let n: i64 = r.get(1)?;
            let Some(ym) = ym else {
                unknown_date = n;
                continue;
            };
            let (y, m) = ym.split_once('-').unwrap_or((&ym, "01"));
            if years.last().is_none_or(|l| l.year != y) {
                years.push(YearNode { year: y.to_string(), count: 0, months: Vec::new() });
            }
            let year = years.last_mut().unwrap();
            year.count += n;
            year.months.push(MonthNode { key: ym.clone(), month: m.parse().unwrap_or(1), count: n });
        }
        drop(rows);
        drop(stmt);

        let gear = gear::list(&db)?;

        // Photos per folder (directly in it), then rolled up into a tree per
        // root. Empty folders (just made, or emptied) are listed too.
        let direct: Vec<(i64, String, i64)> = db
            .prepare(
                "SELECT fo.volume_id, fo.rel_path, COUNT(DISTINCT f.photo_id) FROM folders fo
                   LEFT JOIN files f ON f.folder_id = fo.id GROUP BY fo.id",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let roots: Vec<(i64, i64, String)> = db
            .prepare("SELECT id, volume_id, rel_path FROM roots")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut folders = Vec::new();
        for (root_id, vid, root_rel) in roots {
            let mut top = FolderNode::new(String::new(), root_rel.clone(), vid);
            for (v, rel, n) in direct.iter().filter(|(v, rel, _)| *v == vid && db::is_under(rel, &root_rel)) {
                let rest = rel.get(root_rel.len()..).unwrap_or("").trim_start_matches(SEP);
                top.insert(rest, rel, *v, *n);
            }
            top.sort();
            folders.push(RootTree { root_id, children: top.children });
        }

        Ok(Browse { years, unknown_date, gear, folders })
    }

    pub fn photo_detail(&self, id: i64) -> Result<PhotoDetail> {
        let online = self.online.read().unwrap().clone();
        let db = self.read()?;
        let mut d = db
            .query_row(
                "SELECT id, kind, taken_ts, taken_exif, make, model, lens, width, height, iso, fnumber,
                        exposure, focal, preview_state, preview_rev, error, p.camera_shown, p.lens_shown
                   FROM photos p WHERE id = ?1",
                [id],
                |r| {
                    Ok(PhotoDetail {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        taken_ts: r.get(2)?,
                        taken_from_camera: r.get::<_, i64>(3)? != 0,
                        make: r.get(4)?,
                        model: r.get(5)?,
                        lens_in_file: r.get(6)?,
                        width: r.get(7)?,
                        height: r.get(8)?,
                        iso: r.get(9)?,
                        fnumber: r.get(10)?,
                        exposure: r.get(11)?,
                        focal: r.get(12)?,
                        preview_state: r.get(13)?,
                        preview_rev: r.get(14)?,
                        error: r.get(15)?,
                        camera: r.get::<_, Option<String>>(16)?.filter(|c| c != Kind::Camera.unknown()),
                        lens: r.get::<_, Option<String>>(17)?.filter(|l| l != Kind::Lens.unknown()),
                        files: Vec::new(),
                    })
                },
            )
            .optional()?
            .ok_or_else(|| anyhow!("Unknown photo {id}"))?;
        d.files = db
            .prepare(
                "SELECT f.id, f.name, f.size, f.state, f.error, fo.rel_path, v.id, v.label, v.last_mount
                   FROM files f JOIN folders fo ON fo.id = f.folder_id JOIN volumes v ON v.id = fo.volume_id
                  WHERE f.photo_id = ?1 ORDER BY f.state, v.label",
            )?
            .query_map([id], |r| {
                let name: String = r.get(1)?;
                let rel: String = r.get(5)?;
                let vid: i64 = r.get(6)?;
                let last_mount: Option<String> = r.get(8)?;
                let mount = online.get(&vid).cloned();
                let shown_mount = mount.clone().or(last_mount).unwrap_or_else(|| "?:\\".into());
                let path = format!("{}\\{}", volumes::join(&shown_mount, &rel).trim_end_matches('\\'), name);
                let state: i64 = r.get(3)?;
                Ok(FileLoc {
                    id: r.get(0)?,
                    name,
                    size: r.get(2)?,
                    state: if state == FILE_MISSING {
                        "missing"
                    } else if mount.is_none() {
                        "offline"
                    } else if self.is_unreachable(vid, &rel) {
                        "unreachable"
                    } else {
                        "online"
                    }
                    .into(),
                    error: r.get(4)?,
                    volume_id: vid,
                    volume_label: r.get(7)?,
                    path,
                    folder: rel,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(d)
    }

    /// Can the catalog live in `path`? If `path` is a non-empty folder without
    /// a catalog, the catalog goes into a "Shoebox catalog" folder inside it.
    pub fn check_catalog_folder(&self, path: &Path) -> Result<FolderCheck> {
        let chosen = std::path::absolute(path)?;
        let mut target = chosen.clone();
        let is_empty = |p: &Path| std::fs::read_dir(p).map(|mut d| d.next().is_none()).unwrap_or(true);
        if chosen.is_dir() && !(DataDir { root: chosen.clone() }).has_catalog() && !is_empty(&chosen) {
            target = chosen.join("Shoebox catalog");
        }
        let t = DataDir { root: target.clone() };
        let current = &self.data.root;
        let same = |a: &Path, b: &Path| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
        let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());

        // files() leaves out the databases (snapshotted, not copied): count them here.
        let dbs = paths::SIDE_DATABASES.map(|n| self.data.root.join(n));
        let bytes: u64 = self
            .data
            .files()
            .into_iter()
            .chain(std::iter::once(self.data.db_path()))
            .chain(dbs)
            .filter_map(|p| p.metadata().ok())
            .map(|m| m.len())
            .sum();
        let mut problem = None;
        if !chosen.is_dir() {
            problem = Some("That isn't a folder.".to_string());
        } else if same(&target, current) {
            problem = Some("That's where the catalog already is.".to_string());
        } else if lower(&target).starts_with(lower(current)) || lower(current).starts_with(lower(&target)) {
            problem = Some("Choose a folder that is neither inside the current catalog nor contains it.".to_string());
        } else if !t.has_catalog() && target.is_dir() && !is_empty(&target) {
            problem = Some(format!("{} already exists and isn't empty.", target.display()));
        } else if !t.has_catalog() && volumes::free_bytes(&chosen).is_some_and(|f| f < bytes + bytes / 10 + (64 << 20))
        {
            problem = Some("There isn't enough free space on that disk.".to_string());
        }
        if problem.is_none() {
            for id in self.root_ids()? {
                let root = self.root(id)?;
                if let Some(mount) = self.mount_of(root.volume_id) {
                    let photos = PathBuf::from(volumes::join(&mount, &root.rel_path));
                    if lower(&target).starts_with(lower(&photos)) {
                        problem = Some(format!(
                            "That's inside {}, one of your photo folders. Choose a folder outside them, so Shoebox never catalogs its own previews.",
                            photos.display()
                        ));
                        break;
                    }
                }
            }
        }
        Ok(FolderCheck { target: target.to_string_lossy().into_owned(), has_catalog: t.has_catalog(), bytes, problem })
    }

    /// Moves the catalog to `target` and makes it the catalog for the next
    /// start. The databases are snapshotted first, then previews, thumbnails
    /// and faces move as folders (instant on the same disk, copied to another
    /// one). The target holds a catalog only once all of it is there, and the
    /// next start opens it only once `shoebox.json` says so, the last step.
    /// Until then the move is written down in the old folder: any failure puts
    /// everything back, and so does the next start of a Shoebox stopped
    /// halfway ([`DataDir::undo_move`]). The old folder keeps its old databases
    /// and a note saying where the catalog went until Shoebox has started on
    /// the new one, which then sends them to the Recycle Bin
    /// ([`paths::remove_old_catalog`]). Scans must be stopped.
    pub fn move_catalog(&self, target: &Path, progress: &dyn Fn(usize, usize)) -> Result<()> {
        let to = DataDir { root: target.to_path_buf() };
        if to.has_catalog() {
            bail!("{} already has a catalog", target.display());
        }
        let lock = self.data.begin_move(&to)?;
        let steps = || -> Result<()> {
            for db in paths::SIDE_DATABASES {
                let src = self.data.root.join(db);
                if src.is_file() {
                    paths::snapshot_sqlite(&src, &to)?;
                }
            }
            paths::snapshot_db(&self.db(), &to)?;
            self.data.move_files_to(&to, progress)?;
            to.place_databases()?;
            paths::remember_data_dir(&to.root, Some(&self.data.root))?;
            Ok(())
        };
        if let Err(e) = steps() {
            if let Err(undo) = lock.undo() {
                log::warn!("catalog move to {} failed and couldn't be undone yet: {undo}", to.root.display());
            }
            return Err(e);
        }
        lock.finish();
        log::info!("catalog moved to {}", to.root.display());
        let note = format!(
            "Shoebox's catalog moved to {} on {}.\r\nThis folder only holds the old one: Shoebox sends it to the Recycle Bin the next time it starts. If it's still here, it can be deleted.\r\n",
            to.root.display(),
            chrono::Local::now().format("%Y-%m-%d %H:%M")
        );
        let _ = self.data.write(&self.data.root.join("MOVED.txt"), note.as_bytes());
        Ok(())
    }

    /// Consistent copies of the catalog's databases in `backup\` inside the
    /// catalog folder, made safely while Shoebox runs - for Kopia or any
    /// backup tool, run just before each snapshot. Also leaves a
    /// `.kopiaignore` so Kopia skips the live databases and keeps these copies,
    /// the previews, thumbnails and face pictures (one unchanging file each, so
    /// a daily snapshot only adds what's new).
    pub fn backup(&self) -> Result<BackupReport> {
        let started = std::time::Instant::now();
        let dir = self.data.backup_dir();
        let mut files = Vec::new();
        {
            let db = self.db();
            self.data.snapshot_into(&db, &dir.join("catalog.db"))?;
        }
        files.push("catalog.db".to_string());
        for name in paths::SIDE_DATABASES {
            let src = self.data.root.join(name);
            if src.is_file() {
                let conn = Connection::open_with_flags(&src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                self.data.snapshot_into(&conn, &dir.join(name))?;
                files.push(name.to_string());
            }
        }
        let ignore = self.data.root.join(".kopiaignore");
        if !ignore.exists() {
            self.data.write(&ignore, KOPIA_IGNORE.as_bytes())?;
        }
        let bytes = files.iter().filter_map(|f| dir.join(f).metadata().ok()).map(|m| m.len()).sum();
        log::info!("backup copies made in {} ({} files, {bytes} bytes)", dir.display(), files.len());
        Ok(BackupReport {
            folder: dir.to_string_lossy().into_owned(),
            files,
            bytes,
            at: db::now(),
            ms: started.elapsed().as_millis() as u64,
        })
    }

    /// When `backup` last ran (its catalog copy), if ever.
    pub fn last_backup(&self) -> Option<i64> {
        let modified = self.data.backup_dir().join("catalog.db").metadata().ok()?.modified().ok()?;
        Some(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
    }

    /// Makes an existing catalog folder the catalog for the next start.
    pub fn use_catalog(&self, target: &Path) -> Result<()> {
        if !(DataDir { root: target.to_path_buf() }).has_catalog() {
            bail!("{} has no Shoebox catalog", target.display());
        }
        paths::remember_data_dir(target, None)?;
        Ok(())
    }

    /// A copy of the photo that can be read now (disk plugged in, file there), and its kind.
    pub fn original(&self, photo_id: i64) -> Result<(PathBuf, crate::formats::Kind)> {
        let (kind, files): (String, Vec<i64>) = {
            let db = self.read()?;
            let kind = db.query_row("SELECT kind FROM photos WHERE id = ?1", [photo_id], |r| r.get(0))?;
            let files = db
                .prepare(&format!("SELECT id FROM files WHERE photo_id = ?1 AND state = {FILE_PRESENT}"))?
                .query_map([photo_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            (kind, files)
        };
        let kind = if kind == crate::formats::Kind::Raw.as_str() {
            crate::formats::Kind::Raw
        } else {
            crate::formats::Kind::Image
        };
        for f in files {
            if let Ok(p) = self.file_path(f).map(PathBuf::from)
                && p.is_file()
            {
                return Ok((p, kind));
            }
        }
        bail!("No copy of this photo is plugged in")
    }

    /// The photo's long edge at full size, px, when its files say.
    pub fn full_size(&self, photo_id: i64) -> Option<u32> {
        let db = self.read().ok()?;
        let (w, h): (Option<u32>, Option<u32>) = db
            .query_row("SELECT width, height FROM photos WHERE id = ?1", [photo_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok()?;
        w.max(h).filter(|&l| l > 0)
    }

    /// Camera, exposure and a file name of each photo, by id (each found
    /// through its own row, whatever the catalog's size).
    pub fn shot_info(&self, ids: &[i64]) -> Result<HashMap<i64, crate::cull::ShotInfo>> {
        let db = self.read()?;
        let mut stmt = db.prepare(
            "SELECT p.id, p.camera_shown, p.exposure, p.focal, (SELECT MIN(f.name) FROM files f WHERE f.photo_id = p.id)
               FROM photos p WHERE p.id IN (SELECT value FROM json_each(?1))",
        )?;
        let out = stmt
            .query_map([serde_json::to_string(ids)?], |r| {
                Ok((
                    r.get(0)?,
                    crate::cull::ShotInfo { camera: r.get(1)?, exposure: r.get(2)?, focal: r.get(3)?, name: r.get(4)? },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(out)
    }

    /// Full path of a catalog folder that is there now (disk plugged in, folder found).
    pub fn folder_path(&self, volume_id: i64, rel: &str) -> Result<String> {
        let mount = self.mount_of(volume_id).ok_or_else(|| anyhow!("That disk is not plugged in"))?;
        let path = volumes::join(&mount, rel);
        if self.is_unreachable(volume_id, rel) || !Path::new(&path).is_dir() {
            bail!("{path} isn't there any more");
        }
        Ok(path)
    }

    /// Full path of a file whose disk is plugged in.
    pub fn file_path(&self, file_id: i64) -> Result<String> {
        let (vid, rel, name): (i64, String, String) = self.read()?.query_row(
            "SELECT fo.volume_id, fo.rel_path, f.name FROM files f JOIN folders fo ON fo.id = f.folder_id WHERE f.id = ?1",
            [file_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let mount = self.mount_of(vid).ok_or_else(|| anyhow!("That disk is not plugged in"))?;
        if self.is_unreachable(vid, &rel) {
            bail!("That folder isn't where it was on the disk");
        }
        Ok(format!("{}\\{}", volumes::join(&mount, &rel).trim_end_matches('\\'), name))
    }
}

/// The settings' catalog memory for this catalog's size today.
fn memory_for(s: &Settings, data: &DataDir) -> db::Memory {
    let bytes = data.db_path().metadata().map(|m| m.len()).unwrap_or(0);
    db::Memory::for_catalog(s.catalog_cache_mb, s.catalog_map_mb, bytes)
}

/// The latest few answers, by what was asked, each with the
/// [`Catalog::generation`] it was worked out at: given again only while the
/// catalog hasn't changed since.
pub(crate) struct Kept<T> {
    room: usize,
    answers: Mutex<VecDeque<Answer<T>>>,
}

/// What [`Catalog::generation`] counts: commits seen, and disk or folder changes.
pub type Generation = (u64, u64);

struct Answer<T> {
    asked: String,
    at: Generation,
    answer: T,
}

impl<T: Clone> Kept<T> {
    pub(crate) fn new(room: usize) -> Self {
        Self { room, answers: Mutex::new(VecDeque::new()) }
    }

    pub(crate) fn clear(&self) {
        self.answers.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub(crate) fn get(&self, cat: &Catalog, asked: String, work: impl FnOnce() -> Result<T>) -> Result<T> {
        // Taken before working: a change made meanwhile makes this answer old.
        let now = cat.generation()?;
        {
            let answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(a) = answers.iter().find(|a| a.asked == asked && a.at == now) {
                return Ok(a.answer.clone());
            }
        }
        let answer = work()?;
        let mut answers = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        answers.retain(|a| a.asked != asked);
        answers.push_front(Answer { asked, at: now, answer: answer.clone() });
        answers.truncate(self.room);
        Ok(answer)
    }
}

/// A reading connection from [`Catalog::read`]; back to the pool when dropped.
pub struct Reader<'a> {
    cat: &'a Catalog,
    conn: Option<Connection>,
}

impl std::ops::Deref for Reader<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("reader in use")
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        let mut idle = self.cat.readers.lock().unwrap_or_else(|e| e.into_inner());
        // As many as the window asks for at once; more are closed.
        if idle.len() < 6 {
            idle.push(conn);
        }
    }
}

/// Per photo `p`: 2 = a copy is reachable now, 1 = only on unplugged disks or
/// folders not found, 0 = missing everywhere. Looked up through the photo's
/// own files, so a small view costs little however big the catalog is.
fn state_sql(online_sql: &str) -> String {
    format!(
        "COALESCE((SELECT MAX(CASE WHEN f.state = {FILE_PRESENT} AND ({online_sql}) THEN 2
                                  WHEN f.state = {FILE_PRESENT} THEN 1 ELSE 0 END)
                     FROM files f JOIN folders fo ON fo.id = f.folder_id WHERE f.photo_id = p.id), 0)"
    )
}

/// The Status filter's value for a photo's [`state_sql`].
fn status_sql(state: &str) -> String {
    format!("CASE {state} WHEN 2 THEN 'online' WHEN 1 THEN 'offline' ELSE 'missing' END")
}

fn find_volume(db: &Connection, mv: &MountedVolume) -> Result<Option<i64>> {
    Ok(db
        .query_row(
            "SELECT id FROM volumes WHERE serial = ?1 AND fs = ?2 AND size_bytes = ?3",
            params![mv.serial as i64, mv.fs, mv.size as i64],
            |r| r.get(0),
        )
        .optional()?)
}

fn upsert_volume(db: &Connection, mv: &MountedVolume) -> Result<i64> {
    if let Some(id) = find_volume(db, mv)? {
        db.execute(
            "UPDATE volumes SET last_mount = ?1, last_seen = ?2 WHERE id = ?3",
            params![mv.mount, db::now(), id],
        )?;
        return Ok(id);
    }
    // "OS (C:)" for a drive letter; a disk mounted in a folder goes by its name.
    let at = mv.mount.trim_end_matches('\\');
    let label = if at.len() <= 2 { format!("{} ({at})", mv.default_label()) } else { mv.default_label() };
    db.execute(
        "INSERT INTO volumes (serial, fs, size_bytes, label, last_mount, last_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![mv.serial as i64, mv.fs, mv.size as i64, label, mv.mount, db::now()],
    )?;
    Ok(db.last_insert_rowid())
}

/// Kopia: keep the consistent copies in backup\, skip the live databases.
const KOPIA_IGNORE: &str = "# Written by Shoebox. For Kopia (or any tool reading .kopiaignore):\r
# back up backup\\*.db - consistent copies made by `shoebox-cli backup` just\r
# before each snapshot - and skip the live databases, which change mid-copy.\r
# To restore: with Shoebox closed, copy backup\\*.db back into this folder.\r
/*.db\r
/*.db-wal\r
/*.db-shm\r
/*.db-journal\r
*.tmp\r
/logs/\r
";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupReport {
    pub folder: String,
    pub files: Vec<String>,
    pub bytes: u64,
    /// When, seconds since 1970.
    pub at: i64,
    pub ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveReport {
    pub files: i64,
    pub photos: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub photos: i64,
    pub raw: i64,
    pub other: i64,
    pub no_preview: i64,
    pub files: i64,
    pub missing_files: i64,
    pub folders: i64,
    pub disks: i64,
    pub db_bytes: u64,
    pub preview_bytes: u64,
    pub thumb_bytes: u64,
    pub total_bytes: u64,
    /// What reading the catalog may use now (the settings' automatic values resolved).
    pub memory: db::Memory,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCheck {
    /// Where the catalog would actually go.
    pub target: String,
    pub has_catalog: bool,
    /// Size of the current catalog.
    pub bytes: u64,
    pub problem: Option<String>,
}

pub struct RootRow {
    pub id: i64,
    pub volume_id: i64,
    pub rel_path: String,
    pub volume_label: String,
}

#[derive(Serialize, Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Scope {
    All,
    Missing,
    Volume {
        id: i64,
    },
    Root {
        id: i64,
    },
    Folder {
        volume_id: i64,
        rel_path: String,
    },
    /// "2026", "2026-09" or "unknown".
    Date {
        key: String,
    },
    /// A camera or a lens, by the name shown.
    Camera {
        name: String,
    },
    Lens {
        name: String,
    },
    Person {
        id: i64,
        /// Only the photos they were found in since the user last looked at them
        /// ("Only new", to check quickly what was found).
        #[serde(default)]
        new_only: bool,
    },
    /// Marked for deletion.
    Marked,
}

/// Narrows any view. Within a filter any ticked value matches, except where a
/// photo can hold several values (people, tags): those can ask for all of
/// them. Across filters all must. Empty = no filter.
#[derive(Serialize, serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct Filters {
    /// "2026", "2026-09", "2026-09-26" or "unknown"
    pub dates: Vec<String>,
    pub cameras: Vec<String>,
    /// As shown, "Unknown lens" included.
    pub lenses: Vec<String>,
    /// File extensions, lower case: "cr3", "jpg"
    pub types: Vec<String>,
    /// "online" (plugged in), "offline" (unplugged / folder not found), "missing"
    pub status: Vec<String>,
    /// Person ids.
    pub people: Vec<i64>,
    /// Photos with any of `people` in (default), or all of them together.
    pub people_match: Match,
    /// Tag ids.
    pub tags: Vec<i64>,
    /// Photos with all of `tags` (default: tags narrow down), or any of them.
    pub tags_match: Match,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            dates: Vec::new(),
            cameras: Vec::new(),
            lenses: Vec::new(),
            types: Vec::new(),
            status: Vec::new(),
            people: Vec::new(),
            people_match: Match::Any,
            tags: Vec::new(),
            tags_match: Match::All,
        }
    }
}

/// How the values ticked in one filter combine.
#[derive(Serialize, serde::Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Match {
    /// A photo with any of them.
    #[default]
    Any,
    /// Only photos with every one of them.
    All,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Facet {
    Dates,
    Cameras,
    Lenses,
    Types,
    Status,
    People,
    Tags,
}

impl Filters {
    /// Parameters ?3..?9 of [`filters_sql`], as JSON arrays; `except` is left
    /// out, unless it asks for all of its values: then counts must narrow too.
    fn params(&self, except: Option<Facet>) -> [String; 7] {
        let json = |v: String, f: Facet| if except == Some(f) { "[]".to_string() } else { v };
        let list = |v: &Vec<String>| serde_json::to_string(v).unwrap_or_else(|_| "[]".into());
        let ids = |v: &Vec<i64>, m: Match, f: Facet| {
            let v = serde_json::to_string(v).unwrap_or_else(|_| "[]".into());
            if m == Match::All { v } else { json(v, f) }
        };
        [
            json(list(&self.dates), Facet::Dates),
            json(list(&self.cameras), Facet::Cameras),
            json(list(&self.lenses), Facet::Lenses),
            json(list(&self.types), Facet::Types),
            json(list(&self.status), Facet::Status),
            ids(&self.people, self.people_match, Facet::People),
            ids(&self.tags, self.tags_match, Facet::Tags),
        ]
    }

    pub fn is_empty(&self) -> bool {
        self.dates.is_empty()
            && self.cameras.is_empty()
            && self.lenses.is_empty()
            && self.types.is_empty()
            && self.status.is_empty()
            && self.people.is_empty()
            && self.tags.is_empty()
    }
}

/// A file's extension, lower case ("IMG_0001.CR3" -> "cr3"), on file alias `f`.
const EXT_SQL: &str = db::FILE_TYPE_SQL;

/// The filters as one SQL condition on photo `p` whose state is `state`
/// ([`state_sql`]), with parameters ?3 dates, ?4 cameras, ?5 lenses, ?6 types,
/// ?7 status, ?8 people, ?9 tags (JSON arrays).
fn filters_sql(f: &Filters, state: &str) -> String {
    let people = has_sql(f.people_match, 8, "FROM faces fa WHERE fa.photo_id = p.id", "fa.person_id");
    let tags = has_sql(f.tags_match, 9, "FROM photo_tags pt WHERE pt.photo_id = p.id", "pt.tag_id");
    let status = status_sql(state);
    format!(
        "(json_array_length(?3) = 0 OR EXISTS (SELECT 1 FROM json_each(?3) j WHERE
             (j.value = 'unknown' AND COALESCE(p.taken_ts, 0) <= 0)
          OR (COALESCE(p.taken_ts, 0) > 0
              AND strftime(CASE length(j.value) WHEN 4 THEN '%Y' WHEN 7 THEN '%Y-%m' ELSE '%Y-%m-%d' END,
                           p.taken_ts, 'unixepoch') = j.value)))
     AND (json_array_length(?4) = 0 OR p.camera_shown IN (SELECT value FROM json_each(?4)))
     AND (json_array_length(?5) = 0 OR p.lens_shown IN (SELECT value FROM json_each(?5)))
     AND (json_array_length(?6) = 0 OR EXISTS (SELECT 1 FROM files f WHERE f.photo_id = p.id
             AND {EXT_SQL} IN (SELECT value FROM json_each(?6))))
     AND (json_array_length(?7) = 0 OR {status} IN (SELECT value FROM json_each(?7)))
     AND (json_array_length(?8) = 0 OR {people})
     AND (json_array_length(?9) = 0 OR {tags})"
    )
}

/// A photo has any, or all, of the ids in JSON parameter `?n`. `rows` picks
/// the photo's rows (`FROM .. WHERE .. = p.id`), `col` their id.
fn has_sql(m: Match, n: u8, rows: &str, col: &str) -> String {
    let picked = format!("{col} IN (SELECT value FROM json_each(?{n}))");
    match m {
        Match::Any => format!("EXISTS (SELECT 1 {rows} AND {picked})"),
        Match::All => format!(
            "(SELECT COUNT(DISTINCT {col}) {rows} AND {picked}) = (SELECT COUNT(DISTINCT value) FROM json_each(?{n}))"
        ),
    }
}

/// Someone the People filter offers.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PersonValue {
    pub id: i64,
    pub name: Option<String>,
    /// Their clearest face, for the picture.
    pub cover: i64,
    /// Photos with them in, here.
    pub count: i64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FacetValue {
    pub value: String,
    pub count: i64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TypeValue {
    /// Extension, lower case.
    pub value: String,
    pub raw: bool,
    pub count: i64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Facets {
    /// Days ("2026-09-26") and "unknown", oldest first; the window rolls them up into months and years.
    pub dates: Vec<FacetValue>,
    pub cameras: Vec<FacetValue>,
    pub lenses: Vec<FacetValue>,
    pub types: Vec<TypeValue>,
    pub status: Vec<FacetValue>,
    /// Named people first, then unnamed ones seen in 2+ photos; never ignored strangers.
    pub people: Vec<PersonValue>,
    /// Tags on the photos shown, most used first.
    pub tags: Vec<crate::tags::Tag>,
}

/// Grid order. Capture-time orders keep month headers; the others are one list.
#[derive(Serialize, serde::Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Sort {
    /// Capture time, oldest first: a shoot reads in the order it was taken.
    #[default]
    Oldest,
    Newest,
    /// File name, A to Z.
    Name,
    /// Added to Shoebox, most recent first.
    Added,
}

const SEP: char = std::path::MAIN_SEPARATOR;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Browse {
    pub years: Vec<YearNode>,
    pub unknown_date: i64,
    pub gear: gear::Gear,
    pub folders: Vec<RootTree>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct YearNode {
    pub year: String,
    pub count: i64,
    pub months: Vec<MonthNode>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MonthNode {
    pub key: String,
    pub month: u32,
    pub count: i64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RootTree {
    pub root_id: i64,
    pub children: Vec<FolderNode>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FolderNode {
    pub name: String,
    pub rel_path: String,
    pub volume_id: i64,
    /// Photos in this folder and everything below it.
    pub count: i64,
    pub children: Vec<FolderNode>,
}

impl FolderNode {
    fn new(name: String, rel_path: String, volume_id: i64) -> Self {
        Self { name, rel_path, volume_id, count: 0, children: Vec::new() }
    }

    /// Adds `n` photos at `rest` (path below this node) and to every node on the way.
    fn insert(&mut self, rest: &str, full_rel: &str, volume_id: i64, n: i64) {
        self.count += n;
        if rest.is_empty() {
            return;
        }
        let (head, tail) = rest.split_once(SEP).unwrap_or((rest, ""));
        let rel = full_rel.get(..full_rel.len() - tail.len()).unwrap_or(full_rel).trim_end_matches(SEP).to_string();
        let idx = match self.children.iter().position(|c| c.name == head) {
            Some(i) => i,
            None => {
                self.children.push(FolderNode::new(head.to_string(), rel, volume_id));
                self.children.len() - 1
            }
        };
        self.children[idx].insert(tail, full_rel, volume_id, n);
    }

    /// A to Z as Explorer does: numbers by value ("IMG2" before "IMG10"), so
    /// date-named folders ("2026-09-26") run oldest first. The window flips it
    /// for "Newest first".
    fn sort(&mut self) {
        self.children.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        for c in &mut self.children {
            c.sort();
        }
    }
}

/// Case-insensitive, with runs of digits compared as numbers.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let (na, nb) = (digits(&mut a), digits(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| na.len().cmp(&nb.len()));
                if ord.is_ne() {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                // As Explorer: symbols ("_Inbox", "!old") before numbers before letters.
                let class = |c: char| {
                    if c.is_ascii_digit() {
                        1
                    } else if c.is_alphanumeric() {
                        2
                    } else {
                        0
                    }
                };
                let ord = class(x).cmp(&class(y)).then_with(|| x.to_lowercase().cmp(y.to_lowercase()));
                if ord.is_ne() {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn digits(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
        s.push(c);
        it.next();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;

    #[test]
    fn folders_sort_like_explorer() {
        let mut v = vec!["IMG10", "2026-10-12", "img2", "_Inbox", "2026-09-26", "Stack", "2026-10-03", "IMG1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["_Inbox", "2026-09-26", "2026-10-03", "2026-10-12", "IMG1", "img2", "IMG10", "Stack"]);
    }
}

#[derive(Serialize, Default, Clone)]
pub struct PhotoList {
    pub ids: Vec<i64>,
    pub taken: Vec<i64>,
    pub state: Vec<i64>,
    pub w: Vec<i64>,
    pub h: Vec<i64>,
    pub rev: Vec<i64>,
    pub preview: Vec<i64>,
    /// 1 = marked for deletion.
    pub marked: Vec<i64>,
    /// File extension, lower case ("cr3", "jpg").
    pub ext: Vec<String>,
    /// 1 = a RAW file.
    pub raw: Vec<i64>,
    /// On a person's page: 1 = they were found in it since the user last looked at them.
    pub fresh: Vec<i64>,
    /// What Cull found wrong with each photo, as bits of [`crate::cull::Problem`]
    /// (0: nothing, or not measured yet). Filled by whoever asks with Cull's measures at hand.
    pub flags: Vec<u32>,
}

/// Photos per day, for the calendar.
#[derive(Serialize, Default, Debug)]
pub struct Days {
    /// Days with photos, oldest first.
    pub days: Vec<Day>,
    /// Photos without a date.
    pub undated: i64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Day {
    /// "2006-01-14"
    pub day: String,
    pub photos: i64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub volumes: Vec<VolumeInfo>,
    pub total: i64,
    pub online: i64,
    pub offline: i64,
    pub missing: i64,
    /// Marked for deletion.
    pub marked: i64,
    pub data_dir: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub id: i64,
    pub label: String,
    pub online: bool,
    pub mount: Option<String>,
    pub last_mount: Option<String>,
    pub photos: i64,
    pub roots: Vec<RootInfo>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RootInfo {
    pub id: i64,
    /// False when the disk is plugged in but the folder isn't there.
    pub found: bool,
    pub name: String,
    pub path: String,
    pub rel_path: String,
    pub last_scan: Option<i64>,
    pub photos: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDetail {
    pub id: i64,
    pub kind: String,
    pub taken_ts: Option<i64>,
    pub taken_from_camera: bool,
    pub make: Option<String>,
    pub model: Option<String>,
    /// Camera and lens as shown everywhere (their Gear names); None when the file doesn't say.
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// The lens as the file names it.
    pub lens_in_file: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub iso: Option<i64>,
    pub fnumber: Option<f64>,
    pub exposure: Option<String>,
    pub focal: Option<f64>,
    pub preview_state: i64,
    pub preview_rev: i64,
    pub error: Option<String>,
    pub files: Vec<FileLoc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLoc {
    pub id: i64,
    pub name: String,
    pub size: i64,
    pub state: String,
    pub error: Option<String>,
    pub volume_id: i64,
    pub volume_label: String,
    pub path: String,
    /// Its folder on the disk, as `Scope::Folder` takes it (with `volume_id`): "Go to folder".
    pub folder: String,
}
