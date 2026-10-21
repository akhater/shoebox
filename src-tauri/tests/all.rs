//! Every integration test, built as one test program.
//!
//! Each file in tests/ is a module of this program rather than a program of
//! its own (`autotests = false` in Cargo.toml): a change to the library then
//! relinks and starts one test executable instead of one per file, which on
//! Windows was most of the time a test run took. One file's tests:
//! `cargo test --test all <file>::`.

mod answers_kept;
mod catalog_memory;
mod catalog_move;
mod cli_faces_reset;
mod cull_assistant;
mod damaged_files;
mod date_pages;
mod dates;
mod days;
mod faces_by_date;
mod faces_saved;
mod failed_photos;
mod filter_match;
mod focus_points;
mod folder_paths;
mod forget_missing;
mod grouping_replay;
mod half_read;
mod kill_safe_organize;
mod look_alikes_join;
mod more_faces;
mod namesakes;
mod people_check;
mod people_ignore;
mod people_read;
mod photo_folder;
mod photoshop;
mod private_photos;
mod reassign_faces;
mod regroup_resumes;
mod scan_copies;
mod search_reused_id;
mod stored_gear_names;
mod suggestions_by_date;
mod suggestions_replay;
mod turns;
mod view_cost;
mod window_never_waits;

/// A file in tests/ that isn't declared above would never be built or run.
#[test]
fn every_test_file_is_declared() {
    let declared = include_str!("all.rs");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut missing: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
        .filter(|name| name != "all" && !declared.contains(&format!("\nmod {name};")))
        .collect();
    missing.sort();
    assert!(missing.is_empty(), "add these to tests/all.rs as `mod <name>;`: {missing:?}");
}
