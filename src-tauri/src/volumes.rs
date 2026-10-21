//! Which disks are plugged in, and how to recognise them wherever they are mounted.
//!
//! A disk is known by its serial number, file system and size, never by its drive letter or mount point
//! (promise 4), so a USB disk is the same disk whichever letter or folder it shows up at.
//!
//! - Windows: the serial, file system and size Windows reports for each volume.
//! - macOS: `mount` lists the volumes; `diskutil` gives each one's UUID, name and size. The volume holding the
//!   user's files (`/System/Volumes/Data` since macOS 10.15) is reported as `/`.
//! - Linux: `/proc/self/mountinfo` lists the mounts; the file system UUID, label and size come from
//!   `/dev/disk/by-uuid`, `/dev/disk/by-label` and `/sys/class/block`.
//!
//! On macOS and Linux a disk's serial is its FAT or exFAT volume serial when its UUID has that shape (the number
//! Windows reports for the same disk), and otherwise 32 bits of a BLAKE3 hash of its UUID. A volume with no UUID
//! can't be told apart from other disks and is left out.
//!
//! Paths stored in the catalog (a folder's `rel_path`) separate their parts with a backslash ([`SEP`]) on every
//! system, so one catalog reads the same everywhere. [`join`], [`file_path`], [`rel_of`] and [`split_file`] convert
//! between that form and native paths. Parsing and path arithmetic are plain functions with tests that run on every
//! system; only the thin code that runs a command or reads `/proc` belongs to one system.

use anyhow::{Result, anyhow};
use std::path::Path;
use std::process::{Command, Stdio};

#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
mod linux;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos;

#[cfg(not(any(windows, target_os = "macos")))]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;

/// Separator of the paths stored in the catalog (a root's or a folder's `rel_path`): a backslash on every system,
/// so one catalog reads the same everywhere. Catalogs made on Windows already store it.
pub const SEP: char = '\\';

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedVolume {
    /// Mount point with a trailing separator, e.g. `D:\` or `/Volumes/Photos/`.
    pub mount: String,
    pub serial: u32,
    pub fs: String,
    pub size: u64,
    pub label: String,
}

impl MountedVolume {
    pub fn default_label(&self) -> String {
        if self.label.trim().is_empty() { format!("Disk {:08X}", self.serial) } else { self.label.trim().to_string() }
    }
}

/// Stops Windows from popping "There is no disk in the drive" dialogs when an
/// empty card reader is queried. Call once at startup.
pub fn quiet_errors() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SetErrorMode};
        SetErrorMode(SEM_FAILCRITICALERRORS);
    }
}

/// Cheap number that changes when disks are plugged in or out, used to notice it: the bitmask of present
/// drive letters on Windows, a hash of the list of mounts elsewhere (0 when it can't be read). It is polled, so
/// it never waits and never starts a program.
pub fn drive_mask() -> u32 {
    #[cfg(windows)]
    unsafe {
        windows::Win32::Storage::FileSystem::GetLogicalDrives()
    }
    #[cfg(not(windows))]
    platform::drive_mask()
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Every local fixed and removable volume, once per place it is mounted: a
/// drive letter (`D:\`) or a folder (`C:\Mounts\Disk1\`). A disk
/// mounted both ways appears twice, letters first; it is the same disk.
#[cfg(windows)]
pub fn mounted() -> Vec<MountedVolume> {
    use windows::Win32::Storage::FileSystem::{
        FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDriveTypeW, GetVolumePathNamesForVolumeNameW,
    };
    use windows::core::PCWSTR;

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;

    let mut out = Vec::new();
    let mut name = vec![0u16; 64];
    let Ok(find) = (unsafe { FindFirstVolumeW(&mut name) }) else { return out };
    loop {
        // `\\?\Volume{guid}\`
        let guid = wide(&from_wide(&name));
        let kind = unsafe { GetDriveTypeW(PCWSTR(guid.as_ptr())) };
        if kind == DRIVE_FIXED || kind == DRIVE_REMOVABLE {
            let mut paths = vec![0u16; 2048];
            let mut len = 0u32;
            let listed = unsafe { GetVolumePathNamesForVolumeNameW(PCWSTR(guid.as_ptr()), Some(&mut paths), &mut len) };
            if listed.is_ok() {
                let mut mounts: Vec<String> = paths[..(len as usize).min(paths.len())]
                    .split(|&c| c == 0)
                    .filter(|s| !s.is_empty())
                    .map(String::from_utf16_lossy)
                    .collect();
                mounts.sort_by_key(|m| m.len() > 3); // drive letters first
                out.extend(mounts.iter().filter_map(|m| query(m)));
            }
        }
        if unsafe { FindNextVolumeW(find, &mut name) }.is_err() {
            break;
        }
    }
    let _ = unsafe { FindVolumeClose(find) };
    out
}

#[cfg(windows)]
fn query(mount: &str) -> Option<MountedVolume> {
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumeInformationW};
    use windows::core::PCWSTR;

    let w = wide(mount);
    let mut label = vec![0u16; 261];
    let mut fs = vec![0u16; 261];
    let mut serial = 0u32;
    let mut total = 0u64;
    unsafe {
        GetVolumeInformationW(
            PCWSTR(w.as_ptr()),
            Some(&mut label),
            Some(&mut serial as *mut u32),
            None,
            None,
            Some(&mut fs),
        )
        .ok()?;
        GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), None, Some(&mut total as *mut u64), None).ok()?;
    }
    Some(MountedVolume { mount: mount.to_string(), serial, fs: from_wide(&fs), size: total, label: from_wide(&label) })
}

/// The volume holding `path`, and `path` relative to that volume's root
/// (backslashes, no leading separator; empty for the root itself).
#[cfg(windows)]
pub fn locate(path: &Path) -> Result<(MountedVolume, String)> {
    use windows::Win32::Storage::FileSystem::GetVolumePathNameW;
    use windows::core::PCWSTR;

    let full = std::path::absolute(path)?;
    let full_str = full.to_string_lossy().replace('/', "\\");
    let w = wide(&full_str);
    let mut buf = vec![0u16; 1024];
    unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut buf) }
        .map_err(|e| anyhow!("Can't find the disk for {full_str}: {e}"))?;
    let mount = from_wide(&buf);
    let vol = mounted()
        .into_iter()
        .find(|v| v.mount.eq_ignore_ascii_case(&mount))
        .ok_or_else(|| anyhow!("{mount} is not a local disk Shoebox can catalog"))?;
    let rel = full_str.get(mount.len()..).unwrap_or("").trim_matches('\\').to_string();
    Ok((vol, rel))
}

/// Free bytes on the disk holding `path`.
#[cfg(windows)]
pub fn free_bytes(path: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;
    let w = wide(&path.to_string_lossy());
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free as *mut u64), None, None) }.ok()?;
    Some(free)
}

/// Free bytes on the disk holding `path`, from `df` (POSIX output, 1 KiB blocks).
#[cfg(not(windows))]
pub fn free_bytes(path: &Path) -> Option<u64> {
    let text = output_of(Command::new("df").arg("-Pk").arg("--").arg(path))?;
    parse_df_available(&text)
}

/// Whether a delete on this disk goes to the Recycle Bin. Fixed disks
/// (internal, and most USB hard disks) have one; removable media (SD cards,
/// USB sticks) don't, so Windows deletes their files for good.
#[cfg(windows)]
pub fn has_recycle_bin(mount: &str) -> bool {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows::core::PCWSTR;
    const DRIVE_FIXED: u32 = 3;
    let w = wide(mount);
    unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) == DRIVE_FIXED }
}

/// Elsewhere the system trash handles removable media itself.
#[cfg(not(windows))]
pub fn has_recycle_bin(_mount: &str) -> bool {
    true
}

/// Every local disk, once per place it is mounted: the same disk mounted twice appears twice, the shortest mount
/// path first. A volume the system gives no UUID for can't be told apart from others and is left out.
#[cfg(not(windows))]
pub fn mounted() -> Vec<MountedVolume> {
    let mut found: Vec<MountedVolume> = platform::all_mounts()
        .into_iter()
        .filter(platform::is_local_disk)
        .filter_map(|e| platform::disk_info(&e).map(|info| volume_from(&e, info)))
        .collect();
    shortest_mount_first(&mut found);
    found
}

/// The volume holding `path`, and `path` relative to that volume's root
/// (backslashes, no leading separator; empty for the root itself).
#[cfg(not(windows))]
pub fn locate(path: &Path) -> Result<(MountedVolume, String)> {
    let full = std::fs::canonicalize(path).map_err(|e| anyhow!("Can't find the disk for {}: {e}", path.display()))?;
    let full = platform::normalise(&full.to_string_lossy());
    resolve(&full, &platform::all_mounts(), platform::is_local_disk, platform::disk_info, platform::NO_ID)
}

// ---- Catalog paths -------------------------------------------------------------------------------------------

/// Full native path of a catalog folder (`rel`, [`SEP`]-separated) on the volume mounted at `mount`.
/// An empty `rel` gives `mount` itself.
pub fn join(mount: &str, rel: &str) -> String {
    if cfg!(windows) { join_windows(mount, rel) } else { join_unix(mount, rel) }
}

/// Full native path of the file `name` in the catalog folder `rel` on the volume mounted at `mount`.
pub fn file_path(mount: &str, rel: &str, name: &str) -> String {
    if cfg!(windows) { file_path_windows(mount, rel, name) } else { file_path_unix(mount, rel, name) }
}

/// The catalog form of the native path `p` under `mount`: [`SEP`]-separated, no leading or trailing separator,
/// empty for the mount point itself.
pub fn rel_of(mount: &str, p: &Path) -> String {
    let p = p.to_string_lossy();
    if cfg!(windows) { rel_of_windows(mount, &p) } else { rel_of_unix(mount, &p) }
}

/// Splits the native path `full` of a file on the volume mounted at `mount` into its catalog folder
/// (the catalog form, as [`rel_of`]) and its file name.
pub fn split_file(mount: &str, full: &str) -> (String, String) {
    if cfg!(windows) { split_file_windows(mount, full) } else { split_file_unix(mount, full) }
}

/// Whether a file or folder name can be stored in the catalog. A name on macOS or Linux may legally hold a
/// backslash, which would read as a separator once stored; the scanner skips such names. (Windows doesn't allow
/// one.)
pub fn name_is_storable(name: &str) -> bool {
    cfg!(windows) || !name.contains(SEP)
}

fn join_windows(mount: &str, rel: &str) -> String {
    if rel.is_empty() { mount.to_string() } else { format!("{}\\{}", mount.trim_end_matches('\\'), rel) }
}

fn join_unix(mount: &str, rel: &str) -> String {
    if rel.is_empty() {
        mount.to_string()
    } else {
        format!("{}/{}", mount.trim_end_matches('/'), rel.replace(SEP, "/"))
    }
}

fn file_path_windows(mount: &str, rel: &str, name: &str) -> String {
    format!("{}\\{}", join_windows(mount, rel).trim_end_matches('\\'), name)
}

fn file_path_unix(mount: &str, rel: &str, name: &str) -> String {
    format!("{}/{}", join_unix(mount, rel).trim_end_matches('/'), name)
}

fn rel_of_windows(mount: &str, p: &str) -> String {
    p.get(mount.len()..).unwrap_or("").trim_matches('\\').to_string()
}

/// A path not under `mount` (it should not happen) is returned whole, so it can't pass for the mount point.
fn rel_of_unix(mount: &str, p: &str) -> String {
    under_unix(mount, p).unwrap_or(p).trim_matches('/').replace('/', "\\")
}

fn split_file_windows(mount: &str, full: &str) -> (String, String) {
    let mount = mount.trim_end_matches('\\');
    let rest = full.get(mount.len()..).unwrap_or("").trim_start_matches('\\');
    let (rel, name) = rest.rsplit_once('\\').unwrap_or(("", rest));
    (rel.to_string(), name.to_string())
}

fn split_file_unix(mount: &str, full: &str) -> (String, String) {
    let rest = under_unix(mount, full).unwrap_or(full).trim_start_matches('/');
    let (rel, name) = rest.rsplit_once('/').unwrap_or(("", rest));
    (rel.replace('/', "\\"), name.to_string())
}

/// What follows the mount point `mount` in `p`, if `p` is the mount point or inside it (whole components only:
/// `/Volumes/Photos` doesn't hold `/Volumes/Photos2`).
fn under_unix<'a>(mount: &str, p: &'a str) -> Option<&'a str> {
    let rest = p.strip_prefix(mount.trim_end_matches('/'))?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

// ---- Recognising disks on macOS and Linux ---------------------------------------------------------------------

/// One mount, as the system lists it.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct MountEntry {
    /// Where it is mounted, with a trailing `/`.
    mount: String,
    /// The device (`/dev/sdb1`, `/dev/disk4s1`) or other source the system reports.
    source: String,
    /// The device's `major:minor` number where the system gives one (Linux); empty otherwise.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    device: String,
    /// File system type, lowercase (`ext4`, `exfat`, `apfs`, ...).
    fstype: String,
    /// The directory of the file system that is mounted: `/` for a whole file system, something else for a
    /// btrfs subvolume or a bind mount of a folder (Linux). Always `/` on macOS.
    root: String,
    /// Mount options, comma separated.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))] // only macOS looks at them
    options: String,
}

/// What the system says about a mounted disk besides the mount table.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct DiskInfo {
    /// The volume or file system UUID: what tells this disk from every other one.
    uuid: String,
    label: String,
    size: u64,
}

/// `path` with a trailing `/`.
#[cfg_attr(not(unix), allow(dead_code))]
fn with_trailing_slash(path: &str) -> String {
    if path.ends_with('/') { path.to_string() } else { format!("{path}/") }
}

/// 32 bits of the BLAKE3 hash of `bytes`.
#[cfg_attr(not(unix), allow(dead_code))]
fn hash_u32(bytes: &[u8]) -> u32 {
    let hash = blake3::hash(bytes);
    let b = hash.as_bytes();
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// The FAT or exFAT volume serial written `XXXX-XXXX` (8 hex digits, one dash), as the number Windows reports.
#[cfg_attr(not(unix), allow(dead_code))]
fn fat_serial(uuid: &str) -> Option<u32> {
    let (high, low) = uuid.split_once('-')?;
    if high.len() != 4 || low.len() != 4 || !high.bytes().chain(low.bytes()).all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(&format!("{high}{low}"), 16).ok()
}

/// The serial of the disk with this UUID, the same on every system and for every mount. A FAT or exFAT serial is
/// kept as it is (so the disk keeps its identity when it moves between systems); anything else is hashed. A mount
/// of only part of a file system (`root` is not `/`: a btrfs subvolume, a bind mount) is a disk of its own.
#[cfg_attr(not(unix), allow(dead_code))]
fn serial_for(uuid: &str, root: &str) -> u32 {
    if root == "/" {
        fat_serial(uuid).unwrap_or_else(|| hash_u32(uuid.to_lowercase().as_bytes()))
    } else {
        hash_u32(format!("{}:{root}", uuid.to_lowercase()).as_bytes())
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
fn volume_from(entry: &MountEntry, info: DiskInfo) -> MountedVolume {
    MountedVolume {
        mount: entry.mount.clone(),
        serial: serial_for(&info.uuid, &entry.root),
        fs: entry.fstype.clone(),
        size: info.size,
        label: info.label,
    }
}

/// The same disk mounted in several places is listed shortest mount path first (the order is otherwise kept).
#[cfg_attr(not(unix), allow(dead_code))]
fn shortest_mount_first(volumes: &mut [MountedVolume]) {
    volumes.sort_by_key(|v| v.mount.len());
}

/// The mount holding `path`: the one whose mount point is the longest that contains it (whole components, so
/// `/mnt/Photos` doesn't hold `/mnt/Photos2`). A mount listed later covers an earlier one at the same place.
#[cfg_attr(not(unix), allow(dead_code))]
fn holding_entry<'a>(mounts: &'a [MountEntry], path: &str) -> Option<&'a MountEntry> {
    mounts.iter().filter(|e| under_unix(&e.mount, path).is_some()).max_by_key(|e| e.mount.trim_end_matches('/').len())
}

/// The disk holding `path` (already resolved: no links, one way of writing it) and `path` in catalog form
/// relative to it. `is_local` says whether a mount is a local disk at all, `disk_info` reads its UUID, label and
/// size; `no_id` finishes the sentence saying why a disk without a UUID can't be used.
#[cfg_attr(not(unix), allow(dead_code))]
fn resolve(
    path: &str,
    mounts: &[MountEntry],
    is_local: impl Fn(&MountEntry) -> bool,
    disk_info: impl Fn(&MountEntry) -> Option<DiskInfo>,
    no_id: &str,
) -> Result<(MountedVolume, String)> {
    let holder =
        holding_entry(mounts, path).ok_or_else(|| anyhow!("{path} is not on a local disk Shoebox can catalog"))?;
    if !is_local(holder) {
        return Err(anyhow!("{} is not a local disk Shoebox can catalog", holder.mount));
    }
    let info = disk_info(holder).ok_or_else(|| {
        anyhow!("{} is a local disk, but Shoebox can't tell it apart from other disks: {no_id}", holder.mount)
    })?;
    if path.contains(SEP) {
        return Err(anyhow!("A folder whose name holds a backslash can't be catalogued on this system: {path}"));
    }
    let rel = rel_of_unix(&holder.mount, path);
    Ok((volume_from(holder, info), rel))
}

/// How long `mount`, `diskutil` or `df` may take. A disk that stopped answering (a sleeping USB disk, a lost
/// network share) can make them wait for ever, and the watcher that notices plugged-in disks must not wait with them.
#[cfg_attr(not(unix), allow(dead_code))]
const COMMAND_TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(20);

/// The output of a finished command (as text), or `None` if it couldn't be run, failed, or took longer than
/// [`COMMAND_TIME_LIMIT`] (it is then stopped).
#[cfg_attr(not(unix), allow(dead_code))]
fn output_of(command: &mut Command) -> Option<String> {
    output_within(command, COMMAND_TIME_LIMIT)
}

#[cfg_attr(not(unix), allow(dead_code))]
fn output_within(command: &mut Command, limit: std::time::Duration) -> Option<String> {
    use std::io::Read;
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // Read on the side, so a long output can't fill the pipe and stop the command.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).ok().map(|_| bytes)
    });
    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let bytes = reader.join().ok()??;
    status.success().then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// The bytes free on the file system, from the output of `df -Pk`: a header, then a line of
/// `filesystem 1024-blocks used available capacity mount-point`. A file system name or a mount point may hold
/// spaces, so the columns are counted back from the capacity (`NN%`) rather than from the start of the line.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_df_available(text: &str) -> Option<u64> {
    df_kib(text)?[2].checked_mul(1024)
}

/// The size of the file system in bytes, from the same `df -Pk` output (`None` for a file system of no size).
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_df_total(text: &str) -> Option<u64> {
    df_kib(text)?[0].checked_mul(1024).filter(|&bytes| bytes > 0)
}

/// The total, used and available KiB on the line `df -Pk` prints for a file system.
#[cfg_attr(not(unix), allow(dead_code))]
fn df_kib(text: &str) -> Option<[u64; 3]> {
    let cols: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let is_capacity = |c: &str| {
        c == "-" || c.strip_suffix('%').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    let at =
        (3..cols.len()).find(|&i| is_capacity(cols[i]) && cols[i - 3..i].iter().all(|n| n.parse::<u64>().is_ok()))?;
    let kib = |i: usize| cols[i].parse::<u64>().ok();
    Some([kib(at - 3)?, kib(at - 2)?, kib(at - 1)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mount: &str, source: &str, fstype: &str, root: &str) -> MountEntry {
        MountEntry {
            mount: mount.to_string(),
            source: source.to_string(),
            device: String::new(),
            fstype: fstype.to_string(),
            root: root.to_string(),
            options: String::new(),
        }
    }

    fn info(uuid: &str, label: &str, size: u64) -> DiskInfo {
        DiskInfo { uuid: uuid.to_string(), label: label.to_string(), size }
    }

    fn volume(mount: &str) -> MountedVolume {
        MountedVolume { mount: mount.to_string(), serial: 1, fs: "ext4".to_string(), size: 1, label: String::new() }
    }

    #[test]
    fn unix_join_turns_catalog_paths_into_native_ones() {
        assert_eq!(join_unix("/Volumes/Photos/", "2026\\Trip"), "/Volumes/Photos/2026/Trip");
        assert_eq!(join_unix("/", "home\\a"), "/home/a");
        assert_eq!(join_unix("/", ""), "/");
        assert_eq!(join_unix("/Volumes/Photos/", ""), "/Volumes/Photos/");
        assert_eq!(join_unix("/mnt/disk", "a"), "/mnt/disk/a");
    }

    #[test]
    fn windows_join_is_what_it_always_was() {
        assert_eq!(join_windows("D:\\", "a\\b"), "D:\\a\\b");
        assert_eq!(join_windows("D:\\", ""), "D:\\");
        assert_eq!(join_windows("C:\\Mounts\\Disk1\\", "x"), "C:\\Mounts\\Disk1\\x");
        assert_eq!(join_windows("C:\\folder", "x"), "C:\\folder\\x");
    }

    #[test]
    fn file_paths() {
        assert_eq!(file_path_windows("D:\\", "", "a.jpg"), "D:\\a.jpg");
        assert_eq!(file_path_windows("D:\\", "x\\y", "a.jpg"), "D:\\x\\y\\a.jpg");
        assert_eq!(file_path_unix("/", "", "a.jpg"), "/a.jpg");
        assert_eq!(file_path_unix("/Volumes/P/", "", "a.jpg"), "/Volumes/P/a.jpg");
        assert_eq!(file_path_unix("/Volumes/P/", "x\\y", "a.jpg"), "/Volumes/P/x/y/a.jpg");
    }

    #[test]
    fn unix_rel_of_gives_the_catalog_form() {
        assert_eq!(rel_of_unix("/Volumes/Photos/", "/Volumes/Photos/2026/Trip"), "2026\\Trip");
        assert_eq!(rel_of_unix("/Volumes/Photos/", "/Volumes/Photos"), "");
        assert_eq!(rel_of_unix("/Volumes/Photos/", "/Volumes/Photos/"), "");
        assert_eq!(rel_of_unix("/", "/home/a"), "home\\a");
        assert_eq!(rel_of_unix("/", "/"), "");
        // Not under the mount point, not even by a longer name: never passes for the mount point.
        assert_eq!(rel_of_unix("/Volumes/Photos/", "/Volumes/Photos2/x"), "Volumes\\Photos2\\x");
    }

    #[test]
    fn windows_rel_of_cuts_the_mount_off() {
        assert_eq!(rel_of_windows("D:\\", "D:\\a\\b"), "a\\b");
        assert_eq!(rel_of_windows("D:\\", "D:\\"), "");
        assert_eq!(rel_of_windows("C:\\Mounts\\Disk1\\", "C:\\Mounts\\Disk1\\x\\"), "x");
    }

    #[test]
    fn files_split_into_catalog_folder_and_name() {
        assert_eq!(split_file_windows("D:\\", "D:\\a\\b\\c.jpg"), ("a\\b".to_string(), "c.jpg".to_string()));
        assert_eq!(split_file_windows("D:\\", "D:\\c.jpg"), (String::new(), "c.jpg".to_string()));
        assert_eq!(split_file_unix("/Volumes/P/", "/Volumes/P/a/b/c.jpg"), ("a\\b".to_string(), "c.jpg".to_string()));
        assert_eq!(split_file_unix("/Volumes/P/", "/Volumes/P/c.jpg"), (String::new(), "c.jpg".to_string()));
        assert_eq!(split_file_unix("/", "/home/a/c.jpg"), ("home\\a".to_string(), "c.jpg".to_string()));
        assert_eq!(split_file_unix("/", "/c.jpg"), (String::new(), "c.jpg".to_string()));
    }

    #[test]
    fn a_file_path_splits_back_into_what_made_it() {
        for (mount, rel) in [("/", ""), ("/", "a\\b"), ("/Volumes/P Q/", "x y\\z"), ("/mnt/d", "x")] {
            let made = file_path_unix(mount, rel, "n.jpg");
            assert_eq!(split_file_unix(mount, &made), (rel.to_string(), "n.jpg".to_string()));
        }
        for (mount, rel) in [("D:\\", ""), ("D:\\", "a\\b"), ("C:\\Mounts\\Disk1\\", "x")] {
            let made = file_path_windows(mount, rel, "n.jpg");
            assert_eq!(split_file_windows(mount, &made), (rel.to_string(), "n.jpg".to_string()));
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_public_functions_use_windows_paths_here() {
        assert_eq!(join("D:\\", "a\\b"), "D:\\a\\b");
        assert_eq!(file_path("D:\\", "a", "c.jpg"), "D:\\a\\c.jpg");
        assert_eq!(rel_of("D:\\", Path::new("D:\\a\\b")), "a\\b");
        assert_eq!(split_file("D:\\", "D:\\a\\c.jpg"), ("a".to_string(), "c.jpg".to_string()));
        assert!(name_is_storable("a\\b"));
    }

    #[cfg(unix)]
    #[test]
    fn the_public_functions_use_unix_paths_here() {
        assert_eq!(join("/Volumes/P/", "a\\b"), "/Volumes/P/a/b");
        assert_eq!(file_path("/Volumes/P/", "a", "c.jpg"), "/Volumes/P/a/c.jpg");
        assert_eq!(rel_of("/Volumes/P/", Path::new("/Volumes/P/a/b")), "a\\b");
        assert_eq!(split_file("/Volumes/P/", "/Volumes/P/a/c.jpg"), ("a".to_string(), "c.jpg".to_string()));
        assert!(!name_is_storable("a\\b"));
    }

    #[test]
    fn only_a_backslash_makes_a_name_unstorable() {
        assert!(name_is_storable("IMG_0001.CR3"));
        assert!(name_is_storable("Été 2026 (1)"));
        assert_eq!(name_is_storable("a\\b"), cfg!(windows));
    }

    #[test]
    fn fat_serials_are_8_hex_digits_with_a_dash() {
        assert_eq!(fat_serial("1A2B-3C4D"), Some(0x1A2B_3C4D));
        assert_eq!(fat_serial("1a2b-3c4d"), Some(0x1A2B_3C4D));
        assert_eq!(fat_serial("0000-0001"), Some(1));
        for other in ["1A2B3C4D", "1A2B-3C4", "1A2B-3C4DE", "1A2B-3C4G", "-1A2B3C4D", "1A2B-3C4D-0000", "", "+1A2-3C4D"]
        {
            assert_eq!(fat_serial(other), None, "{other}");
        }
        assert_eq!(fat_serial("9F2C1B7A-4D3E-4C55-8A6B-0123456789AB"), None);
    }

    #[test]
    fn a_fat_disk_keeps_the_serial_windows_reports() {
        assert_eq!(serial_for("1A2B-3C4D", "/"), 0x1A2B_3C4D);
        assert_eq!(serial_for("1a2b-3c4d", "/"), 0x1A2B_3C4D);
    }

    #[test]
    fn other_disks_get_32_bits_of_the_hash_of_their_lowercase_uuid() {
        let uuid = "9F2C1B7A-4D3E-4C55-8A6B-0123456789AB";
        let hash = blake3::hash(uuid.to_lowercase().as_bytes());
        let expected = u32::from_le_bytes(hash.as_bytes()[..4].try_into().unwrap());
        assert_eq!(serial_for(uuid, "/"), expected);
        assert_eq!(serial_for(&uuid.to_lowercase(), "/"), expected);
        assert_ne!(serial_for(uuid, "/"), serial_for("9F2C1B7A-4D3E-4C55-8A6B-0123456789AC", "/"));
    }

    #[test]
    fn a_part_of_a_file_system_is_a_disk_of_its_own() {
        let uuid = "9F2C1B7A-4D3E-4C55-8A6B-0123456789AB";
        let (whole, home, var) = (serial_for(uuid, "/"), serial_for(uuid, "/@home"), serial_for(uuid, "/@var"));
        assert!(whole != home && whole != var && home != var);
        assert_eq!(home, serial_for(&uuid.to_lowercase(), "/@home"));
        // Even a FAT-shaped id: only a whole file system keeps the serial Windows shows.
        assert_ne!(serial_for("1A2B-3C4D", "/data"), 0x1A2B_3C4D);
    }

    #[test]
    fn a_volume_is_built_from_its_mount_and_what_the_system_says() {
        let e = entry("/media/me/Photos/", "/dev/sdb1", "exfat", "/");
        let v = volume_from(&e, info("1A2B-3C4D", "Photos", 2_000_000_000_000));
        assert_eq!(
            v,
            MountedVolume {
                mount: "/media/me/Photos/".into(),
                serial: 0x1A2B_3C4D,
                fs: "exfat".into(),
                size: 2_000_000_000_000,
                label: "Photos".into()
            }
        );
        assert_eq!(v.default_label(), "Photos");
        let unnamed = volume_from(&e, info("1A2B-3C4D", "", 1));
        assert_eq!(unnamed.default_label(), "Disk 1A2B3C4D");
    }

    #[test]
    fn the_same_disk_mounted_twice_is_listed_shortest_path_first() {
        let mut v = [volume("/media/me/Photos/"), volume("/"), volume("/mnt/p/"), volume("/mnt/q/")];
        shortest_mount_first(&mut v);
        let order: Vec<&str> = v.iter().map(|v| v.mount.as_str()).collect();
        assert_eq!(order, ["/", "/mnt/p/", "/mnt/q/", "/media/me/Photos/"]);
    }

    #[test]
    fn the_longest_mount_point_holds_a_path() {
        let table = [
            entry("/", "/dev/sda2", "ext4", "/"),
            entry("/home/", "/dev/sda3", "ext4", "/"),
            entry("/home/me/Photos/", "/dev/sdb1", "exfat", "/"),
            entry("/home/me/Photos2/", "/dev/sdc1", "exfat", "/"),
        ];
        let holder = |p: &str| holding_entry(&table, p).map(|e| e.mount.as_str());
        assert_eq!(holder("/home/me/Photos/2026/Trip"), Some("/home/me/Photos/"));
        assert_eq!(holder("/home/me/Photos"), Some("/home/me/Photos/"));
        assert_eq!(holder("/home/me/Photos3/x"), Some("/home/"));
        assert_eq!(holder("/home/me/Photos2"), Some("/home/me/Photos2/"));
        assert_eq!(holder("/etc/hosts"), Some("/"));
        assert_eq!(holder("relative/path"), None);
    }

    #[test]
    fn a_later_mount_covers_an_earlier_one_at_the_same_place() {
        let table = [entry("/mnt/x/", "/dev/sdb1", "ext4", "/"), entry("/mnt/x/", "/dev/sdc1", "ext4", "/")];
        assert_eq!(holding_entry(&table, "/mnt/x/a").map(|e| e.source.as_str()), Some("/dev/sdc1"));
    }

    fn lookup(mounts: &[MountEntry], path: &str) -> Result<(MountedVolume, String)> {
        resolve(
            path,
            mounts,
            |e| e.source.starts_with("/dev/"),
            |e| match e.source.as_str() {
                "/dev/sda2" => Some(info("0a1b2c3d-0000-4000-8000-000000000001", "", 500_000_000_000)),
                "/dev/sdb1" => Some(info("1A2B-3C4D", "My Photos", 2_000_000_000_000)),
                _ => None,
            },
            "no UUID is listed for it",
        )
    }

    fn sample_mounts() -> [MountEntry; 4] {
        [
            entry("/", "/dev/sda2", "ext4", "/"),
            entry("/boot/efi/", "/dev/sda1", "vfat", "/"),
            entry("/run/", "tmpfs", "tmpfs", "/"),
            entry("/media/me/My Photos/", "/dev/sdb1", "exfat", "/"),
        ]
    }

    #[test]
    fn a_path_is_located_on_the_disk_that_holds_it() {
        let mounts = sample_mounts();
        let (v, rel) = lookup(&mounts, "/media/me/My Photos/2026/Trip").unwrap();
        assert_eq!(
            (v.mount.as_str(), v.serial, v.fs.as_str(), v.label.as_str()),
            ("/media/me/My Photos/", 0x1A2B_3C4D, "exfat", "My Photos")
        );
        assert_eq!(rel, "2026\\Trip");
        let (v, rel) = lookup(&mounts, "/media/me/My Photos").unwrap();
        assert_eq!((v.mount.as_str(), rel.as_str()), ("/media/me/My Photos/", ""));
        let (v, rel) = lookup(&mounts, "/home/me/Pictures").unwrap();
        assert_eq!((v.mount.as_str(), rel.as_str()), ("/", "home\\me\\Pictures"));
        assert_eq!(v.serial, serial_for("0a1b2c3d-0000-4000-8000-000000000001", "/"));
    }

    #[test]
    fn a_path_on_something_that_is_not_a_local_disk_is_refused_by_name() {
        let err = lookup(&sample_mounts(), "/run/user/1000/x").unwrap_err().to_string();
        assert_eq!(err, "/run/ is not a local disk Shoebox can catalog");
        let err = lookup(&[], "/x").unwrap_err().to_string();
        assert_eq!(err, "/x is not on a local disk Shoebox can catalog");
    }

    #[test]
    fn a_disk_without_an_id_is_refused_with_the_reason() {
        let err = lookup(&sample_mounts(), "/boot/efi/EFI").unwrap_err().to_string();
        assert_eq!(
            err,
            "/boot/efi/ is a local disk, but Shoebox can't tell it apart from other disks: no UUID is listed for it"
        );
    }

    #[test]
    fn a_folder_whose_name_holds_a_backslash_is_refused() {
        let err = lookup(&sample_mounts(), "/media/me/My Photos/a\\b").unwrap_err().to_string();
        assert!(err.contains("backslash"), "{err}");
    }

    #[test]
    fn df_output_gives_the_free_bytes() {
        let gnu = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n\
                   /dev/sda2        244568380 123456789 108000000      54% /\n";
        assert_eq!(parse_df_available(gnu), Some(108_000_000 * 1024));
        let mac = "Filesystem   1024-blocks      Used Available Capacity  Mounted on\n\
                   /dev/disk3s5   971350180 383239236 551221948    42%    /System/Volumes/Data\n";
        assert_eq!(parse_df_available(mac), Some(551_221_948 * 1024));
    }

    #[test]
    fn df_output_with_spaces_in_names_is_still_read() {
        let text = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                    map auto_home 0 0 0 100% /System/Volumes/Data/home\n";
        assert_eq!(parse_df_available(text), Some(0));
        let text = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                    /dev/sdb1 1000 400 600 40% /media/me/Photos 100% (copy)\n";
        assert_eq!(parse_df_available(text), Some(600 * 1024));
        let text = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                    //nas/My Photos 1000 400 600 40% /mnt/nas\n";
        assert_eq!(parse_df_available(text), Some(600 * 1024));
    }

    #[test]
    fn df_output_gives_the_size_of_the_file_system() {
        let text = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda1 2000 800 1200 40% /\n";
        assert_eq!(parse_df_total(text), Some(2000 * 1024));
        assert_eq!(parse_df_available(text), Some(1200 * 1024));
        assert_eq!(parse_df_total("header\nnone 0 0 0 - /mnt/empty\n"), None);
    }

    #[test]
    fn unreadable_df_output_gives_nothing() {
        assert_eq!(parse_df_available(""), None);
        assert_eq!(parse_df_available("Filesystem 1024-blocks Used Available Capacity Mounted on\n"), None);
        assert_eq!(parse_df_available("header\ndf: cannot read table of mounted file systems\n"), None);
        assert_eq!(parse_df_available("header\n/dev/sdb1 1000 400 x 40% /mnt\n"), None);
    }

    #[test]
    fn a_command_that_answers_is_read_and_one_that_hangs_is_stopped() {
        let limit = std::time::Duration::from_secs(5);
        let mut quick = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "echo", "hello"]);
            c
        } else {
            let mut c = Command::new("echo");
            c.arg("hello");
            c
        };
        assert_eq!(output_within(&mut quick, limit).as_deref().map(str::trim), Some("hello"));

        let mut slow = if cfg!(windows) {
            let mut c = Command::new("ping");
            c.args(["-n", "30", "127.0.0.1"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        let started = std::time::Instant::now();
        assert_eq!(output_within(&mut slow, std::time::Duration::from_millis(300)), None);
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "stopped, not waited for");
        // A command that doesn't exist is no answer either.
        assert_eq!(output_within(&mut Command::new("no-such-command-here"), limit), None);
    }

    #[test]
    fn mount_points_get_a_trailing_slash() {
        assert_eq!(with_trailing_slash("/"), "/");
        assert_eq!(with_trailing_slash("/Volumes/P"), "/Volumes/P/");
        assert_eq!(with_trailing_slash("/Volumes/P/"), "/Volumes/P/");
    }
}
