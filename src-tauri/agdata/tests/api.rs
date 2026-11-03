//! The crate's promises, one test each.

use agdata::{Kind, SidecarName, status};

fn kind(sidecar: &str, is_raw: bool) -> Kind {
    status(sidecar.as_bytes(), is_raw).expect("reads").kind
}

fn fingerprint(sidecar: &str) -> agdata::Fingerprint {
    status(sidecar.as_bytes(), true).expect("reads").fingerprint
}

#[test]
fn a_photo_has_one_sidecar_and_each_virtual_copy_its_own() {
    assert_eq!(agdata::sidecar_name("IMG_1.CR3"), "IMG_1.CR3.agdata");
    assert_eq!(agdata::copy_sidecar_name("IMG_1.CR3", "a1b2c3").as_deref(), Some("IMG_1.CR3.a1b2c3.agdata"));
    assert_eq!(agdata::copy_sidecar_name("IMG_1.CR3", "A1B2C3"), None);
    assert_eq!(agdata::copy_sidecar_name("IMG_1.CR3", "a1b2c"), None);
}

#[test]
fn a_sidecar_name_tells_its_photo_and_copy() {
    assert_eq!(agdata::parse_sidecar_name("IMG_1.CR3.agdata"), Some(SidecarName { photo: "IMG_1.CR3", copy: None }));
    assert_eq!(
        agdata::parse_sidecar_name("IMG_1.CR3.a1b2c3.agdata"),
        Some(SidecarName { photo: "IMG_1.CR3", copy: Some("a1b2c3") })
    );
    // Not six lowercase hex digits: part of the photo's own name.
    assert_eq!(
        agdata::parse_sidecar_name("IMG_1.CR3.a1b2cz.agdata"),
        Some(SidecarName { photo: "IMG_1.CR3.a1b2cz", copy: None })
    );
    assert_eq!(agdata::parse_sidecar_name("IMG_1.CR3"), None);
    assert_eq!(agdata::parse_sidecar_name("IMG_1.CR3agdata"), None);
}

#[test]
fn an_unreadable_sidecar_is_an_error_not_a_panic() {
    for bad in [&b""[..], b"{", b"[]", b"42", b"{\"adjustments\": }", b"\xff\xfe{}", b"{} x"] {
        assert!(status(bad, true).is_err(), "{:?}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn a_part_argentum_cannot_read_spoils_the_sidecar_even_where_skipped() {
    // A lone surrogate, or a number past f64, anywhere: Argentum's reader fails
    // on the whole file and shows the photo unedited.
    for bad in [
        r#"{"version":1,"rating":0,"exif":{"a":"\ud800"},"adjustments":{"exposure":0.5}}"#,
        r#"{"version":1,"rating":0,"agHistory":["\udc00"],"adjustments":{"exposure":0.5}}"#,
        r#"{"version":1,"rating":0,"exif":{"\ud800":"a"},"adjustments":{"exposure":0.5}}"#,
        r#"{"version":1,"rating":0,"exif":{"n":1e999},"adjustments":{"exposure":0.5}}"#,
        r#"{"version":1,"rating":0,"adjustments":{"exposure":0.5,"note":"\ud800"}}"#,
        r#"{"version":1,"rating":0,"adjustments":{"exposure":0.5,"curves":{"luma":"\ud800"}}}"#,
    ] {
        assert!(status(bad.as_bytes(), true).is_err(), "{bad}");
    }
    // A surrogate pair is fine.
    let pair = r#"{"version":1,"rating":0,"exif":{"a":"😀"},"adjustments":{"exposure":0.5}}"#;
    assert_eq!(kind(pair, true), Kind::Edit);
}

#[test]
fn no_edit_is_none() {
    assert_eq!(kind(r#"{"version":1,"rating":3,"adjustments":{}}"#, true), Kind::None);
    assert_eq!(kind(r#"{"version":1,"rating":0,"adjustments":null}"#, false), Kind::None);
    assert_eq!(kind(r#"{"rating":5,"tags":["a"],"flag":"pick"}"#, true), Kind::None);
}

#[test]
fn a_slider_off_its_default_is_an_edit() {
    assert_eq!(kind(r#"{"adjustments":{"exposure":0.5}}"#, true), Kind::Edit);
    assert_eq!(kind(r#"{"adjustments":{"exposure":0}}"#, true), Kind::None);
    assert_eq!(kind(r#"{"adjustments":{"vignetteMidpoint":50}}"#, false), Kind::None);
    assert_eq!(kind(r#"{"adjustments":{"vignetteMidpoint":40}}"#, false), Kind::Edit);
}

#[test]
fn a_closed_section_hides_its_sliders() {
    let hidden = r#"{"adjustments":{"exposure":0.5,"sectionVisibility":{"basic":false}}}"#;
    assert_eq!(kind(hidden, true), Kind::None);
}

#[test]
fn a_provider_look_is_film() {
    let look = r#"{"adjustments":{"lutPath":"provider-look",
        "agProviderLook":{"camera":"cam1","look":"abc"}}}"#;
    assert_eq!(kind(look, true), Kind::Film);
    let other_lut = r#"{"adjustments":{"lutPath":"film.cube",
        "agProviderLook":{"camera":"cam1","look":"abc"}}}"#;
    assert_eq!(kind(other_lut, true), Kind::Edit);
    let taken_off = r#"{"adjustments":{"lutPath":"provider-look","agProviderLook":null}}"#;
    assert_eq!(kind(taken_off, true), Kind::Edit);
}

#[test]
fn the_fingerprint_ignores_key_order_spacing_and_everything_but_the_edit() {
    let a = fingerprint(r#"{"adjustments":{"exposure":0.5,"curves":{"luma":[{"x":0,"y":0}]}}}"#);
    let b = fingerprint(
        r#"{ "rating": 4, "exif": {"Make": "X"}, "agHistory": {"entries": [1, 2]},
            "agSnapshots": [], "tags": ["t"],
            "adjustments": { "curves": { "luma": [ { "y": 0, "x": 0 } ] }, "exposure": 0.5 } }"#,
    );
    assert_eq!(a, b);
}

#[test]
fn the_fingerprint_changes_with_the_edit() {
    let base = fingerprint(r#"{"adjustments":{"exposure":0.5}}"#);
    for other in [
        r#"{"adjustments":{"exposure":0.6}}"#,
        r#"{"adjustments":{"exposure":0.5,"contrast":0}}"#,
        r#"{"adjustments":{"exposure":"0.5"}}"#,
        r#"{"adjustments":{"exposure":[0.5]}}"#,
        r#"{"adjustments":{}}"#,
        r#"{"adjustments":null}"#,
    ] {
        assert_ne!(fingerprint(other), base, "{other}");
    }
    // One and one point zero read differently in Argentum (`as_u64`).
    assert_ne!(
        fingerprint(r#"{"adjustments":{"showClipping":1}}"#),
        fingerprint(r#"{"adjustments":{"showClipping":1.0}}"#)
    );
}

#[test]
fn a_later_duplicate_key_wins_as_in_argentum() {
    let twice = r#"{"adjustments":{"exposure":0.5},"adjustments":{}}"#;
    assert_eq!(kind(twice, true), Kind::None);
    assert_eq!(fingerprint(twice), fingerprint(r#"{"adjustments":{}}"#));
    let inner = r#"{"adjustments":{"exposure":0.5,"exposure":0}}"#;
    assert_eq!(kind(inner, true), Kind::None);
    assert_eq!(fingerprint(inner), fingerprint(r#"{"adjustments":{"exposure":0}}"#));
}

#[test]
fn the_fingerprint_reads_as_hex() {
    let hex = fingerprint(r#"{"adjustments":{}}"#).to_hex();
    assert_eq!(hex.len(), 64);
    assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
}
