//! `shoebox-cli eyes` with several photos prints one line of JSON per photo,
//! in order, and a photo it can't read says why on its own line instead of
//! stopping the others.
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::paths::DataDir;
use std::process::Command;

#[test]
fn eyes_of_several_photos_is_a_line_each() {
    let base = std::env::temp_dir().join("shoebox-cli-eyes");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let models = base.join("no-models");
    std::fs::create_dir_all(&models).unwrap();
    Catalog::open(data.clone()).unwrap();
    let eyes = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_shoebox-cli"))
            .arg("eyes")
            .args(args)
            .env("SHOEBOX_DATA", &data.root)
            .env("SHOEBOX_MODELS", &models)
            .output()
            .unwrap()
    };

    let out = eyes(&["7", "3"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<serde_json::Value> =
        String::from_utf8(out.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    for (line, id) in lines.iter().zip([7, 3]) {
        assert_eq!(line["id"], id);
        assert!(line["error"].is_string(), "{line}");
    }

    // One photo fails as before; none is a usage error.
    assert!(!eyes(&["7"]).status.success());
    assert!(!eyes(&[]).status.success());
    assert!(!eyes(&["seven"]).status.success());

    let _ = std::fs::remove_dir_all(&base);
}
