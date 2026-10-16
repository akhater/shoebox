//! A year's or a month's page (the sidebar's dates): every photo taken in it,
//! in camera local time, to the second at both ends; undated photos apart.
//!
//! Lives here, not in src/, because it makes a throwaway catalog folder: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use rusqlite::params;
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::paths::DataDir;

#[test]
fn a_year_or_month_holds_its_photos_to_the_second() {
    let base = std::env::temp_dir().join("shoebox-date-pages");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    // A second either side of 2006 and of its first month, and of December;
    // then one without a date.
    let times = [
        ("2005-12-31 23:59:59", 1_136_073_599i64),
        ("2006-01-01 00:00:00", 1_136_073_600),
        ("2006-01-31 23:59:59", 1_138_751_999),
        ("2006-02-01 00:00:00", 1_138_752_000),
        ("2006-12-31 23:59:59", 1_167_609_599),
        ("2007-01-01 00:00:00", 1_167_609_600),
        ("no date", 0),
    ];
    for (name, ts) in times {
        cat.db()
            .execute(
                "INSERT INTO photos (fingerprint, kind, taken_ts, added_at) VALUES (?1, 'raw', NULLIF(?2, 0), 0)",
                params![name, ts],
            )
            .unwrap();
    }
    let page = |key: &str| -> Vec<String> {
        let scope = Scope::Date { key: key.into() };
        let list = cat.list_photos(&scope, Sort::Oldest, &Filters::default()).unwrap();
        // The filters' counts agree with the page.
        let counted: i64 = cat.facets(&scope, &Filters::default()).unwrap().dates.iter().map(|d| d.count).sum();
        assert_eq!(counted, list.ids.len() as i64, "{key}");
        list.ids
            .iter()
            .map(|id| cat.db().query_row("SELECT fingerprint FROM photos WHERE id = ?1", [id], |r| r.get(0)).unwrap())
            .collect()
    };

    assert_eq!(page("2005"), ["2005-12-31 23:59:59"]);
    assert_eq!(
        page("2006"),
        ["2006-01-01 00:00:00", "2006-01-31 23:59:59", "2006-02-01 00:00:00", "2006-12-31 23:59:59"]
    );
    assert_eq!(page("2006-01"), ["2006-01-01 00:00:00", "2006-01-31 23:59:59"]);
    assert_eq!(page("2006-02"), ["2006-02-01 00:00:00"]);
    assert_eq!(page("2006-12"), ["2006-12-31 23:59:59"]);
    assert_eq!(page("2007"), ["2007-01-01 00:00:00"]);
    assert_eq!(page("2007-01"), ["2007-01-01 00:00:00"]);
    assert!(page("2006-03").is_empty());
    assert_eq!(page("unknown"), ["no date"]);
}
