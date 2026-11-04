//! The generated corpus, with the answers Argentum itself gave for each
//! sidecar when this crate was checked against it (see the README).
//!
//! Each line of `fixtures/generated.jsonl` is a case: Argentum's mark for a RAW
//! photo (`raw`) and for any other (`other`), or for both (`both`), and the
//! sidecar, as one of:
//!
//! - `sidecar`: its text, written by hand (with a `case` name saying why);
//! - `edit`: its edit, in a sidecar that holds nothing else;
//! - `defaults`, `alone` or `previous`: JSON pointers to set in an edit, and
//!   any to `remove`, starting from the editor's starting edit
//!   (`fixtures/editor-defaults.json`, Argentum's default values), from the
//!   default values of the keys the pointers name, or from the previous line's
//!   edit.

use std::collections::HashMap;

use agdata::{Kind, status};
use serde_json::{Map, Value, json};

fn sidecar(edit: &Value) -> String {
    serde_json::to_string(&json!({"version": 1, "rating": 0, "adjustments": edit, "tags": []})).unwrap()
}

fn patch(edit: &mut Value, set: &Map<String, Value>, remove: &[Value]) {
    for pointer in remove {
        let (parent, key) = pointer.as_str().and_then(|p| p.rsplit_once('/')).expect("pointer");
        edit.pointer_mut(parent).and_then(Value::as_object_mut).expect("parent").remove(key);
    }
    for (pointer, value) in set {
        match edit.pointer_mut(pointer) {
            Some(slot) => *slot = value.clone(),
            None => {
                let (parent, key) = pointer.rsplit_once('/').expect("pointer");
                let parent = edit.pointer_mut(parent).and_then(Value::as_object_mut).expect("parent");
                parent.insert(key.into(), value.clone());
            }
        }
    }
}

/// A case's edit, built as its line says; `None` for a sidecar written by hand.
fn edit_of(case: &Value, defaults: &Map<String, Value>, previous: Option<&Value>) -> Option<Value> {
    if let Some(edit) = case.get("edit") {
        return Some(edit.clone());
    }
    let remove = case.get("remove").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let (mut edit, set) = if let Some(set) = case.get("defaults") {
        (Value::Object(defaults.clone()), set)
    } else if let Some(set) = case.get("alone") {
        let pointers = set.as_object().expect("set").keys().map(String::as_str);
        let named = pointers.chain(remove.iter().filter_map(Value::as_str)).filter_map(|p| p[1..].split('/').next());
        (Value::Object(named.filter_map(|k| Some((k.to_string(), defaults.get(k)?.clone()))).collect()), set)
    } else {
        (previous.expect("a previous edit").clone(), case.get("previous")?)
    };
    patch(&mut edit, set.as_object().expect("set"), remove);
    Some(edit)
}

struct Case {
    line: usize,
    raw: Kind,
    other: Kind,
    sidecar: String,
    by_hand: bool,
}

fn kind(name: &Value) -> Kind {
    match name.as_str() {
        Some("none") => Kind::None,
        Some("edit") => Kind::Edit,
        Some("film") => Kind::Film,
        other => panic!("unknown kind {other:?}"),
    }
}

fn cases() -> Vec<Case> {
    let defaults: Map<String, Value> =
        serde_json::from_str(include_str!("fixtures/editor-defaults.json")).expect("defaults");
    let mut previous = None;
    let mut cases = Vec::new();
    for (i, line) in include_str!("fixtures/generated.jsonl").lines().enumerate() {
        let v: Value = serde_json::from_str(line).expect("fixture line");
        let edit = if v.get("sidecar").is_some() { None } else { edit_of(&v, &defaults, previous.as_ref()) };
        let sidecar = match &edit {
            Some(edit) => sidecar(edit),
            None => v["sidecar"].as_str().expect("sidecar").to_string(),
        };
        let (raw, other) = match v.get("both") {
            Some(both) => (kind(both), kind(both)),
            None => (kind(&v["raw"]), kind(&v["other"])),
        };
        cases.push(Case { line: i + 1, raw, other, sidecar, by_hand: edit.is_none() });
        previous = edit;
    }
    cases
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
                (got != expected).then(|| format!("line {} (raw {is_raw}): {got:?}, Argentum {expected:?}", c.line))
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

/// `sidecar` as Argentum writes it: through a parsed value, laid out by
/// serde_json's pretty printer.
fn as_argentum_writes(sidecar: &Value) -> String {
    serde_json::to_string_pretty(sidecar).unwrap()
}

#[test]
fn rewriting_all_but_the_edit_keeps_the_fingerprint() {
    let mut checked = 0;
    for c in cases().iter().filter(|c| !c.by_hand) {
        let mut sidecar: Value = serde_json::from_str(&c.sidecar).expect("json");
        let Ok(before) = status(as_argentum_writes(&sidecar).as_bytes(), true) else { continue };
        let s = sidecar.as_object_mut().expect("object");
        s.insert("exif".into(), json!({"Make": "Rewritten", "ISO": "100"}));
        s.insert("agHistory".into(), json!({"entries": [{"label": "x"}]}));
        s.insert("agSnapshots".into(), json!([{"name": "s"}]));
        s.insert("rating".into(), json!(5));
        s.insert("tags".into(), json!(["t"]));
        s.insert("flag".into(), json!("pick"));
        let after = status(as_argentum_writes(&sidecar).as_bytes(), true).expect("reads");
        assert_eq!(after, before, "line {}", c.line);
        checked += 1;
    }
    assert!(checked > 1000, "{checked}");
}

#[test]
fn the_fingerprint_changes_exactly_when_the_edit_does() {
    // Each distinct edit has one fingerprint, and no two distinct edits share one.
    let mut by_edit: HashMap<String, agdata::Fingerprint> = HashMap::new();
    let mut by_fingerprint: HashMap<agdata::Fingerprint, String> = HashMap::new();
    for c in cases().iter().filter(|c| !c.by_hand) {
        let Ok(s) = status(c.sidecar.as_bytes(), true) else { continue };
        let v: Value = serde_json::from_str(&c.sidecar).expect("json");
        let edit = serde_json::to_string(&v["adjustments"]).unwrap();
        if let Some(f) = by_edit.insert(edit.clone(), s.fingerprint) {
            assert_eq!(f, s.fingerprint, "line {}", c.line);
        }
        if let Some(e) = by_fingerprint.insert(s.fingerprint, edit.clone()) {
            assert_eq!(e, edit, "line {}: two edits, one fingerprint", c.line);
        }
    }
    assert!(by_edit.len() > 1000, "{} distinct edits", by_edit.len());
}
