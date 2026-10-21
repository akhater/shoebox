//! A source whose photos live somewhere else now: the same folder on another disk. A new computer (its system
//! disk is another disk, even where the folder's path is the same), a disk copied onto a bigger one, a folder
//! copied to another disk or renamed outside Shoebox. Disks are known by their serial, never by a letter or a
//! mount point (promise 4), so the catalog can't tell by itself: the user points the source there.
//!
//! Repointing changes the catalog only, in place: the source keeps its id and its photos, with their tags,
//! faces and people; its folders take their places on the new disk. No file is touched. The new place is
//! checked first: a sample of the source's photos, spread over all of it, is looked for there by content (size
//! and fingerprint, as a scan reads them). A scan of the source then settles the rest: a file that isn't there
//! becomes missing only after that complete scan (promise 3), and stays in the catalog.
//!
//! Shoebox also looks for each source that isn't here at the same path on the disks plugged in now
//! ([`look_elsewhere`]: a new computer's system disk, a copied disk) and offers it. It never repoints by
//! itself: a backup kept at the same path on another disk looks just the same.
//!
//! Paths are compared below each disk's mount point, in the catalog's form (names joined by [`volumes::SEP`]),
//! so this works the same on Windows, macOS and Linux, and from one system to another.

use crate::catalog::{self, Catalog, RootRow};
use crate::db::{self, FILE_PRESENT, under_sql};
use crate::organize;
use crate::scan;
use crate::volumes::{self, MountedVolume, SEP};
use anyhow::{Result, anyhow, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

/// Photos looked for in a folder the user picked.
const SAMPLE: usize = 40;
/// Photos looked for at the same path on each disk plugged in, for every source that isn't here: few, as it
/// runs whenever disks come and go.
const SAMPLE_ELSEWHERE: usize = 12;

/// What pointing a source at a folder would find there.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Repoint {
    pub root_id: i64,
    /// The folder, as the system writes it.
    pub path: String,
    /// The disk holding it, by the name Shoebox gives it.
    pub disk: String,
    /// The source's photos looked for there: a sample spread over all of it.
    pub checked: usize,
    /// Of those, the ones there, unchanged.
    pub found: usize,
    /// Why the source can't be pointed there; none when it can.
    pub problem: Option<String>,
}

/// A folder a source could be pointed at.
#[derive(Clone)]
struct Place {
    /// Its disk, when the catalog knows it already.
    volume_id: Option<i64>,
    /// Its disk as the system reports it, for one the catalog doesn't know yet.
    disk: Option<MountedVolume>,
    mount: String,
    rel: String,
    label: String,
}

/// One of a source's files, to look for elsewhere.
struct Sampled {
    folder: String,
    name: String,
    size: u64,
    fingerprint: String,
}

/// What pointing the source `root_id` at the folder `path` would find there. Reads only.
pub fn check(cat: &Catalog, root_id: i64, path: &Path) -> Result<Repoint> {
    let root = cat.root(root_id)?;
    check_place(cat, &root, &place_of(cat, path)?, SAMPLE)
}

/// Points the source `root_id` at the folder `path`, in the catalog only, once [`check`] finds its photos
/// there. Scanning the source next settles the rest.
pub fn repoint(cat: &Catalog, root_id: i64, path: &Path) -> Result<Repoint> {
    let root = cat.root(root_id)?;
    apply(cat, &root, &place_of(cat, path)?)
}

/// [`check`] for the folder `rel` of a disk the catalog knows, plugged in now.
pub fn check_on(cat: &Catalog, root_id: i64, volume_id: i64, rel: &str) -> Result<Repoint> {
    let root = cat.root(root_id)?;
    check_place(cat, &root, &place_on(cat, volume_id, rel)?, SAMPLE)
}

/// [`repoint`] onto the folder `rel` of a disk the catalog knows, plugged in now.
pub fn repoint_on(cat: &Catalog, root_id: i64, volume_id: i64, rel: &str) -> Result<Repoint> {
    let root = cat.root(root_id)?;
    apply(cat, &root, &place_on(cat, volume_id, rel)?)
}

/// Looks for every source that isn't here (its disk unplugged, or its folder not found on it) at the same path
/// on each disk plugged in now, known to the catalog or not, and keeps where nearly all the photos looked for
/// are there, unchanged ([`Catalog::found_elsewhere`]). Reads a few photos per place, and only where the
/// folder exists. Returns true if what was found changed.
pub fn look_elsewhere(cat: &Catalog) -> Result<bool> {
    let away: Vec<RootRow> = cat
        .root_ids()?
        .into_iter()
        .filter_map(|id| cat.root(id).ok())
        .filter(|r| !cat.root_is_here(r.volume_id, &r.rel_path))
        .collect();
    let mut found = HashMap::new();
    if !away.is_empty() {
        let disks = disks_plugged_in(cat)?;
        for root in &away {
            for disk in disks.iter().filter(|d| d.volume_id != Some(root.volume_id)) {
                let to = Place { rel: root.rel_path.clone(), ..disk.clone() };
                if !Path::new(&volumes::join(&to.mount, &to.rel)).is_dir() {
                    continue;
                }
                let c = check_place(cat, root, &to, SAMPLE_ELSEWHERE)?;
                if c.problem.is_none() && c.checked > 0 && c.found * 10 >= c.checked * 9 {
                    found.insert(root.id, c);
                    break;
                }
            }
        }
    }
    Ok(cat.set_found_elsewhere(found))
}

/// The folder `path` and its disk.
fn place_of(cat: &Catalog, path: &Path) -> Result<Place> {
    if !path.is_dir() {
        bail!("{} is not a folder", path.display());
    }
    catalog::ensure_keepable(path)?;
    let (mv, rel) = volumes::locate(path)?;
    let known = {
        let db = cat.read()?;
        catalog::find_volume(&db, &mv)?
    };
    Ok(match known {
        Some(id) => Place { rel, ..place_on(cat, id, "")? },
        None => Place {
            volume_id: None,
            label: catalog::new_volume_label(&mv),
            mount: mv.mount.clone(),
            disk: Some(mv),
            rel,
        },
    })
}

/// The folder `rel` of a known disk, plugged in now.
fn place_on(cat: &Catalog, volume_id: i64, rel: &str) -> Result<Place> {
    let mount = cat.mount_of(volume_id).ok_or_else(|| anyhow!("That disk is not plugged in"))?;
    let label: String = cat.read()?.query_row("SELECT label FROM volumes WHERE id = ?1", [volume_id], |r| r.get(0))?;
    Ok(Place { volume_id: Some(volume_id), disk: None, mount, rel: rel.to_string(), label })
}

/// Every disk plugged in now, at the top: the known ones where the catalog sees them, then the others,
/// each at its first mount point.
fn disks_plugged_in(cat: &Catalog) -> Result<Vec<Place>> {
    let mut out = Vec::new();
    let mut known: Vec<(i64, String)> = cat.online_mounts().into_iter().collect();
    known.sort();
    for (id, _) in known {
        out.push(place_on(cat, id, "")?);
    }
    let db = cat.read()?;
    let mut seen: Vec<MountedVolume> = Vec::new();
    for mv in volumes::mounted() {
        let twice = seen.iter().any(|s| s.serial == mv.serial && s.fs == mv.fs && s.size == mv.size);
        if twice || catalog::find_volume(&db, &mv)?.is_some() {
            continue;
        }
        seen.push(mv.clone());
        out.push(Place {
            volume_id: None,
            label: catalog::new_volume_label(&mv),
            mount: mv.mount.clone(),
            disk: Some(mv),
            rel: String::new(),
        });
    }
    Ok(out)
}

/// What pointing `root` at `to` would find there, looking for up to `n` of its photos.
fn check_place(cat: &Catalog, root: &RootRow, to: &Place, n: usize) -> Result<Repoint> {
    let path = volumes::join(&to.mount, &to.rel);
    let mut out =
        Repoint { root_id: root.id, path: path.clone(), disk: to.label.clone(), checked: 0, found: 0, problem: None };
    out.problem = refusal(cat, root, to, &path)?;
    if out.problem.is_some() {
        return Ok(out);
    }
    let sample = sample(cat, root, n)?;
    out.checked = sample.len();
    out.found = sample.iter().filter(|s| is_there(root, to, s)).count();
    if out.checked > 0 && out.found == 0 {
        out.problem = Some(format!(
            "None of the {} photos of {} looked for are in {path}, at the same place: it isn't the same folder. If \
             its photos were reorganized, add {path} as a new folder instead (Shoebox recognises them by their \
             content, with their tags and faces), then remove {} from the catalog.",
            out.checked,
            name_of(root),
            name_of(root)
        ));
    }
    Ok(out)
}

/// Why `root` can't be pointed at `to` (written `path`), if it can't.
fn refusal(cat: &Catalog, root: &RootRow, to: &Place, path: &str) -> Result<Option<String>> {
    let overlap = |a: &str, b: &str| db::is_under(a, b) || db::is_under(b, a);
    let name = name_of(root);
    if to.volume_id == Some(root.volume_id) && overlap(&to.rel, &root.rel_path) {
        return Ok(Some(if to.rel == root.rel_path {
            format!("{name} is already there.")
        } else {
            format!("{path} is inside {name}, or holds it: choose a folder outside it.")
        }));
    }
    if let Some(vid) = to.volume_id {
        for other in cat.root_ids()?.into_iter().filter(|id| *id != root.id).filter_map(|id| cat.root(id).ok()) {
            if other.volume_id != vid || !overlap(&to.rel, &other.rel_path) {
                continue;
            }
            let theirs = volumes::join(&to.mount, &other.rel_path);
            return Ok(Some(if other.rel_path == to.rel {
                format!(
                    "{path} is already in the catalog, as a folder of its own. Remove {name} from the catalog \
                     instead: its photos that are in {path} stay, with their tags and faces."
                )
            } else if db::is_under(&to.rel, &other.rel_path) {
                format!("{path} is already in the catalog, inside {theirs}.")
            } else {
                format!("{path} holds {theirs}, already in the catalog.")
            }));
        }
        let remembered: bool = cat.read()?.query_row(
            &format!("SELECT EXISTS (SELECT 1 FROM folders fo WHERE fo.volume_id = ?1 AND {})", under_sql("fo", 2)),
            params![vid, to.rel],
            |r| r.get(0),
        )?;
        if remembered {
            return Ok(Some(format!("The catalog still remembers folders in {path}: choose another folder.")));
        }
    }
    let busy = organize::busy_folders(cat)?.into_iter().any(|(v, b)| {
        (v == root.volume_id && overlap(&b, &root.rel_path)) || (Some(v) == to.volume_id && overlap(&b, &to.rel))
    });
    if busy {
        return Ok(Some(format!(
            "Shoebox hasn't finished an earlier move, rename or delete in {name} yet: it finishes once the disks it \
             needs are plugged in."
        )));
    }
    Ok(None)
}

/// Up to `n` of the source's files that are there (not missing), spread evenly over all of them.
fn sample(cat: &Catalog, root: &RootRow, n: usize) -> Result<Vec<Sampled>> {
    let db = cat.read()?;
    let all: Vec<Sampled> = db
        .prepare(&format!(
            "SELECT fo.rel_path, f.name, f.size, p.fingerprint FROM files f
               JOIN folders fo ON fo.id = f.folder_id JOIN photos p ON p.id = f.photo_id
              WHERE fo.volume_id = ?1 AND {} AND f.state = {FILE_PRESENT} ORDER BY f.id",
            under_sql("fo", 2)
        ))?
        .query_map(params![root.volume_id, root.rel_path], |r| {
            Ok(Sampled { folder: r.get(0)?, name: r.get(1)?, size: r.get::<_, i64>(2)? as u64, fingerprint: r.get(3)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let len = all.len();
    if len <= n {
        return Ok(all);
    }
    // The k-th of n is the file at k * len / n: increasing, as len > n.
    let mut k = 0;
    Ok(all
        .into_iter()
        .enumerate()
        .filter(|(i, _)| {
            let take = k < n && *i == k * len / n;
            k += take as usize;
            take
        })
        .map(|(_, s)| s)
        .collect())
}

/// Whether the sampled file is at its place in `to`, with the same content.
fn is_there(root: &RootRow, to: &Place, s: &Sampled) -> bool {
    let file = volumes::file_path(&to.mount, &moved(&root.rel_path, &to.rel, &s.folder), &s.name);
    let file = Path::new(&file);
    std::fs::metadata(file).is_ok_and(|m| m.is_file() && m.len() == s.size)
        && scan::fingerprint(file, s.size).is_ok_and(|f| f == s.fingerprint)
}

/// Where the folder `rel`, inside a source at `from`, is once the source is at `to` (catalog form).
fn moved(from: &str, to: &str, rel: &str) -> String {
    let inside = rel.get(from.len()..).unwrap_or("").trim_start_matches(SEP);
    match (to.is_empty(), inside.is_empty()) {
        (_, true) => to.to_string(),
        (true, false) => inside.to_string(),
        (false, false) => format!("{to}{SEP}{inside}"),
    }
}

/// A source as the window names it.
fn name_of(root: &RootRow) -> String {
    root.rel_path.rsplit(SEP).next().filter(|s| !s.is_empty()).unwrap_or(&root.volume_label).to_string()
}

/// Points `root` at `to` once its photos are found there.
fn apply(cat: &Catalog, root: &RootRow, to: &Place) -> Result<Repoint> {
    let check = check_place(cat, root, to, SAMPLE)?;
    if let Some(problem) = &check.problem {
        bail!("{problem} Nothing was changed.");
    }
    let vid = {
        let mut db = cat.db();
        let tx = db.transaction()?;
        let vid = match (to.volume_id, &to.disk) {
            (Some(id), _) => id,
            (None, Some(mv)) => catalog::upsert_volume(&tx, mv)?,
            (None, None) => bail!("Shoebox can't recognise the disk holding {}", check.path),
        };
        let folders: Vec<(i64, String)> = tx
            .prepare(&format!(
                "SELECT fo.id, fo.rel_path FROM folders fo WHERE fo.volume_id = ?1 AND {}",
                under_sql("fo", 2)
            ))?
            .query_map(params![root.volume_id, root.rel_path], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, rel) in &folders {
            tx.execute(
                "UPDATE folders SET volume_id = ?1, rel_path = ?2 WHERE id = ?3",
                params![vid, moved(&root.rel_path, &to.rel, rel), id],
            )?;
        }
        let left = tx
            .query_row("SELECT id FROM roots WHERE volume_id = ?1 AND rel_path = ?2", params![vid, to.rel], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?;
        if left.is_some() {
            bail!("{} is already in the catalog. Nothing was changed.", check.path);
        }
        tx.execute("UPDATE roots SET volume_id = ?1, rel_path = ?2 WHERE id = ?3", params![vid, to.rel, root.id])?;
        tx.commit()?;
        vid
    };
    cat.note_plugged_in(vid, &to.mount);
    let _ = cat.refresh_roots();
    let mut found = cat.found_elsewhere();
    found.remove(&root.id);
    cat.set_found_elsewhere(found);
    log::info!(
        "source {} repointed to {} ({} of {} photos looked for found there)",
        root.id,
        check.path,
        check.found,
        check.checked
    );
    Ok(check)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_inside_a_source_moves_with_it() {
        assert_eq!(
            moved("Users\\ak\\Pictures", "Users\\ak\\Pictures", "Users\\ak\\Pictures\\2021"),
            "Users\\ak\\Pictures\\2021"
        );
        assert_eq!(moved("Users\\ak\\Pictures", "Photos", "Users\\ak\\Pictures"), "Photos");
        assert_eq!(moved("Users\\ak\\Pictures", "Photos", "Users\\ak\\Pictures\\2021\\May"), "Photos\\2021\\May");
        // A whole disk as the source, or as where it goes.
        assert_eq!(moved("", "Photos", "2021\\May"), "Photos\\2021\\May");
        assert_eq!(moved("", "Photos", ""), "Photos");
        assert_eq!(moved("Photos", "", "Photos\\2021"), "2021");
        assert_eq!(moved("Photos", "", "Photos"), "");
        assert_eq!(moved("", "", "2021"), "2021");
    }
}
