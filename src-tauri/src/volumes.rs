//! Which disks are plugged in, and how to recognise them whatever their drive letter.

use anyhow::{Result, anyhow};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedVolume {
    /// Mount point, e.g. `D:\`.
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

/// Cheap bitmask of present drive letters, used to notice plug/unplug.
pub fn drive_mask() -> u32 {
    #[cfg(windows)]
    unsafe {
        windows::Win32::Storage::FileSystem::GetLogicalDrives()
    }
    #[cfg(not(windows))]
    0
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

#[cfg(not(windows))]
pub fn free_bytes(_path: &Path) -> Option<u64> {
    None
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

#[cfg(not(windows))]
pub fn mounted() -> Vec<MountedVolume> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn locate(_path: &Path) -> Result<(MountedVolume, String)> {
    Err(anyhow!("Shoebox only runs on Windows"))
}

/// Full path of a catalog folder on a mounted volume.
pub fn join(mount: &str, rel: &str) -> String {
    if rel.is_empty() { mount.to_string() } else { format!("{}\\{}", mount.trim_end_matches('\\'), rel) }
}
