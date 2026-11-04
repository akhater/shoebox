//! Argentum, the editor Shoebox hands photos to. Always a separate program,
//! started with a photo's path and never linked (its licence would become
//! Shoebox's). It shows the photo in its open window when it has one. What it
//! edits it keeps in its own `.agdata` file; Shoebox only says which photo,
//! and looks at that file again when the user comes back (see edits.rs).

use crate::catalog::Catalog;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where to get Argentum.
pub const RELEASES_PAGE: &str = "https://github.com/akhater/argentum/releases/latest";

/// What became of a photo handed to Argentum.
#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Handoff {
    /// Argentum was started with this file.
    Opened { path: String },
    /// Argentum isn't on this computer.
    NoArgentum,
    /// Its copies are all on these disks, none plugged in.
    Unplugged { disks: Vec<String> },
    /// No copy of it is where the catalog last saw one.
    Gone,
}

/// A macOS app: a folder holding the program, started through the system.
fn is_app(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))
}

/// The program itself to start for Argentum at `path` (as [`find`] gives
/// it): inside a macOS app, when run headless rather than through the system.
pub(crate) fn executable(path: &Path) -> PathBuf {
    if is_app(path) { path.join("Contents/MacOS/Argentum") } else { path.to_path_buf() }
}

/// The program, when `path` is Argentum. Only a program of that name is ever
/// started, whatever the settings say.
pub fn program(path: &Path) -> Option<PathBuf> {
    let named = path.file_stem().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("argentum"));
    let there = if is_app(path) { path.join("Contents/MacOS/Argentum").is_file() } else { path.is_file() };
    (named && there).then(|| path.to_path_buf())
}

/// Where Argentum is: the one chosen in Settings (`chosen`, empty for none)
/// while it is still there, else where its installers put it.
pub fn find(chosen: &str) -> Option<PathBuf> {
    let chosen = (!chosen.is_empty()).then(|| PathBuf::from(chosen));
    chosen.into_iter().chain(installed()).find_map(|p| program(&p))
}

#[cfg(windows)]
fn installed() -> Vec<PathBuf> {
    ["LOCALAPPDATA", "ProgramFiles", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|base| PathBuf::from(base).join("Argentum").join("Argentum.exe"))
        .collect()
}

#[cfg(target_os = "macos")]
fn installed() -> Vec<PathBuf> {
    let mine = std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications/Argentum.app"));
    [Some(PathBuf::from("/Applications/Argentum.app")), mine].into_iter().flatten().collect()
}

/// A package installs it in /usr/bin, which a window started from the desktop
/// may not have on its PATH; an AppImage is kept wherever its owner likes,
/// under its own versioned name.
#[cfg(all(unix, not(target_os = "macos")))]
fn installed() -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = std::env::var_os("PATH").iter().flat_map(std::env::split_paths).collect();
    folders.push(PathBuf::from("/usr/bin"));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let kept = home.iter().flat_map(|h| [h.join("Applications"), h.join(".local/bin"), h.join("Downloads")]);
    let appimages = kept
        .filter_map(|folder| std::fs::read_dir(folder).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| is_appimage(&name.to_string_lossy())));
    folders.iter().flat_map(|f| [f.join("Argentum"), f.join("argentum")]).chain(appimages).collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_appimage(name: &str) -> bool {
    let name = name.to_lowercase();
    name.contains("argentum") && name.ends_with(".appimage")
}

/// The file of a photo to edit, or why there is none. Argentum keeps its edits
/// beside the file it edited, so a photo with copies goes on from the one that
/// has them.
pub fn file_to_edit(cat: &Catalog, photo: i64) -> Result<Handoff> {
    let files = cat.photo_detail(photo)?.files;
    let here: Vec<String> =
        files.iter().filter(|f| f.state == "online").filter_map(|f| cat.file_path(f.id).ok()).collect();
    if let Some(path) = here.iter().find(|p| Path::new(&format!("{p}.agdata")).exists()).or(here.first()) {
        return Ok(Handoff::Opened { path: path.clone() });
    }
    let mut disks: Vec<String> = files
        .iter()
        .filter(|f| f.state == "offline" || f.state == "unreachable")
        .map(|f| f.volume_label.clone())
        .collect();
    disks.sort();
    disks.dedup();
    Ok(if disks.is_empty() { Handoff::Gone } else { Handoff::Unplugged { disks } })
}

fn command(exe: &Path, file: &str) -> Command {
    let mut cmd = if is_app(exe) {
        // The system brings a running app forward and hands it the file.
        let mut open = Command::new("open");
        open.arg("-a").arg(exe);
        open
    } else {
        Command::new(exe)
    };
    cmd.arg(file);
    if let Some(folder) = exe.parent() {
        cmd.current_dir(folder);
    }
    // It outlives whoever asked: holding their output open would keep a script waiting.
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd
}

/// Opens a photo in Argentum, or says why it can't.
pub fn open(cat: &Catalog, photo: i64) -> Result<Handoff> {
    let Some(exe) = find(&cat.settings().argentum_path) else {
        return Ok(Handoff::NoArgentum);
    };
    let handoff = file_to_edit(cat, photo)?;
    if let Handoff::Opened { path } = &handoff {
        crate::edits::handed_over(cat, Path::new(path));
        let mut child = command(&exe, path).spawn().with_context(|| format!("Couldn't start {}", exe.display()))?;
        // Reaped in the background so no finished process lingers.
        std::thread::spawn(move || child.wait());
    }
    Ok(handoff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argentum_gets_the_path_alone_and_nothing_of_the_caller() {
        let cmd = command(Path::new("C:/Apps/Argentum/Argentum.exe"), "D:/Trip 2/IMG_0001.CR3");
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["D:/Trip 2/IMG_0001.CR3"]);
        assert_eq!(cmd.get_current_dir(), Some(Path::new("C:/Apps/Argentum")));
    }

    #[test]
    fn a_macos_app_is_started_through_the_system() {
        let cmd = command(Path::new("/Applications/Argentum.app"), "/Volumes/Trips/IMG_0001.CR3");
        assert_eq!(cmd.get_program(), "open");
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["-a", "/Applications/Argentum.app", "/Volumes/Trips/IMG_0001.CR3"]
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn an_appimage_is_known_by_its_versioned_name() {
        assert!(is_appimage("Argentum_26.44.5_amd64.AppImage"));
        assert!(!is_appimage("Shoebox_26.44.5_amd64.AppImage"));
        assert!(!is_appimage("argentum-notes.txt"));
    }

    #[test]
    fn a_handoff_says_what_became_of_the_photo() {
        let json = |h: Handoff| serde_json::to_string(&h).unwrap();
        assert_eq!(json(Handoff::Opened { path: "D:/a.cr3".into() }), r#"{"state":"opened","path":"D:/a.cr3"}"#);
        assert_eq!(json(Handoff::NoArgentum), r#"{"state":"noArgentum"}"#);
        assert_eq!(
            json(Handoff::Unplugged { disks: vec!["Trips".into()] }),
            r#"{"state":"unplugged","disks":["Trips"]}"#
        );
        assert_eq!(json(Handoff::Gone), r#"{"state":"gone"}"#);
    }
}
