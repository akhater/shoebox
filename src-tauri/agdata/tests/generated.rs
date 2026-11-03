//! The generated corpus, with the answers Argentum itself gave for each
//! sidecar when this crate was checked against it (see the README).
//!
//! Each line of `fixtures/generated.jsonl` is a case: its name, Argentum's mark
//! for a RAW photo (`raw`) and for any other (`other`), and the sidecar, as
//! one of:
//!
//! - `sidecar`: its text;
//! - `edit`: its edit, in a sidecar that holds nothing else;
//! - `on_defaults`: the editor's starting edit (`fixtures/editor-defaults.json`,
//!   Argentum's default values) with keys `set` and others `remove`d.

use std::collections::HashMap;

use agdata::{Kind, status};
use serde_json::{Map, Value, json};

fn sidecar(edit: &Value) -> String {
    serde_json::to_string(&json!({"version": 1, "rating": 0, "adjustments": edit, "tags": []})).unwrap()
}

fn text_of(case: &Value, defaults: &Map<String, Value>) -> String {
    if let Some(text) = case["sidecar"].as_str() {
        return text.to_string();
    }
    if let Some(patch) = case.get("on_defaults") {
        let mut edit = defaults.clone();
        for key in patch["remove"].as_array().expect("remove") {
            edit.remove(key.as_str().expect("key"));
        }
        edit.extend(patch["set"].as_object().expect("set").clone());
        return sidecar(&Value::Object(edit));
    }
    sidecar(&case["edit"])
}

struct Case {
    name: String,
    raw: Kind,
    other: Kind,
    sidecar: String,
}

fn kind(name: &str) -> Kind {
    match name {
        "none" => Kind::None,
        "edit" => Kind::Edit,
        "film" => Kind::Film,
        other => panic!("unknown kind {other}"),
    }
}

fn cases() -> Vec<Case> {
    let defaults: Map<String, Value> =
        serde_json::from_str(include_str!("fixtures/editor-defaults.json")).expect("defaults");
    include_str!("fixtures/generated.jsonl")
        .lines()
        .map(|line| {
            let v: Value = serde_json::from_str(line).expect("fixture line");
            Case {
                name: v["case"].as_str().expect("case").to_string(),
                raw: kind(v["raw"].as_str().expect("raw")),
                other: kind(v["other"].as_str().expect("other")),
                sidecar: text_of(&v, &defaults),
            }
        })
        .collect()
}

fn ours(sidecar: &str, is_raw: bool) -> Kind {
    status(sidecar.as_bytes(), is_raw).map_or(Kind::None, |s| s.kind)
}

#[test]
fn every_case_gets_argentums_answer() {
    let cases = cases();
    assert!(cases.len() > 1000, "{} cases", cases.len());
    let wrong: Vec<String> = cases
        .iter()
        .flat_map(|c| {
            [(true, c.raw), (false, c.other)].into_iter().filter_map(move |(is_raw, expected)| {
                let got = ours(&c.sidecar, is_raw);
                (got != expected).then(|| format!("{} (raw {is_raw}): {got:?}, Argentum {expected:?}", c.name))
            })
        })
        .collect();
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
}

#[test]
fn the_corpus_holds_every_kind() {
    let cases = cases();
    for k in [Kind::None, Kind::Edit, Kind::Film] {
        assert!(cases.iter().any(|c| c.raw == k), "no RAW case is {k:?}");
        assert!(cases.iter().any(|c| c.other == k), "no other case is {k:?}");
    }
    // Some sidecars differ between a RAW and any other photo.
    assert!(cases.iter().any(|c| c.raw != c.other));
}

/// `v` as JSON with every object's keys in reverse order.
fn reversed(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let members: Vec<String> = m
                .iter()
                .rev()
                .map(|(k, v)| format!("{}: {}", serde_json::to_string(k).unwrap(), reversed(v)))
                .collect();
            format!("{{ {} }}", members.join(",\n "))
        }
        Value::Array(a) => format!("[ {} ]", a.iter().map(reversed).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}

#[test]
fn rewriting_all_but_the_edit_keeps_the_fingerprint() {
    let mut checked = 0;
    for c in cases() {
        let Ok(before) = status(c.sidecar.as_bytes(), true) else { continue };
        let Ok(Value::Object(mut sidecar)) = serde_json::from_str::<Value>(&c.sidecar) else { continue };
        if c.sidecar.matches("\"adjustments\"").count() > 1 {
            continue; // Rebuilding it would keep only one.
        }
        sidecar.insert("exif".into(), serde_json::json!({"Make": "Rewritten", "ISO": "100"}));
        sidecar.insert("agHistory".into(), serde_json::json!({"entries": [{"label": "x"}]}));
        sidecar.insert("agSnapshots".into(), serde_json::json!([{"name": "s"}]));
        sidecar.insert("rating".into(), serde_json::json!(5));
        sidecar.insert("tags".into(), serde_json::json!(["t"]));
        sidecar.insert("flag".into(), serde_json::json!("pick"));
        let after = status(reversed(&Value::Object(sidecar)).as_bytes(), true).expect("reads");
        assert_eq!(after, before, "{}", c.name);
        checked += 1;
    }
    assert!(checked > 1000, "{checked}");
}

#[test]
fn the_fingerprint_changes_exactly_when_the_edit_does() {
    // Each distinct edit (as canonical JSON: keys sorted, numbers as written)
    // has one fingerprint, and no two distinct edits share one.
    let mut by_edit: HashMap<String, agdata::Fingerprint> = HashMap::new();
    let mut by_fingerprint: HashMap<agdata::Fingerprint, String> = HashMap::new();
    for c in cases() {
        let Ok(s) = status(c.sidecar.as_bytes(), true) else { continue };
        if c.sidecar.matches("\"adjustments\"").count() > 1 || c.name.starts_with("duplicate") {
            continue; // The text's last value wins; a parsed map keeps another.
        }
        let Ok(v) = serde_json::from_str::<Value>(&c.sidecar) else { continue };
        let edit = serde_json::to_string(&v["adjustments"]).unwrap();
        if let Some(f) = by_edit.insert(edit.clone(), s.fingerprint) {
            assert_eq!(f, s.fingerprint, "{}", c.name);
        }
        if let Some(e) = by_fingerprint.insert(s.fingerprint, edit.clone()) {
            assert_eq!(e, edit, "{}: two edits, one fingerprint", c.name);
        }
    }
    assert!(by_edit.len() > 1000, "{} distinct edits", by_edit.len());
}
