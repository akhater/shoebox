//! The system's bin - the Recycle Bin on Windows, the Trash on macOS and
//! Linux - looked into, never changed. It tells a missing photo that was
//! deleted (its file sits in the bin) from one that was moved or renamed.
//! Restored from the bin, a file is back where it was and the next scan finds
//! it with its faces and tags; removed from the catalog, it would come back as
//! a new photo.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the system calls its bin.
pub const NAME: &str = if cfg!(windows) { "Recycle Bin" } else { "Trash" };

/// The files in the bin, by where they were before.
#[derive(Default)]
pub struct Bin {
    /// Full path the file had (lower case on Windows) -> when it went in
    /// (seconds since 1970; 0 = unknown).
    by_path: HashMap<String, i64>,
    /// File name and size -> when it went in: macOS's Trash keeps where a
    /// file came from only in Finder's own records.
    by_name: HashMap<(String, u64), i64>,
}

impl Bin {
    /// When the file that was at `path` (`size` bytes) went to the bin, if it
    /// is there (0 = it is, time unknown).
    pub fn deleted(&self, path: &Path, size: u64) -> Option<i64> {
        if let Some(at) = self.by_path.get(&path_key(path)) {
            return Some(*at);
        }
        let name = path.file_name()?.to_string_lossy().into_owned();
        self.by_name.get(&(name, size)).copied()
    }

    #[cfg(test)]
    fn with(paths: &[(&str, i64)], names: &[(&str, u64, i64)]) -> Bin {
        Bin {
            by_path: paths.iter().map(|(p, at)| (path_key(Path::new(p)), *at)).collect(),
            by_name: names.iter().map(|(n, size, at)| ((n.to_string(), *size), *at)).collect(),
        }
    }
}

fn path_key(path: &Path) -> String {
    let p = path.to_string_lossy();
    if cfg!(windows) { p.to_lowercase() } else { p.into_owned() }
}

/// The bin as it is now, for files that were on the disks mounted at
/// `mounts`; `None` when it can't be read (macOS asks for Full Disk Access to
/// look into the Trash). Read at most every few seconds for the same disks:
/// the viewer asks for each missing photo it steps through.
pub fn read(mounts: &[String]) -> Option<Arc<Bin>> {
    type Kept = (Instant, Vec<String>, Arc<Bin>);
    static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, disks, bin)) = kept.as_ref()
        && at.elapsed() < Duration::from_secs(5)
        && disks == mounts
    {
        return Some(bin.clone());
    }
    let bin = Arc::new(read_now(mounts)?);
    *kept = Some((Instant::now(), mounts.to_vec(), bin.clone()));
    Some(bin)
}

/// Windows: each disk keeps its own bin, `$Recycle.Bin\<user>\`, where every
/// deleted file has a small `$I` file saying where it was and when it went.
/// Read directly: the shell's view of the bin asks each photo's own handler
/// for its details, seconds for a few hundred RAW files.
#[cfg(windows)]
fn read_now(mounts: &[String]) -> Option<Bin> {
    let mut bin = Bin::default();
    let mut read_any = false;
    for mount in mounts {
        let root = crate::volumes::join(mount, "$Recycle.Bin");
        // One folder per user; other people's can't be read and are skipped.
        for user in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            let Ok(items) = std::fs::read_dir(user.path()) else { continue };
            read_any = true;
            for item in items.flatten() {
                if !item.file_name().to_string_lossy().starts_with("$I") {
                    continue;
                }
                if let Some((path, at)) = std::fs::read(item.path()).ok().and_then(|b| parse_info(&b)) {
                    bin.by_path.insert(path_key(Path::new(&path)), at);
                }
            }
        }
    }
    read_any.then_some(bin)
}

/// A Recycle Bin `$I` file: the deleted file's full path and when it went in.
/// Version 1 (Windows Vista to 8.1) keeps the path in a fixed 520 bytes;
/// version 2 (Windows 10 on) gives its length first.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_info(b: &[u8]) -> Option<(String, i64)> {
    let u64_at = |i: usize| Some(u64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?));
    let filetime = u64_at(16)?;
    let text = match u64_at(0)? {
        1 => b.get(24..)?,
        2 => {
            let chars = u32::from_le_bytes(b.get(24..28)?.try_into().ok()?) as usize;
            b.get(28..28 + chars * 2)?
        }
        _ => return None,
    };
    let units: Vec<u16> =
        text.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
    let path = String::from_utf16(&units).ok().filter(|p| !p.is_empty())?;
    // FILETIME: 100 ns steps since 1601.
    let at = (filetime / 10_000_000) as i64 - 11_644_473_600;
    Some((path, at.max(0)))
}

/// Linux: the freedesktop.org Trash (the home one and each disk's), whose
/// `.trashinfo` files say where each file was.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn read_now(_mounts: &[String]) -> Option<Bin> {
    let items = trash::os_limited::list().map_err(|e| log::warn!("{NAME}: can't be read: {e}")).ok()?;
    let mut bin = Bin::default();
    for item in items {
        bin.by_path.insert(path_key(&item.original_path()), item.time_deleted.max(0));
    }
    Some(bin)
}

/// macOS: the home folder's Trash and each disk's own (`.Trashes/<user id>`),
/// read as folders. A file is known there by its name and size; the time it
/// went in is when it was last moved.
#[cfg(target_os = "macos")]
fn read_now(mounts: &[String]) -> Option<Bin> {
    use std::os::unix::fs::MetadataExt;
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let uid = std::fs::metadata(&home).ok()?.uid();
    let mut folders = vec![home.join(".Trash")];
    folders.extend(mounts.iter().map(|m| Path::new(m).join(".Trashes").join(uid.to_string())));
    let mut bin = Bin::default();
    let mut read_any = false;
    for folder in folders {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        read_any = true;
        for e in entries.flatten() {
            if let Ok(m) = e.metadata()
                && m.is_file()
            {
                bin.by_name.insert((e.file_name().to_string_lossy().into_owned(), m.len()), m.ctime().max(0));
            }
        }
    }
    if !read_any {
        log::warn!("{NAME}: can't be read (Full Disk Access not given?)");
    }
    read_any.then_some(bin)
}

#[cfg(not(any(windows, target_os = "macos", all(unix, not(target_os = "ios"), not(target_os = "android")))))]
fn read_now(_mounts: &[String]) -> Option<Bin> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_found_by_where_it_was_or_by_name_and_size() {
        let bin = Bin::with(&[("D:\\Shoot\\IMG_1.CR3", 100)], &[("IMG_2.CR3", 5_000, 200)]);
        assert_eq!(bin.deleted(Path::new("D:\\Shoot\\IMG_1.CR3"), 1), Some(100));
        if cfg!(windows) {
            assert_eq!(bin.deleted(Path::new("d:\\shoot\\img_1.cr3"), 1), Some(100), "Windows paths ignore case");
        }
        assert_eq!(bin.deleted(Path::new("D:\\Other\\IMG_1.CR3"), 1), None, "another folder's file");
        assert_eq!(bin.deleted(Path::new("/Volumes/Disk/Shoot/IMG_2.CR3"), 5_000), Some(200));
        assert_eq!(bin.deleted(Path::new("/Volumes/Disk/Shoot/IMG_2.CR3"), 4_999), None, "same name, another file");
    }

    /// A `$I` file as Windows writes it: version, size, time, then the path.
    fn info(version: u64, path: &str, unix: i64) -> Vec<u8> {
        let mut b = version.to_le_bytes().to_vec();
        b.extend(14_931_156u64.to_le_bytes());
        b.extend(((unix + 11_644_473_600) as u64 * 10_000_000).to_le_bytes());
        let mut text: Vec<u16> = path.encode_utf16().collect();
        text.push(0);
        if version == 2 {
            b.extend((text.len() as u32).to_le_bytes());
        } else {
            text.resize(260, 0);
        }
        b.extend(text.iter().flat_map(|u| u.to_le_bytes()));
        b
    }

    #[test]
    fn recycle_bin_info_files_both_versions() {
        let path = "C:\\Users\\Me\\Pictures\\2026-10-17\\Éclair_2060.CR3";
        for version in [1, 2] {
            assert_eq!(parse_info(&info(version, path, 1_792_247_329)), Some((path.to_string(), 1_792_247_329)));
        }
        assert_eq!(parse_info(&info(3, path, 0)), None, "a version not known");
        assert_eq!(parse_info(&info(2, path, 0)[..30]), None, "cut short");
    }
}
