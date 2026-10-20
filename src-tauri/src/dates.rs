//! Dates the user corrects, in the catalog only. A camera whose clock was
//! wrong (never set, reset by a flat battery, a month off) dates its photos
//! wrong, and everything that goes by date follows: the timeline, the
//! calendar, the sidebar's days, People's ages (a baby shown before he was
//! born). **The photo file is never touched**: `photos.taken_ts` is the date
//! shown and sorted by, `photos.taken_original` the date the file gives (its
//! camera's clock, or the file's own time), kept once the user moves
//! `taken_ts` away from it; NULL while nobody did. A scan reading the photo
//! again keeps the correction while its file still gives the same date; once
//! the file says something else (its date fixed in another program), the
//! file wins.
//!
//! [`check`] finds such cameras: folders named with a date ("2006-11-04
//! Birthday", or a range: "2005-02-26 03-04 Trip") whose photos from one
//! camera are days away from that date and from the other photos there.
//! [`change`] moves a set of photos' dates (by a fixed amount, to a day
//! keeping the time of day, or back to their files' dates); [`undo`] puts
//! back exactly what a change did.

use crate::catalog::Catalog;
use crate::db::{self, FILE_PRESENT};
use anyhow::{Result, anyhow, bail};
use chrono::{Datelike, NaiveDate};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Photos further than this many days from their folder's dates are listed.
pub const DAYS: i64 = 3;
const DAY: i64 = 86_400;
/// A folder name's range ("2005-02-26 03-04") longer than this is not one.
const LONGEST_RANGE: i64 = 92;

/// The dates a folder's name gives, from and to: "2006-11-04 Birthday" (one
/// day), "2005-02-26 03-04 Trip" or "2005-02-26 to 2005-03-04 Trip" (a
/// range), "2006-08-20 24 25 29 Beach" (days of that month). `-`, `_` or `.`
/// between the numbers.
pub fn name_dates(name: &str) -> Option<(NaiveDate, NaiveDate)> {
    let (from, rest) = date_at(name)?;
    let to = range_end(from, rest).unwrap_or(from);
    Some((from, to))
}

/// The folder whose name dates folder `rel` (itself, or the nearest folder
/// it is in), and its dates.
pub fn dated_folder(rel: &str) -> Option<(&str, NaiveDate, NaiveDate)> {
    let mut end = rel.len();
    loop {
        let path = &rel[..end];
        let name_at = path.rfind('\\').map_or(0, |i| i + 1);
        if let Some((from, to)) = name_dates(&path[name_at..]) {
            return Some((path, from, to));
        }
        if name_at == 0 {
            return None;
        }
        end = name_at - 1;
    }
}

fn digits(s: &str, n: usize) -> Option<(u32, &str)> {
    let d = s.get(..n)?;
    if !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((d.parse().ok()?, &s[n..]))
}

fn sep(s: &str) -> Option<&str> {
    s.strip_prefix(['-', '_', '.'])
}

fn not_a_number(s: &str) -> bool {
    !s.starts_with(|c: char| c.is_ascii_digit())
}

/// "YYYY-MM-DD" at the start of `s` (not followed by a digit), and what follows.
fn date_at(s: &str) -> Option<(NaiveDate, &str)> {
    let (y, s) = digits(s, 4)?;
    let (m, s) = digits(sep(s)?, 2)?;
    let (d, s) = digits(sep(s)?, 2)?;
    not_a_number(s).then_some(())?;
    Some((NaiveDate::from_ymd_opt(y as i32, m, d)?, s))
}

/// The last day a name gives after its first: " 03-04", "-03-04",
/// " - 2005-03-04", " to 03-04", " 24 25 29".
fn range_end(from: NaiveDate, s: &str) -> Option<NaiveDate> {
    let dashes: &[char] = &[' ', '-', '_', ',', '&', '+', '\u{2013}', '\u{2014}'];
    let s = s.trim_start_matches(dashes);
    let s = ["to ", "au ", "\u{e0} "]
        .iter()
        .find_map(|w| s.get(..w.len()).filter(|p| p.eq_ignore_ascii_case(w)).map(|_| &s[w.len()..]))
        .unwrap_or(s)
        .trim_start_matches(dashes);
    let to = date_at(s).map(|(to, _)| to).or_else(|| month_day(from, s)).or_else(|| days_of_month(from, s))?;
    (1..=LONGEST_RANGE).contains(&(to - from).num_days()).then_some(to)
}

/// "03-04" after "2005-02-26": that day, the year after when it comes
/// earlier in the year ("2005-12-28 01-03").
fn month_day(from: NaiveDate, s: &str) -> Option<NaiveDate> {
    let (m, rest) = digits(s, 2)?;
    let (d, rest) = digits(sep(rest)?, 2)?;
    not_a_number(rest).then_some(())?;
    let same_year = NaiveDate::from_ymd_opt(from.year(), m, d)?;
    if same_year < from { NaiveDate::from_ymd_opt(from.year() + 1, m, d) } else { Some(same_year) }
}

/// "24 25 29" after "2006-08-20": later days of the same month, the last one.
fn days_of_month(from: NaiveDate, mut s: &str) -> Option<NaiveDate> {
    let mut last = None;
    while let Some((d, rest)) = digits(s, 2) {
        let Some(day) = NaiveDate::from_ymd_opt(from.year(), from.month(), d) else { break };
        if !not_a_number(rest) || day <= last.unwrap_or(from) {
            break;
        }
        last = Some(day);
        s = rest.trim_start_matches([' ', ',', '&', '+', '-', '_']);
    }
    last
}

fn day_of(ts: i64) -> i64 {
    ts.div_euclid(DAY)
}

fn day_number(d: NaiveDate) -> i64 {
    (d - NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()).num_days()
}

fn day_text(day: i64) -> String {
    chrono::DateTime::from_timestamp(day * DAY, 0).map(|t| t.date_naive().to_string()).unwrap_or_default()
}

/// Days between days `lo..=hi` and `from..=to` (0 when they meet).
fn gap((lo, hi): (i64, i64), (from, to): (i64, i64)) -> i64 {
    if hi < from {
        from - hi
    } else if lo > to {
        lo - to
    } else {
        0
    }
}

/// What the date check found.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DateCheck {
    /// Photos further than this many days from their folder's dates are listed.
    pub days: i64,
    /// Folders looked at (named with a date, or in one), and their photos with a date.
    pub folders: usize,
    pub photos: usize,
    /// Photos of the whole catalog whose date was corrected.
    pub corrected: usize,
    /// Findings left as is: left out, unless all were asked for (then listed, `left` set).
    pub left: usize,
    /// The most photos first, then the furthest from their folder's dates.
    pub findings: Vec<Finding>,
}

/// Photos from one camera in one folder, days away from the folder's dates.
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// The folder they are in (`volumeId` and `relPath` make its scope).
    pub folder_id: i64,
    pub volume_id: i64,
    pub rel_path: String,
    /// The folder whose name gives the date: the same one, or a folder it is in.
    pub dated: String,
    /// The dates its name gives, from and to (the same day unless a range).
    pub from: String,
    pub to: String,
    /// The camera as shown everywhere; none when the files don't say.
    pub camera: Option<String>,
    /// Its body's serial number, when the files give it.
    pub serial: Option<String>,
    /// No date in the files: theirs is the time the file was written.
    pub file_date: bool,
    /// The camera as "Leave as is" keeps it.
    pub key: String,
    /// The photos, oldest first, and their first and last dates now.
    pub photos: Vec<i64>,
    pub first: i64,
    pub last: i64,
    /// Days from the folder's dates (negative: before them).
    pub days_off: i64,
    /// Seconds that move the first photo to the folder's first day, its time
    /// of day kept (the others by as much): a [`Change::By`].
    pub shift: i64,
    /// The other photos of the dated folder (and the folders in it) that
    /// agree with its dates, and their first and last day.
    pub others: usize,
    pub others_from: Option<String>,
    pub others_to: Option<String>,
    /// Photos among them whose date was corrected already.
    pub corrected: usize,
    /// Left as is (listed only when all are asked for).
    pub left: bool,
}

/// A finding the user left as is: kept by folder, camera and first day.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LeftDates {
    pub folder_id: i64,
    pub key: String,
    /// The first photo's date (its day is what counts).
    pub first: i64,
}

impl From<&Finding> for LeftDates {
    fn from(f: &Finding) -> Self {
        Self { folder_id: f.folder_id, key: f.key.clone(), first: f.first }
    }
}

/// A dated folder and the folders in it that no closer name dates.
struct Event {
    from: NaiveDate,
    to: NaiveDate,
    /// Folder id, path, and whether it is in what was asked about.
    folders: Vec<(i64, String, bool)>,
}

/// A photo in a [`Run`]: its id, date, and whether it was corrected.
type Shot = (i64, i64, bool);

/// A camera as the check tells them apart: as shown, its body's serial
/// number, and whether its photos are dated by their files.
type Camera = (Option<String>, Option<String>, bool);

/// Photos of one camera in one folder whose days follow one another (no gap
/// of more than the check's days).
struct Run {
    folder: usize,
    camera: Option<String>,
    serial: Option<String>,
    file_date: bool,
    photos: Vec<Shot>,
}

impl Run {
    fn days(&self) -> (i64, i64) {
        (day_of(self.photos[0].1), day_of(self.photos[self.photos.len() - 1].1))
    }

    fn key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.camera.as_deref().unwrap_or(""),
            self.serial.as_deref().unwrap_or(""),
            if self.file_date { "file" } else { "camera" }
        )
    }
}

/// Folders named with a date (or in one) whose photos from one camera are
/// more than `days` away from that date and from the other photos there:
/// a camera whose clock was wrong. In folder `within` (volume id, path) and
/// those in it, or the whole catalog. Nothing changes. `all`: findings left
/// as is too, flagged.
pub fn check(cat: &Catalog, within: Option<(i64, &str)>, days: i64, all: bool) -> Result<DateCheck> {
    let days = days.max(0);
    let db = cat.read()?;
    let mut events: BTreeMap<(i64, String), Event> = BTreeMap::new();
    let folders: Vec<(i64, i64, String)> = db
        .prepare("SELECT id, volume_id, rel_path FROM folders ORDER BY volume_id, rel_path")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, volume, rel) in folders {
        let Some((dated, from, to)) = dated_folder(&rel) else { continue };
        let inside = within.is_none_or(|(v, w)| v == volume && db::is_under(&rel, w));
        let event = events.entry((volume, dated.to_string())).or_insert(Event { from, to, folders: Vec::new() });
        event.folders.push((id, rel, inside));
    }
    let left = left_as_is(&db)?;
    let corrected: i64 =
        db.query_row("SELECT COUNT(*) FROM photos WHERE taken_original IS NOT NULL", [], |r| r.get(0))?;
    let mut photos_of = db.prepare(&format!(
        "SELECT p.id, p.taken_ts, NULLIF(p.camera_shown, ?2), p.capture_key, p.taken_exif, p.taken_original IS NOT NULL
           FROM files f JOIN photos p ON p.id = f.photo_id
          WHERE f.folder_id = ?1 AND f.state = {FILE_PRESENT} AND p.taken_ts > 0 AND {}
          ORDER BY p.taken_ts, p.id",
        cat.shown_sql("p")
    ))?;
    let mut check =
        DateCheck { days, folders: 0, photos: 0, corrected: corrected as usize, left: 0, findings: Vec::new() };
    for ((volume, dated), event) in events.into_iter().filter(|(_, e)| e.folders.iter().any(|f| f.2)) {
        let mut runs: Vec<Run> = Vec::new();
        for (i, (folder, _, inside)) in event.folders.iter().enumerate() {
            let rows: Vec<Row> = photos_of
                .query_map(params![folder, crate::gear::Kind::Camera.unknown()], |r| {
                    Ok(Row {
                        id: r.get(0)?,
                        ts: r.get(1)?,
                        camera: r.get(2)?,
                        key: r.get(3)?,
                        file_date: r.get::<_, i64>(4)? == 0,
                        corrected: r.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            if *inside {
                check.folders += 1;
                check.photos += rows.len();
            }
            runs.extend(runs_of(i, rows, days));
        }
        // The folder's dates, and the photos that meet them or meet those
        // that do: what agrees with the folder. The rest is a finding.
        let (from, to) = (day_number(event.from), day_number(event.to));
        let mut window = (from, to);
        let mut agrees = vec![false; runs.len()];
        loop {
            let mut grew = false;
            for (i, run) in runs.iter().enumerate() {
                if !agrees[i] && gap(run.days(), window) <= days {
                    agrees[i] = true;
                    let (lo, hi) = run.days();
                    window = (window.0.min(lo), window.1.max(hi));
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        let agreeing = runs.iter().zip(&agrees).filter(|(_, a)| **a).map(|(r, _)| r);
        let others = agreeing.clone().map(|r| r.photos.len()).sum();
        let others_from = agreeing.clone().map(|r| r.days().0).min().map(day_text);
        let others_to = agreeing.map(|r| r.days().1).max().map(day_text);
        for (run, _) in runs.iter().zip(&agrees).filter(|(_, a)| !**a) {
            let (folder, rel, inside) = &event.folders[run.folder];
            if !inside {
                continue;
            }
            let (lo, hi) = run.days();
            let first = run.photos[0].1;
            let is_left = left.contains(&(*folder, run.key(), lo));
            check.left += is_left as usize;
            if is_left && !all {
                continue;
            }
            check.findings.push(Finding {
                folder_id: *folder,
                volume_id: volume,
                rel_path: rel.clone(),
                dated: dated.clone(),
                from: event.from.to_string(),
                to: event.to.to_string(),
                camera: run.camera.clone(),
                serial: run.serial.clone(),
                file_date: run.file_date,
                key: run.key(),
                photos: run.photos.iter().map(|p| p.0).collect(),
                first,
                last: run.photos[run.photos.len() - 1].1,
                days_off: if hi < from { hi - from } else { lo - to },
                shift: (from - lo) * DAY,
                others,
                others_from: others_from.clone(),
                others_to: others_to.clone(),
                corrected: run.photos.iter().filter(|p| p.2).count(),
                left: is_left,
            });
        }
    }
    check.findings.sort_by(|a, b| {
        (b.photos.len(), b.days_off.abs(), &a.rel_path).cmp(&(a.photos.len(), a.days_off.abs(), &b.rel_path))
    });
    Ok(check)
}

/// A photo of a folder, as the check reads it.
struct Row {
    id: i64,
    ts: i64,
    /// As shown; none when unknown.
    camera: Option<String>,
    /// Its capture key (scan.rs), which holds the body's serial number.
    key: Option<String>,
    file_date: bool,
    corrected: bool,
}

/// A folder's photos (in date order) as runs: by camera, then split where
/// `days` pass without a photo.
fn runs_of(folder: usize, rows: Vec<Row>, days: i64) -> Vec<Run> {
    // The body's serial number, last but three in the capture key
    // (scan::capture_key). Photos that don't give it go with their camera's
    // only body in the folder, when there is one.
    let serial_of = |key: &Option<String>| {
        key.as_deref().and_then(|k| k.rsplit('|').nth(3)).filter(|s| !s.is_empty()).map(str::to_string)
    };
    let mut bodies: HashMap<Option<String>, HashSet<String>> = HashMap::new();
    for r in &rows {
        if let Some(s) = serial_of(&r.key) {
            bodies.entry(r.camera.clone()).or_default().insert(s);
        }
    }
    let mut cameras: BTreeMap<Camera, Vec<Shot>> = BTreeMap::new();
    for r in rows {
        let serial = serial_of(&r.key).or_else(|| match bodies.get(&r.camera) {
            Some(only) if only.len() == 1 => only.iter().next().cloned(),
            _ => None,
        });
        cameras.entry((r.camera, serial, r.file_date)).or_default().push((r.id, r.ts, r.corrected));
    }
    let mut runs = Vec::new();
    for ((camera, serial, file_date), photos) in cameras {
        let mut run: Vec<Shot> = Vec::new();
        for p in photos {
            if run.last().is_some_and(|last| day_of(p.1) - day_of(last.1) > days) {
                let photos = std::mem::take(&mut run);
                runs.push(Run { folder, camera: camera.clone(), serial: serial.clone(), file_date, photos });
            }
            run.push(p);
        }
        if !run.is_empty() {
            runs.push(Run { folder, camera, serial, file_date, photos: run });
        }
    }
    runs
}

/// The folder a check is about, as [`check`] takes it, from a view's scope:
/// a folder, a source or a disk (and the folders in it), or everything.
pub fn within(cat: &Catalog, scope: &crate::catalog::Scope) -> Result<Option<(i64, String)>> {
    use crate::catalog::Scope;
    Ok(match scope {
        Scope::All => None,
        Scope::Volume { id } => Some((*id, String::new())),
        Scope::Root { id } => {
            let root = cat.root(*id)?;
            Some((root.volume_id, root.rel_path))
        }
        Scope::Folder { volume_id, rel_path } => Some((*volume_id, rel_path.clone())),
        _ => bail!("Dates are checked in a folder, a source, a disk or everywhere"),
    })
}

/// Findings left as is: folder, camera and first day.
fn left_as_is(db: &Connection) -> Result<HashSet<(i64, String, i64)>> {
    let rows: Vec<(i64, String, String)> = db
        .prepare("SELECT folder_id, camera, first_day FROM dates_left")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(f, c, d)| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok().map(|d| (f, c, day_number(d))))
        .collect())
}

/// "Leave as is" on findings of the check: looked at and right as they are
/// (a folder named after the day the photos were copied, a trip). The check
/// leaves them out from then on. Not `left`: back in the check.
pub fn leave_as_is(cat: &Catalog, findings: &[LeftDates], left: bool) -> Result<usize> {
    let db = cat.db();
    let tx = db.unchecked_transaction()?;
    let mut n = 0;
    for f in findings {
        let day = day_text(day_of(f.first));
        n += if left {
            tx.execute(
                "INSERT OR REPLACE INTO dates_left (folder_id, camera, first_day, left_at) VALUES (?1, ?2, ?3, ?4)",
                params![f.folder_id, f.key, day, db::now()],
            )?
        } else {
            tx.execute(
                "DELETE FROM dates_left WHERE folder_id = ?1 AND camera = ?2 AND first_day = ?3",
                params![f.folder_id, f.key, day],
            )?
        };
    }
    tx.commit()?;
    Ok(n)
}

/// How to change some photos' dates.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Change {
    /// Later by this many seconds (earlier when negative).
    By { seconds: i64 },
    /// The first of them to this day ("2006-11-04"), its time of day kept;
    /// the others moved by as much.
    To { date: String },
    /// Back to the dates their files give.
    Back,
}

/// What a change did: [`undo`] puts it back.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Shifted {
    /// The photos whose date moved.
    pub photos: Vec<Redated>,
    /// Photos left as they were: no date to move, already there, or it would
    /// fall before 1970.
    #[serde(default)]
    pub unchanged: usize,
}

/// One photo's date moved.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Redated {
    pub id: i64,
    /// Seconds later (negative: earlier).
    pub by: i64,
    /// Its date before and after.
    #[serde(default)]
    pub from: i64,
    #[serde(default)]
    pub to: i64,
}

/// Moves the dates of photos `ids` (those shown). Only the catalog changes.
pub fn change(cat: &Catalog, ids: &[i64], how: &Change) -> Result<Shifted> {
    let ids = cat.shown_ids(ids)?;
    let list = serde_json::to_string(&ids)?;
    let moves: Vec<(i64, i64)> = match how {
        Change::By { seconds } => ids.iter().map(|id| (*id, *seconds)).collect(),
        Change::To { date } => {
            let day = NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
                .map_err(|_| anyhow!("{date} isn't a day (YYYY-MM-DD)"))?;
            let first: Option<i64> = cat.read()?.query_row(
                "SELECT MIN(taken_ts) FROM photos WHERE id IN (SELECT value FROM json_each(?1)) AND taken_ts > 0",
                [&list],
                |r| r.get(0),
            )?;
            match first {
                Some(first) => {
                    let by = (day_number(day) - day_of(first)) * DAY;
                    ids.iter().map(|id| (*id, by)).collect()
                }
                None => Vec::new(),
            }
        }
        Change::Back => cat
            .read()?
            .prepare(
                "SELECT id, taken_original - taken_ts FROM photos
                  WHERE id IN (SELECT value FROM json_each(?1)) AND taken_original IS NOT NULL",
            )?
            .query_map([&list], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?,
    };
    let mut done = apply(cat, &moves)?;
    done.unchanged = ids.len() - done.photos.len();
    Ok(done)
}

/// Puts back what [`change`] did: each photo moved back by as much.
pub fn undo(cat: &Catalog, done: &Shifted) -> Result<Shifted> {
    let shown: HashSet<i64> =
        cat.shown_ids(&done.photos.iter().map(|p| p.id).collect::<Vec<_>>())?.into_iter().collect();
    let moves: Vec<(i64, i64)> = done.photos.iter().filter(|p| shown.contains(&p.id)).map(|p| (p.id, -p.by)).collect();
    let mut back = apply(cat, &moves)?;
    back.unchanged = done.photos.len() - back.photos.len();
    Ok(back)
}

/// Each photo's date later by its seconds, in one transaction. The file's
/// own date is kept the first time; a photo back on it forgets it.
fn apply(cat: &Catalog, moves: &[(i64, i64)]) -> Result<Shifted> {
    let db = cat.db();
    let tx = db.unchecked_transaction()?;
    let mut done = Shifted::default();
    {
        let mut moved = tx.prepare(
            "UPDATE photos SET taken_ts = taken_ts + ?2,
                    taken_original = CASE WHEN taken_ts + ?2 = COALESCE(taken_original, taken_ts) THEN NULL
                                          ELSE COALESCE(taken_original, taken_ts) END
              WHERE id = ?1 AND taken_ts > 0 AND taken_ts + ?2 > 0
             RETURNING taken_ts",
        )?;
        for &(id, by) in moves.iter().filter(|(_, by)| *by != 0) {
            if let Some(to) = moved.query_row(params![id, by], |r| r.get::<_, i64>(0)).optional()? {
                done.photos.push(Redated { id, by, from: to - by, to });
            }
        }
    }
    tx.commit()?;
    Ok(done)
}

/// Some photos' dates, as "Fix dates" shows them before a change.
#[derive(Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Dates {
    /// Photos asked about (those shown), and those with a date.
    pub photos: usize,
    pub dated: usize,
    /// Their first and last date.
    pub first: Option<i64>,
    pub last: Option<i64>,
    /// Photos among them whose date was corrected.
    pub corrected: usize,
    /// The folder whose name dates most of them, and its first day: where
    /// "to the folder's date" takes them.
    pub folder: Option<String>,
    pub folder_date: Option<String>,
}

pub fn of(cat: &Catalog, ids: &[i64]) -> Result<Dates> {
    let ids = cat.shown_ids(ids)?;
    let list = serde_json::to_string(&ids)?;
    let db = cat.read()?;
    let (dated, first, last, corrected): (i64, Option<i64>, Option<i64>, i64) = db.query_row(
        "SELECT COUNT(*), MIN(taken_ts), MAX(taken_ts), COUNT(taken_original) FROM photos
          WHERE id IN (SELECT value FROM json_each(?1)) AND taken_ts > 0",
        [&list],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let mut by_folder: HashMap<(i64, String), (NaiveDate, usize)> = HashMap::new();
    let folders = db
        .prepare(&format!(
            "SELECT fo.volume_id, fo.rel_path, COUNT(DISTINCT f.photo_id) FROM files f JOIN folders fo ON fo.id = f.folder_id
              WHERE f.photo_id IN (SELECT value FROM json_each(?1)) AND f.state = {FILE_PRESENT} GROUP BY fo.id"
        ))?
        .query_map([&list], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (volume, rel, n) in folders {
        if let Some((dated, from, _)) = dated_folder(&rel) {
            by_folder.entry((volume, dated.to_string())).or_insert((from, 0)).1 += n as usize;
        }
    }
    let most = by_folder.into_iter().max_by(|a, b| (a.1.1, &b.0).cmp(&(b.1.1, &a.0)));
    Ok(Dates {
        photos: ids.len(),
        dated: dated as usize,
        first,
        last,
        corrected: corrected as usize,
        folder: most.as_ref().map(|((_, path), _)| path.rsplit('\\').next().unwrap_or(path).to_string()),
        folder_date: most.map(|(_, (date, _))| date.to_string()),
    })
}

/// An amount of time as the CLI takes it: "264d", "-30d", "+1d2h30m", "-90s".
pub fn parse_amount(s: &str) -> Result<i64> {
    let bad = || anyhow!("{s}: say how much, as days, hours, minutes, seconds (e.g. 264d, -1h, 2h30m)");
    let t = s.trim();
    let (sign, mut rest) = match t.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, t.strip_prefix('+').unwrap_or(t)),
    };
    if rest.is_empty() {
        return Err(bad());
    }
    let mut total: i64 = 0;
    while !rest.is_empty() {
        let n_end = rest.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
        let n: i64 = rest[..n_end].parse().map_err(|_| bad())?;
        let unit = match rest[n_end..].chars().next() {
            Some('d') => DAY,
            Some('h') => 3600,
            Some('m') => 60,
            Some('s') => 1,
            _ => return Err(bad()),
        };
        total = n.checked_mul(unit).and_then(|x| total.checked_add(x)).ok_or_else(bad)?;
        rest = &rest[n_end + 1..];
    }
    if total == 0 {
        bail!("{s}: that moves nothing");
    }
    Ok(sign * total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn folder_names_give_a_day_or_a_range() {
        assert_eq!(name_dates("2006-11-04 1st Birthday"), Some((d("2006-11-04"), d("2006-11-04"))));
        assert_eq!(name_dates("2006_11_04"), Some((d("2006-11-04"), d("2006-11-04"))));
        assert_eq!(name_dates("2006.11.04-party"), Some((d("2006-11-04"), d("2006-11-04"))));
        // A range: month and day, or a whole date, with or without a word between.
        assert_eq!(name_dates("2005-02-26 03-04 Trip"), Some((d("2005-02-26"), d("2005-03-04"))));
        assert_eq!(name_dates("2005-02-26-03-04"), Some((d("2005-02-26"), d("2005-03-04"))));
        assert_eq!(name_dates("2005-02-26 to 2005-03-04 Trip"), Some((d("2005-02-26"), d("2005-03-04"))));
        assert_eq!(name_dates("2005-02-26 au 03-04"), Some((d("2005-02-26"), d("2005-03-04"))));
        assert_eq!(name_dates("2005-12-28 01-03 Ski"), Some((d("2005-12-28"), d("2006-01-03"))));
        // Days of the month.
        assert_eq!(name_dates("2006-08-20 24 25 29 Beach"), Some((d("2006-08-20"), d("2006-08-29"))));
        assert_eq!(name_dates("2006-08-20, 24 & 25"), Some((d("2006-08-20"), d("2006-08-25"))));
        assert_eq!(name_dates("2005-02-26-28"), Some((d("2005-02-26"), d("2005-02-28"))));
        // Not ranges: a number that isn't a day, the same day, too long.
        assert_eq!(name_dates("2006-11-04 03 friends"), Some((d("2006-11-04"), d("2006-11-04"))));
        assert_eq!(name_dates("2006-11-04 11-04"), Some((d("2006-11-04"), d("2006-11-04"))));
        assert_eq!(name_dates("2006-01-04 12-30"), Some((d("2006-01-04"), d("2006-01-04"))));
        // Not dates.
        for name in ["20061104 Party", "2006-13-01 x", "2006-02-30", "2006-11-045", "Guest", "106-11-04", ""] {
            assert_eq!(name_dates(name), None, "{name}");
        }
    }

    #[test]
    fn a_folder_is_dated_by_its_own_name_or_the_nearest_folder_it_is_in() {
        let (path, from, _) = dated_folder("Photos\\2006\\2006-11-04 1st Birthday\\Guest").unwrap();
        assert_eq!((path, from), ("Photos\\2006\\2006-11-04 1st Birthday", d("2006-11-04")));
        let (path, ..) = dated_folder("2006-11-04 Match\\2006-11-05 Day two\\Raw").unwrap();
        assert_eq!(path, "2006-11-04 Match\\2006-11-05 Day two");
        assert!(dated_folder("Photos\\2006\\Misc").is_none());
        assert!(dated_folder("").is_none());
    }

    #[test]
    fn amounts_of_time() {
        assert_eq!(parse_amount("264d").unwrap(), 264 * DAY);
        assert_eq!(parse_amount("-30d").unwrap(), -30 * DAY);
        assert_eq!(parse_amount("+1d2h30m").unwrap(), DAY + 2 * 3600 + 30 * 60);
        assert_eq!(parse_amount("-90s").unwrap(), -90);
        for bad in ["", "-", "12", "3x", "d", "0d", "1d-2h"] {
            assert!(parse_amount(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn days_apart() {
        assert_eq!(gap((1, 2), (5, 9)), 3);
        assert_eq!(gap((10, 12), (5, 9)), 1);
        assert_eq!(gap((4, 6), (5, 9)), 0);
    }
}
