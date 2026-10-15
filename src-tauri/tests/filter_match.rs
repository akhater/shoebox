//! People and tags can be ticked as "any of them" or "all of them": a photo
//! holds several of each, so "Ana and Bob together" is a real question. The
//! filter counts follow: with "all", only who (or what) is with them is left.
//!
//! Lives here, not in src/, because it makes a throwaway catalog folder: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use shoebox_lib::catalog::{Catalog, Filters, Match, Scope, Sort};
use shoebox_lib::paths::DataDir;

fn ids(cat: &Catalog, f: &Filters) -> Vec<i64> {
    let mut ids = cat.list_photos(&Scope::All, Sort::default(), f).unwrap().ids;
    ids.sort();
    ids
}

#[test]
fn people_and_tags_match_any_or_all() {
    let base = std::env::temp_dir().join("shoebox-filter-match");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    // 1: Ana, Paris. 2: Ana and Bob, Paris and Beach. 3: Bob and Cy, Beach. 4: nobody, no tag.
    cat.db()
        .execute_batch(
            "INSERT INTO photos (id, fingerprint, kind, added_at) VALUES
                 (1, 'f1', 'image', 0), (2, 'f2', 'image', 0), (3, 'f3', 'image', 0), (4, 'f4', 'image', 0);
             INSERT INTO people (id, name, created_at) VALUES (1, 'Ana', 0), (2, 'Bob', 0), (3, 'Cy', 0);
             INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score) VALUES
                 (1, 1, 0.1, 0.1, 0.4, 0.4, '', 0.9), (2, 1, 0.1, 0.1, 0.4, 0.4, '', 0.9),
                 (2, 2, 0.5, 0.1, 0.8, 0.4, '', 0.9), (3, 2, 0.1, 0.1, 0.4, 0.4, '', 0.9),
                 (3, 3, 0.5, 0.1, 0.8, 0.4, '', 0.9);
             INSERT INTO tags (id, name, created_at) VALUES (1, 'Paris', 0), (2, 'Beach', 0);
             INSERT INTO photo_tags (photo_id, tag_id, added_at) VALUES (1, 1, 0), (2, 1, 0), (2, 2, 0), (3, 2, 0);",
        )
        .unwrap();

    // People: any of them by default, or together.
    let anyone = Filters { people: vec![1, 2], ..Default::default() };
    assert_eq!(ids(&cat, &anyone), [1, 2, 3]);
    let together = Filters { people_match: Match::All, ..anyone.clone() };
    assert_eq!(ids(&cat, &together), [2]);

    // Tags: all of them by default, or any.
    let both = Filters { tags: vec![1, 2], ..Default::default() };
    assert_eq!(ids(&cat, &both), [2]);
    let either = Filters { tags_match: Match::Any, ..both.clone() };
    assert_eq!(ids(&cat, &either), [1, 2, 3]);

    // Counts: with "any" a tick doesn't narrow its own filter; with "all" only
    // who is in a photo with Ana is offered.
    let people = |f: &Filters| -> Vec<(i64, i64)> {
        cat.facets(&Scope::All, f).unwrap().people.iter().map(|p| (p.id, p.count)).collect()
    };
    let ana = Filters { people: vec![1], ..Default::default() };
    assert_eq!(people(&ana), [(1, 2), (2, 2), (3, 1)]);
    assert_eq!(people(&Filters { people_match: Match::All, ..ana }), [(1, 2), (2, 1)]);
    let tags = |f: &Filters| -> Vec<(i64, i64)> {
        cat.facets(&Scope::All, f).unwrap().tags.iter().map(|t| (t.id, t.count)).collect()
    };
    let paris = Filters { tags: vec![1], ..Default::default() };
    assert_eq!(tags(&paris), [(1, 2), (2, 1)]);
    assert_eq!(tags(&Filters { tags_match: Match::Any, ..paris }), [(2, 2), (1, 2)]);

    // Headless: the same filters as JSON, as `shoebox-cli list`, `facets` and `days` take them.
    let json: Filters =
        serde_json::from_str(r#"{"people":[1,2],"peopleMatch":"all","tags":[1,2],"tagsMatch":"any"}"#).unwrap();
    assert_eq!((json.people_match, json.tags_match), (Match::All, Match::Any));
    assert_eq!(ids(&cat, &json), [2]);
    let plain: Filters = serde_json::from_str(r#"{"tags":[1]}"#).unwrap();
    assert_eq!((plain.people_match, plain.tags_match), (Match::Any, Match::All));

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
