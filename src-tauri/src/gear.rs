//! Gear: the cameras and lenses the photos were taken with, one name each.
//!
//! Files name the same gear in different ways, whatever the brand. A Canon RAW
//! read through rawler's lens table says "Canon EF 70-200mm f/4L IS USM", the
//! camera's own JPEG "EF70-200mm f/4L IS USM"; a Fuji's JPEG "XF23mmF1.4 R",
//! rawler "Fujifilm XF 23mm F1.4 R"; a Nikon's JPEG "NIKON D850", rawler
//! "Nikon D850". The catalog keeps what each file says. Names that differ only
//! by the maker's name, case, spaces or "f/4" vs "F4" are grouped by
//! themselves and shown under their fullest spelling ([`tidy`], after every
//! scan). The user renames and merges the rest (Gear in the sidebar,
//! `shoebox-cli gear-rename`); their names win, and what comes later - new
//! photos, rescans, another spelling of the same lens - falls in with them.
//! Names live in the catalog only, never in the photo files.

use crate::catalog::Catalog;
use crate::imaging;
use anyhow::{Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};

/// Version of the grouping rules; a catalog grouped with older ones is grouped again at start.
const GEAR_RULES: &str = "2";

/// The camera as its files name it, on photo `p`, NULL when they don't. One
/// name whatever wrote the file: rawler says "EOS R6 Mark III", the camera's
/// JPEG "Canon EOS R6 Mark III"; "NIKON CORPORATION" + "NIKON D850" is "NIKON D850".
macro_rules! camera_files {
    () => {
        "NULLIF(TRIM(CASE
            WHEN p.model IS NULL THEN COALESCE(p.make, '')
            WHEN p.make IS NOT NULL AND p.model NOT LIKE substr(p.make, 1, instr(p.make || ' ', ' ') - 1) || '%'
                THEN p.make || ' ' || p.model
            ELSE p.model END), '')"
    };
}
macro_rules! lens_files {
    () => {
        "NULLIF(TRIM(p.lens), '')"
    };
}
/// As shown everywhere: the group's or the user's name, else the files', else "Unknown ...".
macro_rules! shown {
    ($kind:literal, $files:expr, $unknown:literal) => {
        concat!(
            "COALESCE((SELECT g.shown FROM gear_names g WHERE g.kind = '",
            $kind,
            "' AND g.name = ",
            $files,
            "), ",
            $files,
            ", '",
            $unknown,
            "')"
        )
    };
}

pub const CAMERA_FILES_SQL: &str = camera_files!();
pub const LENS_FILES_SQL: &str = lens_files!();
/// The camera's name as shown (filters, sidebar, viewer), on photo `p`.
pub const CAMERA_SQL: &str = shown!("camera", camera_files!(), "Unknown camera");
/// The lens's name as shown, on photo `p`.
pub const LENS_SQL: &str = shown!("lens", lens_files!(), "Unknown lens");

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Camera,
    Lens,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Kind> {
        match s {
            "camera" | "cameras" => Ok(Kind::Camera),
            "lens" | "lenses" => Ok(Kind::Lens),
            _ => bail!("camera or lens, not {s:?}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Kind::Camera => "camera",
            Kind::Lens => "lens",
        }
    }

    /// Photos that don't say: shown, never renamed.
    pub fn unknown(self) -> &'static str {
        match self {
            Kind::Camera => "Unknown camera",
            Kind::Lens => "Unknown lens",
        }
    }

    fn files_sql(self) -> &'static str {
        match self {
            Kind::Camera => CAMERA_FILES_SQL,
            Kind::Lens => LENS_FILES_SQL,
        }
    }

    pub fn shown_sql(self) -> &'static str {
        match self {
            Kind::Camera => CAMERA_SQL,
            Kind::Lens => LENS_SQL,
        }
    }
}

/// A name as a file gives it, tidied: inner spaces collapsed. None for one
/// that says nothing ("", "----", "0.0 mm f/0.0": a lens that didn't report).
pub fn file_name(raw: &str) -> Option<String> {
    let n = raw.trim_matches('\0').split_whitespace().collect::<Vec<_>>().join(" ");
    if n.chars().all(|c| "0.-/ fFmM".contains(c)) {
        return None;
    }
    Some(n)
}

/// Makers that start lens names (rawler's table, Lightroom, some cameras).
/// Longest first, so "carl zeiss" is taken before "zeiss".
const MAKERS: &[&str] = &[
    "om digital solutions",
    "carl zeiss",
    "om system",
    "voigtlander",
    "hasselblad",
    "panasonic",
    "7artisans",
    "ttartisan",
    "fujifilm",
    "olympus",
    "samsung",
    "samyang",
    "rokinon",
    "viltrox",
    "yongnuo",
    "minolta",
    "mitakon",
    "pentax",
    "tamron",
    "tokina",
    "cosina",
    "google",
    "canon",
    "nikon",
    "sigma",
    "leica",
    "lumix",
    "laowa",
    "apple",
    "ricoh",
    "zeiss",
    "meike",
    "sirui",
    "venus",
    "sony",
    "fuji",
    "irix",
    "dji",
];

/// What tells gear apart: the maker, when named, and the rest of the name
/// without case, spaces, "f/" or a trailing ".0" ("f/4.0" = "F4").
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    maker: Option<String>,
    rest: String,
}

fn squash(s: &str) -> String {
    let lower: Vec<char> = s.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = String::with_capacity(lower.len());
    let mut i = 0;
    while i < lower.len() {
        let c = lower[i];
        // "f/2.8" -> "f2.8"
        if c == 'f' && lower.get(i + 1) == Some(&'/') {
            out.push('f');
            i += 2;
            continue;
        }
        // "4.0" -> "4", "70.0-200.0mm" -> "70-200mm" (not "4.05")
        if c == '.'
            && lower.get(i + 1) == Some(&'0')
            && i > 0
            && lower[i - 1].is_ascii_digit()
            && !lower.get(i + 2).is_some_and(|d| d.is_ascii_digit())
        {
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn lens_key(name: &str) -> Key {
    let lower = name.to_lowercase();
    for m in MAKERS {
        if let Some(rest) = lower.strip_prefix(m)
            && (rest.is_empty() || rest.starts_with(' '))
        {
            // "Fuji" and "Fujifilm" are one maker.
            let maker = if *m == "fuji" { "fujifilm" } else { m };
            return Key { maker: Some(maker.to_string()), rest: squash(rest) };
        }
    }
    Key { maker: None, rest: squash(&lower) }
}

/// A camera: its maker's first word ("NIKON CORPORATION" and "Nikon" are
/// one), and the model without the maker's words.
fn camera_key(make: Option<&str>, model: Option<&str>) -> Key {
    let make = make.unwrap_or("").to_lowercase();
    let words: Vec<&str> = make.split_whitespace().collect();
    let model = model.unwrap_or("").to_lowercase();
    let rest: Vec<&str> = model.split_whitespace().filter(|w| !words.contains(w)).collect();
    let first = words.first().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string());
    Key { maker: first.filter(|w| !w.is_empty()), rest: squash(&rest.join(" ")) }
}

/// What a lens is, as its name says: focal range (mm), and widest aperture
/// at both ends of a zoom (the same twice for a prime or a constant zoom).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Spec {
    short: f32,
    long: f32,
    wide_f: f32,
    tele_f: f32,
}

impl Spec {
    /// A photo's lens named by what it is (`photo`) could be this lens.
    /// Its apertures at both ends, when it gives both ("f/3.5-5.6"), or the
    /// one it gives (some cameras say the widest at the focal length used).
    fn holds(&self, photo: &Spec) -> bool {
        let near = |a: f32, b: f32| (a - b).abs() < 0.6;
        let close = |a: f32, b: f32| (a / b).ln().abs() < 0.06;
        let aperture = if photo.tele_f > photo.wide_f + 0.05 {
            close(photo.wide_f, self.wide_f) && close(photo.tele_f, self.tele_f)
        } else {
            photo.wide_f >= self.wide_f * 0.97 && photo.wide_f <= self.tele_f * 1.03
        };
        near(self.short, photo.short) && near(self.long, photo.long) && aperture
    }
}

/// Standard f-numbers, in thirds of a stop: an aperture shows as the nearest.
const F_STOPS: [f32; 31] = [
    1.0, 1.1, 1.2, 1.4, 1.6, 1.8, 2.0, 2.2, 2.5, 2.8, 3.2, 3.5, 4.0, 4.5, 5.0, 5.6, 6.3, 7.1, 8.0, 9.0, 10.0, 11.0,
    13.0, 14.0, 16.0, 18.0, 20.0, 22.0, 25.0, 29.0, 32.0,
];

/// A lens named by what it is - its focal range, and widest aperture at both
/// ends - for files that say no more (no name, no id it can be found by):
/// "24-70mm f/2.8", "18-55mm f/3.5-5.6", "50mm".
pub fn spec_name(short: f32, long: f32, aperture: Option<(f32, f32)>) -> Option<String> {
    if !(short > 0.0 && long >= short && long < 10_000.0) {
        return None;
    }
    let num = |v: f32| {
        let r = (v * 10.0).round() / 10.0;
        if r.fract() == 0.0 { format!("{}", r as i64) } else { format!("{r}") }
    };
    let focal = if long - short < 0.05 { num(short) } else { format!("{}-{}", num(short), num(long)) };
    let stop =
        |a: f32| F_STOPS.iter().copied().min_by(|x, y| (x.ln() - a.ln()).abs().total_cmp(&(y.ln() - a.ln()).abs()));
    let f = |a: f32| Some(a).filter(|a| (0.7..=64.0).contains(a)).and_then(stop);
    Some(match aperture.and_then(|(w, t)| f(w).map(|w| (w, f(t).unwrap_or(w)))) {
        Some((w, t)) if t > w => format!("{focal}mm f/{}-{}", num(w), num(t)),
        Some((w, _)) => format!("{focal}mm f/{}", num(w)),
        None => format!("{focal}mm"),
    })
}

/// "4.5-5.6" -> (4.5, 5.6); "2.8" -> (2.8, 2.8).
fn num_range(s: &str) -> Option<(f32, f32)> {
    let mut parts = s.split('-');
    let a: f32 = parts.next()?.parse().ok()?;
    let b: f32 = match parts.next() {
        Some(x) => x.parse().ok()?,
        None => a,
    };
    (parts.next().is_none() && a > 0.0 && b >= a).then_some((a, b))
}

/// What a lens's name says it is, in any maker's style: "Canon EF
/// 100-400mm f/4.5-5.6L IS USM", "XF23mmF1.4 R", "FE 24-70mm F2.8 GM",
/// "24.0-70.0 mm f/2.8". None without both a focal length and an aperture.
fn spec_of(name: &str) -> Option<Spec> {
    let lower = name.to_lowercase();
    let is_num = |c: char| c.is_ascii_digit() || c == '.' || c == '-';
    for (at, _) in lower.match_indices("mm") {
        let head = lower[..at].trim_end();
        let start = head.char_indices().rev().find(|&(_, c)| !is_num(c)).map_or(0, |(i, c)| i + c.len_utf8());
        let Some((short, long)) = num_range(head[start..].trim_matches('-')) else { continue };
        let tail = &lower[at + 2..];
        for (i, c) in tail.char_indices() {
            if c != 'f' {
                continue;
            }
            let rest = tail[i + 1..].trim_start_matches('/');
            let end = rest.find(|c: char| !is_num(c)).unwrap_or(rest.len());
            if let Some((wide_f, tele_f)) = num_range(rest[..end].trim_matches('-')) {
                return Some(Spec { short, long, wide_f, tele_f });
            }
        }
        return None;
    }
    None
}

/// A name that is only what a lens is, no maker or model: "24-70mm f/2.8"
/// (Shoebox's, for an older Canon), "24.0-70.0 mm f/2.8" (Lightroom's).
fn is_spec_only(name: &str) -> bool {
    let s: String = name.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let Some((focal, f)) = s.split_once("mm") else { return false };
    let range = |r: &str| num_range(r).is_some();
    range(focal) && (f.is_empty() || f.strip_prefix('f').map(|x| x.strip_prefix('/').unwrap_or(x)).is_some_and(range))
}

/// Two names of the same lens, as [`tidy`] would group them.
pub fn same_lens(a: &str, b: &str) -> bool {
    let (a, b) = (lens_key(a), lens_key(b));
    a.rest == b.rest && (a.maker == b.maker || a.maker.is_none() || b.maker.is_none())
}

/// The fullest spelling of a group: naming its maker, not in capitals, with
/// spaces; then the most used.
fn best<'a>(names: &[(&'a str, &Key, i64)]) -> &'a str {
    names
        .iter()
        .max_by_key(|(n, k, count)| {
            (
                k.maker.is_some(),
                n.chars().any(|c| c.is_lowercase()),
                n.matches(' ').count(),
                *count,
                std::cmp::Reverse(*n),
            )
        })
        .map(|(n, _, _)| *n)
        .unwrap_or_default()
}

/// Groups the names that are the same gear spelled differently, as the module
/// says. A group the user named shows that name, and new spellings join it; a group
/// they split (several names of theirs) is left alone. Run after anything that
/// brings new names (scans, [`find_lenses`]): one pass over the distinct names.
pub fn tidy(db: &Connection) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    tx.execute("DELETE FROM gear_names WHERE by_hand = 0", [])?;
    for kind in [Kind::Camera, Kind::Lens] {
        // (name as in the files, key, photos)
        let mut names: BTreeMap<String, (Key, i64)> = BTreeMap::new();
        let sql = match kind {
            Kind::Camera => format!(
                "SELECT {CAMERA_FILES_SQL} AS n, p.make, p.model, COUNT(*) FROM photos p WHERE n IS NOT NULL GROUP BY n, p.make, p.model"
            ),
            Kind::Lens => format!(
                "SELECT {LENS_FILES_SQL} AS n, NULL, NULL, COUNT(*) FROM photos p WHERE n IS NOT NULL GROUP BY n"
            ),
        };
        let mut stmt = tx.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let name: String = r.get(0)?;
            let n: i64 = r.get(3)?;
            let key = match kind {
                Kind::Camera => {
                    camera_key(r.get::<_, Option<String>>(1)?.as_deref(), r.get::<_, Option<String>>(2)?.as_deref())
                }
                Kind::Lens => lens_key(&name),
            };
            names.entry(name).and_modify(|e| e.1 += n).or_insert((key, n));
        }
        drop(rows);
        drop(stmt);
        let hand: HashMap<String, String> = tx
            .prepare("SELECT name, shown FROM gear_names WHERE kind = ?1 AND by_hand = 1")?
            .query_map([kind.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        let mut auto: HashMap<&str, String> = HashMap::new();
        let mut groups: BTreeMap<&str, Vec<(&str, &Key, i64)>> = BTreeMap::new();
        for (name, (key, n)) in &names {
            groups.entry(key.rest.as_str()).or_default().push((name, key, *n));
        }
        for group in groups.values().filter(|g| g.len() > 1) {
            // Two makers with one model name: not known to be the same.
            let makers: BTreeSet<&Option<String>> =
                group.iter().map(|(_, k, _)| &k.maker).filter(|m| m.is_some()).collect();
            if makers.len() > 1 {
                continue;
            }
            let named: BTreeSet<&String> = group.iter().filter_map(|(n, _, _)| hand.get(*n)).collect();
            let target = match named.len() {
                0 => best(group).to_string(),
                1 => named.into_iter().next().cloned().unwrap_or_default(),
                _ => continue,
            };
            for (name, _, _) in group {
                if *name != target && !hand.contains_key(*name) {
                    tx.execute(
                        "INSERT OR REPLACE INTO gear_names (kind, name, shown, by_hand) VALUES (?1, ?2, ?3, 0)",
                        params![kind.as_str(), name, target],
                    )?;
                    auto.insert(name, target.clone());
                }
            }
        }
        if kind == Kind::Lens {
            // A lens named only by what it is ("24-70mm f/2.8": an older Canon
            // records no lens id) is the catalog's lens of that focal range and
            // aperture - when exactly one fits. Two that fit (a Canon and a
            // Tamron 24-70 f/2.8): left as it is.
            let shown = |n: &str| hand.get(n).or_else(|| auto.get(n)).cloned().unwrap_or_else(|| n.to_string());
            for name in names.keys().filter(|n| is_spec_only(n) && !hand.contains_key(*n)) {
                let Some(spec) = spec_of(name) else { continue };
                let fits: BTreeSet<String> = names
                    .keys()
                    .filter(|other| !is_spec_only(other) && spec_of(other).is_some_and(|o| o.holds(&spec)))
                    .map(|other| shown(other))
                    .collect();
                if fits.len() == 1
                    && let Some(target) = fits.into_iter().next()
                    && target != *name
                {
                    tx.execute(
                        "INSERT OR REPLACE INTO gear_names (kind, name, shown, by_hand) VALUES (?1, ?2, ?3, 0)",
                        params![kind.as_str(), name, target],
                    )?;
                }
            }
        }
    }
    tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('gear_rules', ?1)", [GEAR_RULES])?;
    tx.commit()?;
    Ok(())
}

/// Groups the catalog's names if it was grouped with older rules (or never).
pub fn tidy_if_new_rules(db: &Connection) -> Result<()> {
    let rules: Option<String> =
        db.query_row("SELECT value FROM meta WHERE key = 'gear_rules'", [], |r| r.get(0)).optional()?;
    if rules.as_deref() != Some(GEAR_RULES) {
        tidy(db)?;
    }
    Ok(())
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GearItem {
    /// As shown everywhere.
    pub name: String,
    pub count: i64,
    /// What the files call it, when that isn't just `name` (several spellings
    /// grouped, or renamed).
    pub in_files: Vec<String>,
    /// Renamed or merged by the user; it can be split back into the files' names.
    pub by_hand: bool,
    /// Photos that don't say ("Unknown lens"): never renamed.
    pub unknown: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Gear {
    pub cameras: Vec<GearItem>,
    pub lenses: Vec<GearItem>,
}

/// Every camera and lens, most used first, the unknowns last.
pub fn list(db: &Connection) -> Result<Gear> {
    let items = |kind: Kind| -> Result<Vec<GearItem>> {
        let files = kind.files_sql();
        let sql = format!(
            "SELECT {} AS s, {files} AS f, COUNT(*),
                    EXISTS (SELECT 1 FROM gear_names g WHERE g.kind = ?1 AND g.name = {files} AND g.by_hand = 1)
               FROM photos p GROUP BY s, f",
            kind.shown_sql()
        );
        let mut out: Vec<GearItem> = Vec::new();
        let mut stmt = db.prepare(&sql)?;
        let mut rows = stmt.query([kind.as_str()])?;
        while let Some(r) = rows.next()? {
            let (name, file, n, hand): (String, Option<String>, i64, bool) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let i = match out.iter().position(|x| x.name == name) {
                Some(i) => i,
                None => {
                    let unknown = name == kind.unknown();
                    out.push(GearItem { name, count: 0, in_files: Vec::new(), by_hand: false, unknown });
                    out.len() - 1
                }
            };
            let item = &mut out[i];
            item.count += n;
            item.by_hand |= hand;
            item.in_files.extend(file);
        }
        for item in &mut out {
            item.in_files.sort_by_key(|f| f.to_lowercase());
            if item.in_files.len() == 1 && item.in_files[0] == item.name {
                item.in_files.clear();
            }
        }
        out.sort_by(|a, b| a.unknown.cmp(&b.unknown).then(b.count.cmp(&a.count)).then(a.name.cmp(&b.name)));
        Ok(out)
    };
    Ok(Gear { cameras: items(Kind::Camera)?, lenses: items(Kind::Lens)? })
}

/// A name as typed: trimmed, inner spaces collapsed.
fn clean(name: &str) -> Option<String> {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!n.is_empty() && n.chars().count() <= 200).then_some(n)
}

/// The names in the files of what shows as `shown`, and every name shown.
fn names_of(tx: &Connection, kind: Kind, shown: &str) -> Result<(Vec<String>, Vec<String>)> {
    let pairs: Vec<(String, Option<String>)> = tx
        .prepare(&format!("SELECT DISTINCT {}, {} FROM photos p", kind.shown_sql(), kind.files_sql()))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut files: BTreeSet<String> = pairs.iter().filter(|(s, _)| s == shown).filter_map(|(_, f)| f.clone()).collect();
    // Names of the group no photo has at the moment (all on a removed folder) stay with it.
    let kept: Vec<String> = tx
        .prepare("SELECT name FROM gear_names WHERE kind = ?1 AND shown = ?2")?
        .query_map(params![kind.as_str(), shown], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    files.extend(kept);
    let all: BTreeSet<String> = pairs.into_iter().map(|(s, _)| s).collect();
    Ok((files.into_iter().collect(), all.into_iter().collect()))
}

/// Renames a camera or lens everywhere it shows, and for every photo to come
/// with the same names in its files. Onto a name already shown (any case) =
/// merge; that name keeps its spelling. Returns the name it now goes by.
pub fn rename(db: &Connection, kind: Kind, from: &str, to: &str) -> Result<String> {
    let Some(to) = clean(to) else { bail!("A name can't be empty.") };
    if from == kind.unknown() || to == kind.unknown() {
        bail!("{} is for photos that don't say; it can't be renamed.", kind.unknown());
    }
    let tx = db.unchecked_transaction()?;
    let (files, shown) = names_of(&tx, kind, from)?;
    if files.is_empty() {
        bail!("No {} is called {from:?}.", kind.as_str());
    }
    let to = shown.into_iter().find(|s| s != from && s.eq_ignore_ascii_case(&to) && s != kind.unknown()).unwrap_or(to);
    for f in &files {
        tx.execute(
            "INSERT OR REPLACE INTO gear_names (kind, name, shown, by_hand) VALUES (?1, ?2, ?3, 1)",
            params![kind.as_str(), f, to],
        )?;
    }
    tx.commit()?;
    Ok(to)
}

/// Splits a camera or lens back into the names its files give, each kept
/// apart from now on (also undoes a rename). Returns those names.
pub fn split(db: &Connection, kind: Kind, name: &str) -> Result<Vec<String>> {
    let tx = db.unchecked_transaction()?;
    let (files, _) = names_of(&tx, kind, name)?;
    if files.is_empty() {
        bail!("No {} is called {name:?}.", kind.as_str());
    }
    for f in &files {
        tx.execute(
            "INSERT OR REPLACE INTO gear_names (kind, name, shown, by_hand) VALUES (?1, ?2, ?2, 1)",
            params![kind.as_str(), f],
        )?;
    }
    tx.commit()?;
    Ok(files)
}

/// Photos without a lens take it from the same shot's other file (a RAW's
/// JPEG, or the other way): same camera, same second, from the camera's
/// clock, and only when those name one lens. Catalog only. Returns how many.
pub fn borrow_from_twins(db: &Connection) -> Result<usize> {
    let found: Vec<(i64, String)> = db
        .prepare(&format!(
            "WITH shot AS (SELECT p.id, p.taken_ts AS t, {CAMERA_SQL} AS cam, p.lens FROM photos p
                            WHERE p.taken_exif = 1 AND p.taken_ts > 0)
             SELECT a.id, MIN(b.lens) FROM shot a JOIN shot b ON b.t = a.t AND b.cam = a.cam AND b.id <> a.id
              WHERE a.lens IS NULL AND b.lens IS NOT NULL AND a.cam <> 'Unknown camera'
              GROUP BY a.id HAVING COUNT(DISTINCT b.lens) = 1"
        ))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let tx = db.unchecked_transaction()?;
    for (id, lens) in &found {
        tx.execute("UPDATE photos SET lens = ?1 WHERE id = ?2 AND lens IS NULL", params![lens, id])?;
    }
    tx.commit()?;
    Ok(found.len())
}

/// After a scan: new names grouped, and new photos without a lens given their
/// twin's. Catalog only, no file read.
pub fn after_scan(db: &Connection) -> Result<()> {
    tidy(db)?;
    if borrow_from_twins(db)? > 0 {
        tidy(db)?;
    }
    Ok(())
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LensReport {
    /// Originals read again (metadata only).
    pub read: usize,
    /// Of those, lenses found.
    pub found: usize,
    /// Taken from the same shot's other file.
    pub from_twins: usize,
    /// Still to read: their disk isn't plugged in.
    pub waiting: usize,
}

/// Photos without a lens that an older Shoebox read: their originals are read
/// again with what Shoebox reads now - metadata only, never the picture, and
/// only from disks plugged in (the rest wait for theirs) - then the twins
/// lend theirs. Each photo is read once per [`imaging::LENS_READER`].
pub fn find_lenses(cat: &Catalog, cancel: &AtomicBool) -> Result<LensReport> {
    let pending: Vec<i64> = cat
        .db()
        .prepare("SELECT id FROM photos WHERE lens IS NULL AND lens_read < ?1 ORDER BY id")?
        .query_map([imaging::LENS_READER], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut report = LensReport::default();
    for id in pending {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok((path, kind)) = cat.original(id) else {
            report.waiting += 1;
            continue;
        };
        let lens = imaging::read_lens(&path, kind);
        report.read += 1;
        report.found += lens.is_some() as usize;
        cat.db().execute(
            "UPDATE photos SET lens = COALESCE(lens, ?1), lens_read = ?2 WHERE id = ?3",
            params![lens, imaging::LENS_READER, id],
        )?;
    }
    let db = cat.db();
    tidy(&db)?;
    report.from_twins = borrow_from_twins(&db)?;
    if report.found + report.from_twins > 0 {
        tidy(&db)?;
    }
    if report.read + report.from_twins > 0 {
        log::info!("lenses: {report:?}");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_say_something() {
        assert_eq!(file_name("  EF70-200mm   f/4L IS USM\0").as_deref(), Some("EF70-200mm f/4L IS USM"));
        assert_eq!(file_name("----"), None);
        assert_eq!(file_name("0.0 mm f/0.0"), None);
        assert_eq!(file_name(""), None);
    }

    #[test]
    fn the_same_lens_however_it_is_spelled() {
        assert!(same_lens("EF70-200mm f/4L IS USM", "Canon EF 70-200mm f/4L IS USM"));
        assert!(same_lens("RF24-105mm F4 L IS USM", "Canon RF 24-105mm F4L IS USM"));
        assert!(same_lens("XF23mmF1.4 R", "Fujifilm XF 23mm F1.4 R"));
        assert!(same_lens("XF23mmF1.4 R", "Fuji XF 23mm f/1.4 R"));
        assert!(same_lens("FE 24-70mm F2.8 GM", "Sony FE 24-70mm F2.8 GM"));
        assert!(same_lens("NIKKOR Z 24-70mm f/4 S", "Nikon NIKKOR Z 24-70mm f/4 S"));
        assert!(same_lens("24.0-70.0 mm f/4.0", "24-70mm F4"));
        assert!(!same_lens("EF70-200mm f/4L IS USM", "EF70-200mm f/4L USM"));
        assert!(!same_lens("EF 50mm f/1.4 USM", "EF 50mm f/1.8 STM"));
        assert!(!same_lens("4.05mm f/2", "4mm f/2"));
        // Two makers, one model name: not the same lens.
        assert!(!same_lens("Sigma 50mm F1.4", "Tamron 50mm F1.4"));
    }

    #[test]
    fn the_same_camera_whatever_wrote_the_file() {
        let k = |make, model| camera_key(Some(make), Some(model));
        assert_eq!(k("NIKON CORPORATION", "NIKON D850"), k("Nikon", "D850"));
        assert_eq!(k("Canon", "Canon EOS 5D Mark II"), k("Canon", "EOS 5D Mark II"));
        assert_eq!(k("OLYMPUS IMAGING CORP.", "E-M1"), k("Olympus", "E-M1"));
        assert_eq!(k("FUJIFILM", "X-T4"), k("Fujifilm", "X-T4"));
        assert_ne!(k("Canon", "EOS 5D Mark II"), k("Canon", "EOS 5D Mark III"));
        assert_ne!(k("Nikon", "Z 6"), k("Canon", "Z 6"));
    }

    fn catalog() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE photos (id INTEGER PRIMARY KEY, make TEXT, model TEXT, lens TEXT, taken_ts INTEGER, taken_exif INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE gear_names (kind TEXT NOT NULL, name TEXT NOT NULL, shown TEXT NOT NULL, by_hand INTEGER NOT NULL,
                                      PRIMARY KEY (kind, name)) WITHOUT ROWID;
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        db
    }

    fn add(db: &Connection, make: &str, model: &str, lens: Option<&str>, n: usize) {
        for _ in 0..n {
            db.execute("INSERT INTO photos (make, model, lens) VALUES (?1, ?2, ?3)", params![make, model, lens])
                .unwrap();
        }
    }

    fn shown(g: &[GearItem]) -> Vec<(&str, i64)> {
        g.iter().map(|i| (i.name.as_str(), i.count)).collect()
    }

    #[test]
    fn spellings_group_and_names_stick() {
        let db = catalog();
        add(&db, "Canon", "EOS 5D Mark II", Some("Canon EF 70-200mm f/4L IS USM"), 3);
        add(&db, "Canon", "Canon EOS 5D Mark II", Some("EF70-200mm f/4L IS USM"), 2);
        add(&db, "NIKON CORPORATION", "NIKON D850", Some("Sigma 35mm F1.4 DG HSM | A"), 1);
        add(&db, "Nikon", "D850", None, 1);
        tidy(&db).unwrap();
        let g = list(&db).unwrap();
        assert_eq!(
            shown(&g.lenses),
            [("Canon EF 70-200mm f/4L IS USM", 5), ("Sigma 35mm F1.4 DG HSM | A", 1), ("Unknown lens", 1)]
        );
        assert_eq!(shown(&g.cameras), [("Canon EOS 5D Mark II", 5), ("Nikon D850", 2)]);
        assert_eq!(g.lenses[0].in_files, ["Canon EF 70-200mm f/4L IS USM", "EF70-200mm f/4L IS USM"]);

        // The user's name wins, and a spelling seen later joins it.
        assert_eq!(rename(&db, Kind::Lens, "Canon EF 70-200mm f/4L IS USM", "70-200 f/4").unwrap(), "70-200 f/4");
        add(&db, "Canon", "EOS R6 Mark III", Some("EF 70-200mm F4L IS USM"), 1);
        tidy(&db).unwrap();
        assert_eq!(shown(&list(&db).unwrap().lenses)[0], ("70-200 f/4", 6));

        // Renaming onto a name shown (any case) merges, keeping its spelling.
        assert_eq!(rename(&db, Kind::Lens, "Sigma 35mm F1.4 DG HSM | A", "70-200 F/4").unwrap(), "70-200 f/4");
        assert_eq!(shown(&list(&db).unwrap().lenses)[0], ("70-200 f/4", 7));

        // Split: back to the files' names, kept apart from now on.
        split(&db, Kind::Lens, "70-200 f/4").unwrap();
        tidy(&db).unwrap();
        assert_eq!(list(&db).unwrap().lenses.len(), 5);
        assert!(rename(&db, Kind::Lens, "Unknown lens", "50mm").is_err());
        assert!(rename(&db, Kind::Lens, "Nothing like it", "50mm").is_err());
    }

    #[test]
    fn a_lens_by_what_it_is() {
        assert_eq!(spec_name(24.0, 70.0, Some((2.83, 2.83))).as_deref(), Some("24-70mm f/2.8"));
        assert_eq!(spec_name(50.0, 50.0, Some((1.78, 1.78))).as_deref(), Some("50mm f/1.8"));
        assert_eq!(spec_name(12.0, 24.0, Some((4.0, 4.0))).as_deref(), Some("12-24mm f/4"));
        assert_eq!(spec_name(100.0, 400.0, Some((5.66, 5.66))).as_deref(), Some("100-400mm f/5.6"));
        assert_eq!(spec_name(18.0, 55.0, Some((3.5, 5.6))).as_deref(), Some("18-55mm f/3.5-5.6"));
        assert_eq!(spec_name(50.0, 50.0, None).as_deref(), Some("50mm"));
        assert_eq!(spec_name(0.0, 0.0, Some((2.8, 2.8))), None);

        let spec = |n: &str| spec_of(n).map(|s| (s.short, s.long, s.wide_f, s.tele_f));
        assert_eq!(spec("Canon EF 24-70mm f/2.8L USM"), Some((24.0, 70.0, 2.8, 2.8)));
        assert_eq!(spec("EF100-400mm f/4.5-5.6L IS USM"), Some((100.0, 400.0, 4.5, 5.6)));
        assert_eq!(spec("EF-S18-55mm f/3.5-5.6 IS STM"), Some((18.0, 55.0, 3.5, 5.6)));
        assert_eq!(spec("XF23mmF1.4 R"), Some((23.0, 23.0, 1.4, 1.4)));
        assert_eq!(spec("Sony FE 24-70mm F2.8 GM"), Some((24.0, 70.0, 2.8, 2.8)));
        assert_eq!(spec("Leica Summilux-M 50mm f/1.4 ASPH."), Some((50.0, 50.0, 1.4, 1.4)));
        assert_eq!(spec("24.0-70.0 mm f/2.8"), Some((24.0, 70.0, 2.8, 2.8)));
        assert_eq!(spec("iPhone 13 Pro back camera"), None);

        assert!(is_spec_only("24-70mm f/2.8"));
        assert!(is_spec_only("24.0-70.0 mm f/2.8"));
        assert!(is_spec_only("50mm"));
        assert!(!is_spec_only("Canon EF 50mm f/1.4 USM"));
        assert!(!is_spec_only("EF50mm f/1.8 II"));
    }

    #[test]
    fn a_lens_by_what_it_is_joins_the_one_lens_that_fits() {
        let db = catalog();
        add(&db, "Canon", "Canon EOS 5D Mark II", Some("Canon EF 24-70mm f/2.8L USM"), 2);
        add(&db, "Canon", "Canon EOS 5D Mark II", Some("Canon EF 50mm f/1.4 USM"), 1);
        add(&db, "Canon", "Canon EOS 5D Mark II", Some("EF100-400mm f/4.5-5.6L IS USM"), 1);
        add(&db, "Canon", "EOS 10D", Some("24-70mm f/2.8"), 3);
        add(&db, "Canon", "EOS 10D", Some("50mm f/1.8"), 1); // not the f/1.4
        add(&db, "Canon", "EOS 10D", Some("12-24mm f/4"), 1); // no such lens here
        add(&db, "Canon", "EOS 10D", Some("100-400mm f/5.6"), 1); // a zoom, at its long end
        add(&db, "Nikon", "D700", Some("100-400mm f/4.5-5.6"), 1); // the same, both ends given
        add(&db, "Nikon", "D700", Some("100-400mm f/4-5.6"), 1); // not the same
        tidy(&db).unwrap();
        let g = list(&db).unwrap();
        assert_eq!(
            shown(&g.lenses),
            [
                ("Canon EF 24-70mm f/2.8L USM", 5),
                ("EF100-400mm f/4.5-5.6L IS USM", 3),
                ("100-400mm f/4-5.6", 1),
                ("12-24mm f/4", 1),
                ("50mm f/1.8", 1),
                ("Canon EF 50mm f/1.4 USM", 1)
            ]
        );
        assert_eq!(g.lenses[0].in_files, ["24-70mm f/2.8", "Canon EF 24-70mm f/2.8L USM"]);

        // The user's name for the lens: it joins that.
        rename(&db, Kind::Lens, "Canon EF 24-70mm f/2.8L USM", "My 24-70").unwrap();
        tidy(&db).unwrap();
        assert_eq!(shown(&list(&db).unwrap().lenses)[0], ("My 24-70", 5));

        // Two lenses fit (a Canon and a Tamron 24-70 f/2.8): no guess.
        let db = catalog();
        add(&db, "Canon", "Canon EOS 5D Mark II", Some("Canon EF 24-70mm f/2.8L USM"), 2);
        add(&db, "Canon", "Canon EOS 5D Mark II", Some("Tamron SP 24-70mm F/2.8 Di VC USD"), 1);
        add(&db, "Canon", "EOS 10D", Some("24-70mm f/2.8"), 3);
        tidy(&db).unwrap();
        assert_eq!(
            shown(&list(&db).unwrap().lenses),
            [("24-70mm f/2.8", 3), ("Canon EF 24-70mm f/2.8L USM", 2), ("Tamron SP 24-70mm F/2.8 Di VC USD", 1)]
        );
    }

    #[test]
    fn twins_lend_their_lens() {
        let db = catalog();
        let shot = |model: &str, lens: Option<&str>, ts: i64| {
            db.execute(
                "INSERT INTO photos (make, model, lens, taken_ts, taken_exif) VALUES ('Canon', ?1, ?2, ?3, 1)",
                params![model, lens, ts],
            )
            .unwrap();
        };
        shot("EOS 5D Mark II", None, 100); // the RAW
        shot("Canon EOS 5D Mark II", Some("EF135mm f/2L USM"), 100); // its JPEG
        shot("EOS 5D Mark II", None, 200); // two lenses that second: left alone
        shot("Canon EOS 5D Mark II", Some("EF135mm f/2L USM"), 200);
        shot("Canon EOS 5D Mark II", Some("EF85mm f/1.8 USM"), 200);
        shot("EOS R6 Mark III", None, 300); // another camera's photo: not a twin
        shot("Canon EOS 5D Mark II", Some("EF85mm f/1.8 USM"), 300);
        tidy(&db).unwrap();
        assert_eq!(borrow_from_twins(&db).unwrap(), 1);
        let lens: Option<String> = db.query_row("SELECT lens FROM photos WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(lens.as_deref(), Some("EF135mm f/2L USM"));
    }
}
