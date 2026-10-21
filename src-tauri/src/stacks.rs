//! Focus stacks: the frames of one focus bracket, one picture to come.
//!
//! A camera set to bracket focus takes a series of frames in quick
//! succession, focus a step further each time, for software to merge into
//! one picture sharp from front to back. Each frame's maker note says it is
//! part of a bracket ([`Bracket`]); none says which bracket, or which frame
//! of it. So the frames are gathered here: taken one after the other by the
//! same camera with the same bracket settings, each soon after the one
//! before, up to the number of frames the camera was set to take. A Canon
//! stops short once focus reaches infinity, so a bracket can have fewer; two
//! brackets shot back to back are told apart by the pause between them
//! (frames come a fraction of a second apart, the next bracket seconds later).
//!
//! The catalog keeps a stack as `photos.stack`, the id of its first frame,
//! on each of its frames. It is worked out again from the frames whenever
//! they change ([`assign`]): never the user's work, nothing to back up.

use crate::catalog::Catalog;
pub use crate::focus::Bracket;
use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// A pause between two frames longer than this, beyond the first one's
/// exposure and the camera's own interval, ends a bracket.
const PAUSE: f64 = 1.0;

/// A bracket as `photos.bracket` keeps it.
pub fn stored(bracket: Option<&Bracket>) -> Option<String> {
    bracket.and_then(|b| serde_json::to_string(b).ok())
}

fn parsed(s: &str) -> Option<Bracket> {
    serde_json::from_str(s).ok()
}

/// A frame of a bracket, as stacks are gathered.
#[derive(Clone, Debug)]
pub struct Frame {
    pub id: i64,
    /// When it was taken, s, with the fraction of a second its file gives.
    pub at: f64,
    /// Which camera took it: maker, model and serial number.
    pub camera: String,
    /// How long the shutter was open, s.
    pub exposure: f64,
    pub bracket: Bracket,
}

/// Frames gathered into stacks, each the ids of its frames in the order
/// taken. `frames` in the order taken. A frame alone makes no stack.
pub fn gather(frames: &[Frame]) -> Vec<Vec<i64>> {
    let mut stacks: Vec<Vec<&Frame>> = Vec::new();
    for f in frames {
        let joins = stacks.last().is_some_and(|s| {
            let prev = s[s.len() - 1];
            let gap = f.at - prev.at;
            // A RAW and its JPEG are one shot: taken at the same moment.
            let shots = s.iter().map(|g| g.at.to_bits()).collect::<HashSet<_>>().len();
            let room = f.bracket.shots == 0 || shots < f.bracket.shots as usize || gap == 0.0;
            prev.camera == f.camera
                && prev.bracket.same_settings(&f.bracket)
                && !f.bracket.refocused_after(&prev.bracket)
                && room
                && (0.0..=prev.exposure + f.bracket.interval as f64 + PAUSE).contains(&gap)
        });
        match stacks.last_mut() {
            Some(s) if joins => s.push(f),
            _ => stacks.push(vec![f]),
        }
    }
    stacks.into_iter().filter(|s| s.len() > 1).map(|s| s.iter().map(|f| f.id).collect()).collect()
}

/// Seconds the shutter was open: "1/125", "0.5s", "2s".
pub fn exposure_secs(e: &str) -> Option<f64> {
    match e.strip_prefix("1/") {
        Some(d) => d.parse::<f64>().ok().filter(|d| *d > 0.0).map(|d| 1.0 / d),
        None => e.trim_end_matches('s').parse().ok(),
    }
}

/// The camera's serial number and the fraction of a second, from a photo's
/// capture key (see `scan::capture_key`: "ext|make|model|serial|time|subsec|number").
fn serial_and_fraction(key: Option<&str>) -> (String, f64) {
    let parts: Vec<&str> = key.map(|k| k.split('|').collect()).unwrap_or_default();
    if parts.len() != 7 {
        return (String::new(), 0.0);
    }
    let digits = parts[5].trim();
    let fraction = format!("0.{digits}").parse().unwrap_or(0.0);
    (parts[3].to_string(), if digits.is_empty() { 0.0 } else { fraction })
}

/// Gathers every frame of a focus bracket in the catalog into stacks, and
/// keeps `photos.stack` in step. Reads only the frames of brackets (an index
/// of their own), so it costs what brackets there are. Returns how many
/// photos changed stack.
pub fn assign(cat: &Catalog) -> Result<usize> {
    let (mut frames, before) = {
        let db = cat.read()?;
        let frames: Vec<Frame> = db
            .prepare(
                "SELECT id, taken_ts, capture_key, make, model, exposure, bracket FROM photos
                  WHERE bracket IS NOT NULL AND COALESCE(taken_ts, 0) > 0",
            )?
            .query_map([], |r| {
                let key: Option<String> = r.get(2)?;
                let (serial, fraction) = serial_and_fraction(key.as_deref());
                let (make, model): (Option<String>, Option<String>) = (r.get(3)?, r.get(4)?);
                let exposure: Option<String> = r.get(5)?;
                let (id, taken, bracket): (i64, i64, String) = (r.get(0)?, r.get(1)?, r.get(6)?);
                Ok(parsed(&bracket).map(|bracket| Frame {
                    id,
                    at: taken as f64 + fraction,
                    camera: format!("{}|{}|{serial}", make.unwrap_or_default(), model.unwrap_or_default()),
                    exposure: exposure.as_deref().and_then(exposure_secs).unwrap_or(0.0),
                    bracket,
                }))
            })?
            .filter_map(|r| r.transpose())
            .collect::<rusqlite::Result<_>>()?;
        let before: HashMap<i64, i64> = db
            .prepare("SELECT id, stack FROM photos WHERE stack IS NOT NULL")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        (frames, before)
    };
    frames.sort_by(|a, b| a.at.total_cmp(&b.at).then(a.id.cmp(&b.id)));
    let mut after: HashMap<i64, i64> = HashMap::new();
    for stack in gather(&frames) {
        for id in &stack {
            after.insert(*id, stack[0]);
        }
    }
    let changed: Vec<(i64, Option<i64>)> = before
        .keys()
        .filter(|id| !after.contains_key(id))
        .map(|id| (*id, None))
        .chain(after.iter().filter(|(id, s)| before.get(id) != Some(s)).map(|(id, s)| (*id, Some(*s))))
        .collect();
    if !changed.is_empty() {
        let db = cat.db();
        let tx = db.unchecked_transaction()?;
        for (id, stack) in &changed {
            tx.execute("UPDATE photos SET stack = ?1 WHERE id = ?2", params![stack, id])?;
        }
        tx.commit()?;
        log::info!("focus stacks: {} photos changed stack", changed.len());
    }
    Ok(changed.len())
}

impl Catalog {
    /// The focus stack each of these photos is a frame of (its first frame's
    /// id), 0 for none.
    pub fn stacks_of(&self, ids: &[i64]) -> Result<Vec<i64>> {
        let found: HashMap<i64, i64> = self
            .read()?
            .prepare(
                "SELECT id, stack FROM photos WHERE stack IS NOT NULL AND id IN (SELECT value FROM json_each(?1))",
            )?
            .query_map([serde_json::to_string(ids)?], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids.iter().map(|id| found.get(id).copied().unwrap_or(0)).collect())
    }
}

/// A photo's place in its focus stack.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    /// The stack: its first frame's id.
    pub stack: i64,
    /// Which frame this is (1 for the first) of how many.
    pub frame: usize,
    pub frames: usize,
}

/// Where photo `id` is in `stack` (through the stacks' own index).
pub fn place(db: &Connection, stack: i64, id: i64) -> rusqlite::Result<Option<Place>> {
    let ids: Vec<i64> = db
        .prepare_cached("SELECT id FROM photos WHERE stack = ?1 ORDER BY taken_ts, capture_key, id")?
        .query_map([stack], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids.iter().position(|&i| i == id).map(|i| Place { stack, frame: i + 1, frames: ids.len() }))
}

/// A focus stack, as `shoebox-cli stacks` gives it.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    /// The id of its first frame: what `photos.stack` holds.
    pub id: i64,
    /// Its frames' ids, in the order taken.
    pub frames: Vec<i64>,
    /// When its first frame was taken (s since 1970, camera time).
    pub taken: i64,
    pub camera: Option<String>,
    /// Frames the camera was set to take.
    pub shots: u16,
}

/// Every focus stack the catalog has, oldest first; private photos only
/// while their space is open.
pub fn list(cat: &Catalog) -> Result<Vec<Stack>> {
    // Photo, stack, taken, camera, bracket.
    type Row = (i64, i64, i64, Option<String>, Option<String>);
    let rows: Vec<Row> = cat
        .read()?
        .prepare(
            "SELECT id, stack, COALESCE(taken_ts, 0), camera_shown, bracket FROM photos
              WHERE stack IS NOT NULL ORDER BY taken_ts, capture_key, id",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
    let shown: HashSet<i64> = cat.shown_ids(&ids)?.into_iter().collect();
    let mut stacks: Vec<Stack> = Vec::new();
    let mut at: HashMap<i64, usize> = HashMap::new();
    for (id, stack, taken, camera, bracket) in rows.into_iter().filter(|r| shown.contains(&r.0)) {
        let i = *at.entry(stack).or_insert_with(|| {
            let shots = bracket.as_deref().and_then(parsed).map_or(0, |b| b.shots);
            stacks.push(Stack { id: stack, frames: Vec::new(), taken, camera, shots });
            stacks.len() - 1
        });
        stacks[i].frames.push(id);
    }
    Ok(stacks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: i64, at: f64) -> Frame {
        Frame {
            id,
            at,
            camera: "Canon|EOS R6 Mark III|1".into(),
            exposure: 1.0 / 125.0,
            bracket: Bracket { shots: 25, step: 4, interval: 0, distance: None },
        }
    }

    /// Frames a tenth of a second apart, two brackets 2.4 s apart, as a
    /// Canon shot them (the first stopped at 15 frames, focus at infinity).
    #[test]
    fn brackets_back_to_back_are_two_stacks() {
        let mut frames: Vec<Frame> = (0..15).map(|i| frame(i, 1000.0 + 0.1 * i as f64)).collect();
        frames.extend((0..25).map(|i| frame(100 + i, 1003.8 + 0.1 * i as f64)));
        let stacks = gather(&frames);
        assert_eq!(stacks.iter().map(Vec::len).collect::<Vec<_>>(), [15, 25]);
        assert_eq!(stacks[1][0], 100);
    }

    /// Two brackets with hardly a pause between them: focus going back to
    /// near starts the second (an R6 Mark III's distances, cm).
    #[test]
    fn focus_going_back_to_near_starts_a_new_stack() {
        let at = |id, t, near, far| Frame {
            bracket: Bracket { distance: Some([near, far]), ..frame(id, t).bracket },
            ..frame(id, t)
        };
        let frames = [at(1, 0.0, 107, 142), at(2, 0.1, 142, 172), at(3, 0.5, 86, 96), at(4, 0.6, 86, 96)];
        assert_eq!(gather(&frames), [vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn a_bracket_never_has_more_frames_than_the_camera_was_set_to_take() {
        let frames: Vec<Frame> = (0..30).map(|i| frame(i, 1000.0 + 0.1 * i as f64)).collect();
        assert_eq!(gather(&frames).iter().map(Vec::len).collect::<Vec<_>>(), [25, 5]);
        // Shot in RAW and JPEG: 25 shots, 50 files, one stack.
        let both: Vec<Frame> = (0..50).map(|i| frame(i, 1000.0 + 0.1 * (i / 2) as f64)).collect();
        assert_eq!(gather(&both).iter().map(Vec::len).collect::<Vec<_>>(), [50]);
    }

    #[test]
    fn long_exposures_leave_room_between_frames() {
        let slow = |id, at| Frame { exposure: 2.0, ..frame(id, at) };
        assert_eq!(gather(&[slow(1, 0.0), slow(2, 2.6), slow(3, 5.2)]).len(), 1);
        // The camera's own interval between frames too.
        let waits = |id, at| Frame { bracket: Bracket { interval: 3, ..frame(id, at).bracket }, ..frame(id, at) };
        assert_eq!(gather(&[waits(1, 0.0), waits(2, 3.5)]).len(), 1);
    }

    #[test]
    fn other_cameras_or_settings_and_lone_frames_make_no_stack() {
        let other = Frame { camera: "Canon|EOS R5|2".into(), ..frame(2, 0.1) };
        assert!(gather(&[frame(1, 0.0), other]).is_empty());
        let finer = Frame { bracket: Bracket { step: 2, ..frame(2, 0.1).bracket }, ..frame(2, 0.1) };
        assert!(gather(&[frame(1, 0.0), finer]).is_empty());
        assert!(gather(&[frame(1, 0.0), frame(2, 5.0)]).is_empty());
    }

    #[test]
    fn capture_keys_give_the_serial_and_the_fraction_of_a_second() {
        let (serial, f) = serial_and_fraction(Some("cr3|Canon|EOS R6 Mark III|160802001870|20261021174402|07|"));
        assert_eq!(serial, "160802001870");
        assert!((f - 0.07).abs() < 1e-9);
        assert_eq!(serial_and_fraction(None), (String::new(), 0.0));
        assert_eq!(exposure_secs("1/125"), Some(0.008));
        assert_eq!(exposure_secs("0.5s"), Some(0.5));
    }
}
