//! Turning photos. The user says how a photo stands, and Shoebox shows it so
//! everywhere: its preview and thumbnail, its faces, the viewer's sharp
//! picture, word search. **The photo file is never touched**: the turn is the
//! user's work, kept in the catalog (`photos.turn`, quarter turns clockwise on
//! top of what the file says), and applied to Shoebox's own copies.
//!
//! A turn is made from the preview and thumbnail as they are, so it needs no
//! disk. Each says in its header which turn it shows ([`imaging::mark_turn`]):
//! a turn cut short by a stop is finished at the next start from the files as
//! they are ([`settle`]), never turned twice. Faces keep their names and
//! places, their boxes turned with the photo; the photo is then looked at
//! again for faces missed while it stood the wrong way.

use crate::catalog::Catalog;
use crate::db::{PREVIEW_OK, PREVIEW_PENDING, SIDEWAYS_DECIDED};
use crate::{faces, imaging};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TurnReport {
    /// Photos turned.
    pub turned: usize,
    /// Their faces, turned with them.
    pub faces: usize,
    /// Photos whose preview couldn't be turned (damaged or missing): made
    /// again, turned, from their original when its disk is plugged in.
    pub to_read_again: Vec<i64>,
    /// Each photo turned, as it stands now: the window shows it turned where
    /// it is, and can undo exactly this turn.
    pub photos: Vec<Turned>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Turned {
    pub id: i64,
    /// This turn, quarter turns clockwise (1-3).
    pub by: i64,
    /// How the user has it turned now, on top of what its file says.
    pub turn: i64,
    /// Its preview revision and thumbnail size now.
    pub rev: i64,
    pub w: i64,
    pub h: i64,
}

/// Turns each photo clockwise by its number of quarter turns (negative:
/// counter-clockwise), on top of how it stands now. The user has decided how
/// these photos stand: the upright model never suggests turning them again.
pub fn turn(cat: &Catalog, turns: &[(i64, i64)]) -> Result<TurnReport> {
    let quality = cat.settings().preview_quality;
    let mut report = TurnReport::default();
    let hidden = cat.hidden_ids()?;
    for &(id, by) in turns.iter().filter(|(id, _)| !hidden.contains(id)) {
        let by = by.rem_euclid(4);
        if by == 0 {
            continue;
        }
        let db = cat.db();
        let was: Option<(i64, i64)> = db
            .query_row("SELECT turn, preview_state FROM photos WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        // Gone from the catalog meanwhile.
        let Some((was, preview)) = was else { continue };
        let to = (was + by).rem_euclid(4);
        // The photo's rows first, faces included, while its files still show
        // the old turn: `turning` says they're on their way.
        let moved = {
            let tx = db.unchecked_transaction()?;
            tx.execute(
                "UPDATE photos SET turn = ?1, turning = 1, sideways = ?2 WHERE id = ?3",
                params![to, SIDEWAYS_DECIDED, id],
            )?;
            let moved = faces::turn_faces(&tx, id, by)?;
            // Looked at again: faces missed while it stood the wrong way.
            tx.execute("DELETE FROM face_scan WHERE photo_id = ?1", [id])?;
            faces::copy_fingerprints(cat, &moved)?;
            tx.commit()?;
            moved
        };
        // A photo with no preview yet gets it made turned.
        if preview == PREVIEW_OK {
            if !turn_files(cat, &db, id, to, quality)? {
                report.to_read_again.push(id);
            }
        } else {
            db.execute("UPDATE photos SET turning = 0 WHERE id = ?1", [id])?;
        }
        let (rev, w, h) = db.query_row(
            "SELECT preview_rev, COALESCE(thumb_w, 0), COALESCE(thumb_h, 0) FROM photos WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        report.photos.push(Turned { id, by, turn: to, rev, w, h });
        drop(db);
        faces::after_turn(cat, &moved)?;
        report.turned += 1;
        report.faces += moved.len();
    }
    if report.turned > 0 {
        log::info!("turned {} photos ({} faces)", report.turned, report.faces);
    }
    Ok(report)
}

/// Turns these photos back to how their files show them.
pub fn back(cat: &Catalog, ids: &[i64]) -> Result<TurnReport> {
    let turns: Vec<(i64, i64)> = {
        let db = cat.read()?;
        let mut stmt =
            db.prepare("SELECT id, turn FROM photos WHERE id IN (SELECT value FROM json_each(?1)) AND turn <> 0")?;
        stmt.query_map([serde_json::to_string(ids)?], |r| Ok((r.get(0)?, -r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    turn(cat, &turns)
}

/// Brings a photo's preview and thumbnail to `to` (from the turn each says it
/// shows), then says it's done. `db` is held: nothing else writes them
/// meanwhile. A preview that can't be read (damaged, missing) is made again
/// from the original instead, as a scan makes it: false.
pub(crate) fn turn_files(cat: &Catalog, db: &Connection, id: i64, to: i64, quality: u8) -> Result<bool> {
    let to = to.rem_euclid(4) as u8;
    let turned = || -> Result<(u32, u32)> {
        // Private photos' are sealed: read and written back sealed (unlocked only).
        for (path, q) in [(cat.data.preview_path(id), quality), (cat.data.thumb_path(id), imaging::THUMB_QUALITY)] {
            if let Some((jpeg, ..)) = imaging::turn_jpeg(&cat.read_copy(&path)?, to, q)? {
                cat.write_copy(id, &path, &jpeg)?;
            }
        }
        cat.copy_dimensions(&cat.data.thumb_path(id))
    };
    match turned() {
        Ok((w, h)) => {
            // A new revision: the window asks for the new pictures.
            db.execute(
                "UPDATE photos SET turning = 0, preview_rev = preview_rev + 1, thumb_w = ?1, thumb_h = ?2
                  WHERE id = ?3 AND turn = ?4",
                params![w, h, id, to],
            )?;
            Ok(true)
        }
        Err(e) => {
            log::warn!("photo {id}: preview not turned, to be read again from its original: {e:#}");
            db.execute(
                "UPDATE photos SET turning = 0, preview_state = ?1 WHERE id = ?2 AND preview_state = ?3",
                params![PREVIEW_PENDING, id, PREVIEW_OK],
            )?;
            Ok(false)
        }
    }
}

/// Finishes the turns a stop cut short (at start): previews and thumbnails
/// brought to the turn the catalog says, face pictures cut again. Returns how many.
pub fn settle(cat: &Catalog) -> Result<usize> {
    let ids: Vec<i64> = cat
        .db()
        .prepare("SELECT id FROM photos WHERE turning = 1")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let quality = cat.settings().preview_quality;
    for &id in &ids {
        let db = cat.db();
        let (to, preview): (i64, i64) =
            db.query_row("SELECT turn, preview_state FROM photos WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if preview == PREVIEW_OK {
            turn_files(cat, &db, id, to, quality)?;
        } else {
            db.execute("UPDATE photos SET turning = 0 WHERE id = ?1", [id])?;
        }
        let faces: Vec<i64> = db
            .prepare("SELECT id FROM faces WHERE photo_id = ?1")?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        drop(db);
        faces::cut_pictures_again(cat, &faces)?;
    }
    if !ids.is_empty() {
        log::info!("{} turns cut short by a stop finished", ids.len());
    }
    Ok(ids.len())
}
