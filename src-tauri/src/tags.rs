//! Tags: the user's own keywords on photos ("Paris", "sunset", "a birthday").
//!
//! A tag hangs off the photo - its content, not its path - so it survives
//! moves, copies and renames, and a copy found on another disk already has
//! it. Tags live in the catalog only (backed up with it); nothing is ever
//! written into a photo file. Flat for now; the table can nest later.

use crate::catalog::Catalog;
use crate::db;
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: i64,
    pub name: String,
    /// Photos with it (in the whole catalog, or in the photos asked about).
    pub count: i64,
}

/// A tag name as typed: trimmed, inner spaces collapsed.
fn clean(name: &str) -> Option<String> {
    let n = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!n.is_empty() && n.chars().count() <= 100).then_some(n)
}

/// Every tag, most used first (unused ones too, so they're still suggested).
pub fn all(cat: &Catalog) -> Result<Vec<Tag>> {
    let db = cat.db();
    let tags = db
        .prepare(
            "SELECT t.id, t.name, COUNT(pt.photo_id) FROM tags t LEFT JOIN photo_tags pt ON pt.tag_id = t.id
              GROUP BY t.id ORDER BY COUNT(pt.photo_id) DESC, t.name COLLATE NOCASE",
        )?
        .query_map([], |r| Ok(Tag { id: r.get(0)?, name: r.get(1)?, count: r.get(2)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(tags)
}

/// The tags on some photos; `count` = how many of them have it.
pub fn on(cat: &Catalog, photo_ids: &[i64]) -> Result<Vec<Tag>> {
    let db = cat.db();
    let tags = db
        .prepare(
            "SELECT t.id, t.name, COUNT(*) FROM photo_tags pt JOIN tags t ON t.id = pt.tag_id
              WHERE pt.photo_id IN (SELECT value FROM json_each(?1))
              GROUP BY t.id ORDER BY t.name COLLATE NOCASE",
        )?
        .query_map([serde_json::to_string(photo_ids)?], |r| {
            Ok(Tag { id: r.get(0)?, name: r.get(1)?, count: r.get(2)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(tags)
}

/// Puts tags on photos, making the tags that don't exist yet (a name that
/// differs only in case is the same tag). Returns the tags used.
pub fn add(cat: &Catalog, photo_ids: &[i64], names: &[String]) -> Result<Vec<Tag>> {
    let names: Vec<String> = names.iter().filter_map(|n| clean(n)).collect();
    if names.is_empty() {
        bail!("A tag needs a name.");
    }
    let now = db::now();
    let mut conn = cat.db();
    let tx = conn.transaction()?;
    let mut used = Vec::new();
    for name in names {
        let id: i64 = match tx
            .query_row("SELECT id FROM tags WHERE parent_id IS NULL AND name = ?1 COLLATE NOCASE", [&name], |r| {
                r.get(0)
            })
            .optional()?
        {
            Some(id) => id,
            None => {
                tx.execute("INSERT INTO tags (name, created_at) VALUES (?1, ?2)", params![name, now])?;
                tx.last_insert_rowid()
            }
        };
        for pid in photo_ids {
            tx.execute(
                "INSERT OR IGNORE INTO photo_tags (photo_id, tag_id, added_at) VALUES (?1, ?2, ?3)",
                params![pid, id, now],
            )?;
        }
        let name: String = tx.query_row("SELECT name FROM tags WHERE id = ?1", [id], |r| r.get(0))?;
        used.push(Tag { id, name, count: photo_ids.len() as i64 });
    }
    tx.commit()?;
    Ok(used)
}

/// Takes tags off photos. The tags themselves stay (still suggested).
pub fn remove(cat: &Catalog, photo_ids: &[i64], tag_ids: &[i64]) -> Result<usize> {
    let mut db = cat.db();
    let tx = db.transaction()?;
    let mut n = 0;
    for pid in photo_ids {
        for tid in tag_ids {
            n += tx.execute("DELETE FROM photo_tags WHERE photo_id = ?1 AND tag_id = ?2", params![pid, tid])?;
        }
    }
    tx.commit()?;
    Ok(n)
}

/// Renames a tag everywhere. Renaming onto an existing name merges the two.
pub fn rename(cat: &Catalog, tag_id: i64, name: &str) -> Result<()> {
    let Some(name) = clean(name) else { bail!("A tag needs a name.") };
    let mut db = cat.db();
    let tx = db.transaction()?;
    let other: Option<i64> = tx
        .query_row(
            "SELECT id FROM tags WHERE parent_id IS NULL AND name = ?1 COLLATE NOCASE AND id <> ?2",
            params![name, tag_id],
            |r| r.get(0),
        )
        .optional()?;
    match other {
        Some(into) => {
            tx.execute(
                "INSERT OR IGNORE INTO photo_tags (photo_id, tag_id, added_at)
                 SELECT photo_id, ?1, added_at FROM photo_tags WHERE tag_id = ?2",
                params![into, tag_id],
            )?;
            tx.execute("DELETE FROM tags WHERE id = ?1", [tag_id])?;
        }
        None => {
            tx.execute("UPDATE tags SET name = ?1 WHERE id = ?2", params![name, tag_id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Deletes a tag from every photo and from the list.
pub fn delete(cat: &Catalog, tag_id: i64) -> Result<()> {
    cat.db().execute("DELETE FROM tags WHERE id = ?1", [tag_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn names_are_tidied() {
        assert_eq!(clean("  Mum's   birthday ").as_deref(), Some("Mum's birthday"));
        assert_eq!(clean("   "), None);
        assert_eq!(clean(&"x".repeat(101)), None);
    }
}
