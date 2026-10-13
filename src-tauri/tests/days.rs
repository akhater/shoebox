//! Photos per day, as the calendar shows them (`shoebox-cli days`): counted in
//! camera local time, narrowed by the filters, undated photos apart.
//!
//! Lives here, not in src/, because it makes a throwaway catalog folder: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use rusqlite::params;
use shoebox_lib::catalog::{Catalog, Day, Filters, Scope};
use shoebox_lib::paths::DataDir;

#[test]
fn photos_are_counted_per_day() {
    let base = std::env::temp_dir().join("shoebox-days");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    // 2006-01-14 09:00 and 23:59 (one day, read as camera time), 2006-02-04, no date.
    for (i, ts) in [1_137_229_200i64, 1_137_283_140, 1_139_050_800, 0].iter().enumerate() {
        cat.db()
            .execute(
                "INSERT INTO photos (fingerprint, kind, taken_ts, added_at) VALUES (?1, 'raw', NULLIF(?2, 0), 0)",
                params![format!("fp{i}"), ts],
            )
            .unwrap();
    }
    let day = |d: &str, n| Day { day: d.into(), photos: n };

    let all = cat.days(&Scope::All, &Filters::default()).unwrap();
    assert_eq!(all.days, [day("2006-01-14", 2), day("2006-02-04", 1)]);
    assert_eq!(all.undated, 1);

    let feb = Filters { dates: vec!["2006-02".into()], ..Default::default() };
    let some = cat.days(&Scope::All, &feb).unwrap();
    assert_eq!(some.days, [day("2006-02-04", 1)]);
    assert_eq!(some.undated, 0);
}
