//! What a photo edited in Argentum looks like: Argentum renders it (its
//! headless export, one start per batch of photos) and Shoebox keeps the
//! result as the photo's edited preview and thumbnail, shown instead of the
//! original's. Rendered again only when the edit changes; a disk unplugged
//! waits, and keeps showing its last edited pictures.
//!
//! Argentum is played here by this test program itself ([`fake_argentum`]),
//! started again by the code under test. Lives here, not in src/, because it
//! writes sample photos and sidecars (promise 1).

use crate::edits::{EDIT, NO_EDIT, World, scan, tick};
use image::RgbImage;
use shoebox_lib::catalog::Catalog;
use shoebox_lib::edits::FileEdit;
use shoebox_lib::paths::DataDir;
use shoebox_lib::private;
use shoebox_lib::renders::{self, Exporter, Options, Picture, Report, Support};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const EDIT_2: &str = r#"{"version":1,"rating":0,"adjustments":{"exposure":1.5}}"#;
/// An edit the fake Argentum can't render.
const BROKEN: &str = r#"{"version":1,"rating":0,"adjustments":{"exposure":0.7,"note":"fail-me"}}"#;

/// Argentum's headless export as Shoebox starts it, played by this program:
/// `<this program> --exact edited_previews::fake_argentum --nocapture -q -- <mode> export ...`.
/// Run with the other tests it finds no `--` and does nothing. `mode`: `new`
/// (renders each photo asked for, its picture inverted; one whose edit says
/// "fail-me" fails), `old` (an Argentum from before `--files`), `slow` (takes
/// a minute before rendering), `noisy` (prints an endless line and bytes that
/// aren't text first), `escape` (answers with the photo itself as its output).
#[test]
fn fake_argentum() {
    let args: Vec<String> = std::env::args().collect();
    let Some(at) = args.iter().position(|a| a == "--") else { return };
    let (mode, args) = (args[at + 1].as_str(), &args[at + 2..]);
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    if mode == "old" {
        eprintln!("No supported images found at the source path.");
        std::process::exit(1);
    }
    let Some(list) = value("--files") else {
        eprintln!("Headless export failed: Missing value for --files; expected a text file with one path per line.");
        std::process::exit(2);
    };
    assert_eq!(value("--format").map(String::as_str), Some("jpeg"));
    let long: u32 = value("--long-edge").unwrap().parse().unwrap();
    let out = PathBuf::from(value("--output").unwrap());
    if mode == "slow" {
        std::thread::sleep(Duration::from_secs(60));
    }
    std::fs::create_dir_all(&out).unwrap();
    if mode == "noisy" {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(&vec![b'x'; 200_000]).unwrap();
        stdout.write_all(b"\n\xff\xfe not text {\"ok\":tru\n").unwrap();
    }
    let mut failed = false;
    for (n, line) in std::fs::read_to_string(list).unwrap().lines().enumerate() {
        let (photo, copy) = match line.split_once("?vc=") {
            Some((photo, copy)) => (photo, Some(copy)),
            None => (line, None),
        };
        let sidecar = match copy {
            Some(copy) => format!("{photo}.{copy}.agdata"),
            None => format!("{photo}.agdata"),
        };
        let line = match std::fs::read_to_string(&sidecar) {
            Ok(edit) if !edit.contains("fail-me") => {
                let mut img = image::open(photo).unwrap();
                img.invert();
                let file = out.join(format!("{n}_edited.jpg"));
                img.thumbnail(long, long).to_rgb8().save(&file).unwrap();
                let output = if mode == "escape" { PathBuf::from(photo) } else { file };
                serde_json::json!({ "source": photo, "copy": copy, "output": output, "ok": true })
            }
            _ => {
                failed = true;
                serde_json::json!({ "source": photo, "copy": copy, "ok": false, "error": "Failed to load" })
            }
        };
        println!("{line}");
    }
    std::process::exit(i32::from(failed));
}

fn argentum(mode: &str) -> Exporter {
    let me = std::env::current_exe().unwrap();
    Exporter::wrapped(&me, &["--exact", "edited_previews::fake_argentum", "--nocapture", "-q", "--", mode])
}

fn render_with(cat: &Catalog, mode: &str, options: Options, cancel: &AtomicBool) -> Report {
    renders::render_pending(cat, &argentum(mode), &options, cancel, &|_| {}).unwrap()
}

fn render(cat: &Catalog, retry: bool) -> Report {
    render_with(cat, "new", Options { photo: None, retry }, &AtomicBool::new(false))
}

fn photo(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

fn shown(cat: &Catalog, photo: i64, what: Picture) -> PathBuf {
    renders::shown(cat, photo, what).unwrap()
}

/// The edits listed with the photo's first file, as its details show them.
fn edits(cat: &Catalog, photo: i64) -> Vec<FileEdit> {
    cat.photo_detail(photo).unwrap().files.remove(0).edits
}

/// The version an edited picture's file is of (`<photo>-<version>.jpg`).
fn version(path: &Path) -> String {
    path.file_stem().unwrap().to_str().unwrap().split_once('-').unwrap().1.to_string()
}

fn middle(path: &Path) -> u8 {
    let img: RgbImage = image::open(path).unwrap().to_rgb8();
    img.get_pixel(img.width() / 2, img.height() / 2).0[0]
}

fn rev(cat: &Catalog, photo: i64) -> i64 {
    cat.db().query_row("SELECT preview_rev FROM photos WHERE id = ?1", [photo], |r| r.get(0)).unwrap()
}

#[test]
fn an_edited_photo_shows_what_argentum_made_of_it() {
    let w = World::new("render-shown");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let (p, other) = (photo(&cat, "IMG_0.JPG"), photo(&cat, "IMG_1.JPG"));
    let before = rev(&cat, p);
    let r = render(&cat, false);
    assert_eq!((r.started, r.rendered, r.failed, r.waiting), (1, 1, 0, 0));

    let thumb = shown(&cat, p, Picture::Thumb);
    assert_ne!(thumb, w.data.thumb_path(p), "the edited thumbnail is shown");
    assert!(thumb.starts_with(&w.data.root), "kept in the catalog");
    assert!(middle(&w.data.thumb_path(p)) < 40 && middle(&thumb) > 215, "Argentum's picture, not the original's");
    let preview = shown(&cat, p, Picture::Preview);
    let (pw, ph) = image::image_dimensions(&preview).unwrap();
    assert!(pw.max(ph) <= renders::LONG_EDGE);
    assert!(rev(&cat, p) > before, "a new revision: the window asks for the new pictures");
    assert_eq!(edits(&cat, p)[0].version, Some(version(&thumb)));

    assert_eq!(shown(&cat, other, Picture::Thumb), w.data.thumb_path(other), "a photo not edited shows its own");
    assert_eq!(shown(&cat, p, Picture::Original), w.data.preview_path(p));
    let beside: Vec<_> = std::fs::read_dir(w.photos()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(beside.len(), 5, "nothing written beside the photos: {beside:?}");
    assert!(!w.data.root.join("rendering").exists(), "Argentum's output is cleared once kept");
}

#[test]
fn an_edit_rendered_once_is_not_rendered_again() {
    let w = World::new("render-once");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    assert_eq!(render(&cat, false).rendered, 1);
    assert_eq!(render(&cat, false), Report::default(), "the same edit: Argentum isn't started");
    drop(cat);
    let cat = w.open(true);
    scan(&cat);
    assert_eq!(render(&cat, true), Report::default(), "nor after a start and a rescan");
}

#[test]
fn a_new_edit_replaces_the_old_pictures() {
    let w = World::new("render-again");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    render(&cat, false);
    let p = photo(&cat, "IMG_0.JPG");
    let (thumb, preview) = (shown(&cat, p, Picture::Thumb), shown(&cat, p, Picture::Preview));
    tick();
    w.write("IMG_0.JPG.agdata", EDIT_2);
    scan(&cat);
    assert_eq!(shown(&cat, p, Picture::Thumb), thumb, "the last edited one shows until the new one is in");
    assert_eq!(render(&cat, false).rendered, 1);
    let new = shown(&cat, p, Picture::Thumb);
    assert!(new != thumb && new.is_file(), "a new name: a backup copies only what's new");
    assert!(!thumb.exists() && !preview.exists(), "the old ones are removed");
}

#[test]
fn an_edit_undone_or_gone_shows_the_original_again() {
    let w = World::new("render-undone");
    w.write("IMG_0.JPG.agdata", EDIT);
    w.write("IMG_1.JPG.agdata", EDIT_2);
    let cat = w.open(true);
    scan(&cat);
    render(&cat, false);
    let (a, b) = (photo(&cat, "IMG_0.JPG"), photo(&cat, "IMG_1.JPG"));
    let edited = [shown(&cat, a, Picture::Thumb), shown(&cat, b, Picture::Preview)];
    tick();
    w.write("IMG_0.JPG.agdata", NO_EDIT);
    std::fs::remove_file(w.photos().join("IMG_1.JPG.agdata")).unwrap();
    scan(&cat);
    let (rev_a, rev_b) = (rev(&cat, a), rev(&cat, b));
    assert_eq!(render(&cat, false).started, 0);
    assert_eq!(shown(&cat, a, Picture::Thumb), w.data.thumb_path(a));
    assert_eq!(shown(&cat, b, Picture::Preview), w.data.preview_path(b));
    assert!(rev(&cat, a) > rev_a && rev(&cat, b) > rev_b, "the window asks for the original pictures again");
    assert!(edited.iter().all(|f| !f.exists()), "their edited pictures are removed");
}

#[test]
fn a_photo_argentum_cannot_render_is_tried_again_only_when_asked_or_edited() {
    let w = World::new("render-failed");
    w.write("IMG_0.JPG.agdata", BROKEN);
    w.write("IMG_1.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let p = photo(&cat, "IMG_0.JPG");
    let r = render(&cat, false);
    assert_eq!((r.started, r.rendered, r.failed), (1, 1, 1), "one failure doesn't hold back the others");
    assert_eq!(edits(&cat, p)[0].error.as_deref(), Some("Failed to load"));
    assert_eq!(shown(&cat, p, Picture::Thumb), w.data.thumb_path(p), "the original shows meanwhile");
    assert_eq!(render(&cat, false), Report::default(), "not tried again on every look");
    let r = render(&cat, true);
    assert_eq!((r.started, r.failed), (1, 1), "tried again when asked (at a start, from the CLI)");
    tick();
    w.write("IMG_0.JPG.agdata", EDIT_2);
    scan(&cat);
    assert_eq!(render(&cat, false).rendered, 1, "a new edit is tried");
    assert_eq!(edits(&cat, p)[0].error, None);
}

#[test]
fn an_unplugged_disk_waits_and_keeps_its_edited_pictures() {
    let w = World::new("render-unplugged");
    w.write("IMG_0.JPG.agdata", EDIT);
    w.write("IMG_1.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let (a, b) = (photo(&cat, "IMG_0.JPG"), photo(&cat, "IMG_1.JPG"));
    let only_a = Options { photo: Some(a), retry: false };
    assert_eq!(render_with(&cat, "new", only_a, &AtomicBool::new(false)).rendered, 1);
    let edited = shown(&cat, a, Picture::Thumb);
    drop(cat);

    let cat = w.open(false);
    let r = render(&cat, false);
    assert_eq!((r.started, r.waiting), (0, 1), "IMG_1 waits for its disk");
    assert_eq!(shown(&cat, a, Picture::Thumb), edited, "IMG_0 keeps its edited picture");
    assert!(edited.is_file());
    assert!(edits(&cat, b)[0].waiting);
}

#[test]
fn each_virtual_copy_is_a_version_of_its_own() {
    let w = World::new("render-copies");
    w.write("IMG_0.JPG.agdata", EDIT);
    w.write("IMG_0.JPG.a1b2c3.agdata", EDIT_2);
    w.write("IMG_1.JPG.d4e5f6.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let r = render(&cat, false);
    assert_eq!((r.started, r.rendered), (1, 3), "one start for all of them");
    let (a, b) = (photo(&cat, "IMG_0.JPG"), photo(&cat, "IMG_1.JPG"));
    let versions: Vec<String> = edits(&cat, a).into_iter().map(|e| e.version.unwrap()).collect();
    assert_eq!(versions.len(), 2);
    assert_ne!(versions[0], versions[1]);
    assert_eq!(version(&shown(&cat, a, Picture::Thumb)), versions[0], "its own edit first");
    for v in &versions {
        assert!(renders::version(&cat, a, v, Picture::Preview).unwrap().is_file());
    }
    let only = edits(&cat, b);
    assert_eq!(only[0].copy.as_deref(), Some("d4e5f6"));
    assert_eq!(Some(version(&shown(&cat, b, Picture::Thumb))), only[0].version);
}

#[test]
fn an_argentum_from_before_batch_export_is_told_apart() {
    assert_eq!(argentum("new").probe(), Support::Ready);
    assert_eq!(argentum("old").probe(), Support::Outdated);
}

#[test]
fn a_stop_ends_argentum_and_counts_nothing_as_failed() {
    let w = World::new("render-stop");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        c.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let r = render_with(&cat, "slow", Options { photo: None, retry: false }, &cancel);
    assert!(started.elapsed() < Duration::from_secs(20), "Argentum is stopped, not waited for");
    assert_eq!((r.rendered, r.failed), (0, 0));
    assert_eq!(render(&cat, false).rendered, 1, "rendered at the next look");
}

#[test]
fn a_private_photo_keeps_its_edited_pictures_sealed() {
    let w = World::new("render-private");
    w.write("IMG_0.JPG.agdata", EDIT);
    w.write("IMG_1.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let (a, b) = (photo(&cat, "IMG_0.JPG"), photo(&cat, "IMG_1.JPG"));
    let only_b = Options { photo: Some(b), retry: false };
    render_with(&cat, "new", only_b, &AtomicBool::new(false));
    private::hide_with_password(&cat, "correct horse", &[a, b], true).unwrap();
    render(&cat, false);
    for p in [a, b] {
        for what in [Picture::Thumb, Picture::Preview] {
            let path = shown(&cat, p, what);
            assert!(path.starts_with(&w.data.root));
            assert!(!path.exists() && DataDir::sealed_path(&path).is_file(), "{path:?} sealed");
        }
    }
}

#[test]
fn argentum_taking_too_long_is_stopped_and_its_photos_tried_again_later() {
    let w = World::new("render-timeout");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let slow = argentum("slow").limited(Duration::from_secs(1), Duration::from_secs(1));
    let started = Instant::now();
    let options = Options { photo: None, retry: false };
    let r = renders::render_pending(&cat, &slow, &options, &AtomicBool::new(false), &|_| {}).unwrap();
    assert!(started.elapsed() < Duration::from_secs(20), "stopped, not waited for");
    assert_eq!((r.rendered, r.failed), (0, 1));
    let p = photo(&cat, "IMG_0.JPG");
    assert_eq!(edits(&cat, p)[0].error.as_deref(), Some("Argentum took too long and was stopped"));
    assert_eq!(render(&cat, true).rendered, 1, "tried again when asked");
}

#[test]
fn a_folder_with_spaces_and_letters_beyond_ascii_reaches_argentum_and_back() {
    let w = World::new("render Été ünï cødé 写真");
    w.write("IMG_0.JPG.agdata", EDIT);
    w.write("IMG_0.JPG.a1b2c3.agdata", EDIT_2);
    let cat = w.open(true);
    scan(&cat);
    let r = render(&cat, false);
    assert_eq!((r.rendered, r.failed), (2, 0));
    let p = photo(&cat, "IMG_0.JPG");
    assert!(edits(&cat, p).iter().all(|e| e.version.is_some()));
}

#[test]
fn an_answer_naming_a_file_outside_argentums_folder_is_refused() {
    let w = World::new("render-escape");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let r = render_with(&cat, "escape", Options { photo: None, retry: false }, &AtomicBool::new(false));
    assert_eq!((r.rendered, r.failed), (0, 1));
    let p = photo(&cat, "IMG_0.JPG");
    assert!(edits(&cat, p)[0].error.as_deref().unwrap().contains("outside its folder"));
    assert_eq!(shown(&cat, p, Picture::Thumb), w.data.thumb_path(p), "the photo itself was never taken as its edit");
    assert!(w.photos().join("IMG_0.JPG").is_file(), "nor touched");
}

#[test]
fn output_that_isnt_an_answer_is_passed_over() {
    let w = World::new("render-noisy");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let r = render_with(&cat, "noisy", Options { photo: None, retry: false }, &AtomicBool::new(false));
    assert_eq!((r.rendered, r.failed), (1, 0), "an endless line and bytes that aren't text hold nothing back");
}
