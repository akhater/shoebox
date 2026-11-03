//! One pass over a sidecar: the edit (`adjustments`) is hashed as it streams
//! past and the few values the edited rule reads are kept; every other part of
//! the sidecar (EXIF copy, history, snapshots, rating, tags) is skipped
//! without being built, though read as strictly as Argentum reads it: a part
//! Argentum cannot read (a lone surrogate in a string, a number out of range)
//! makes the whole sidecar unreadable here too.

use std::fmt;

use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::Error;
use crate::edited;
use crate::fingerprint::{self, Fingerprint, Hash, Hashed, Key, Members, Scratch};

/// What the edited rule reads of an edit.
#[derive(Default)]
pub(crate) struct Facts {
    /// The edit is a JSON object; Argentum reads nothing else as an edit.
    pub object: bool,
    /// The edit's values for the keys `edited::reads` accepts.
    pub values: Map<String, Value>,
    /// `masks` is an array with something in it.
    pub masks: bool,
    /// `aiPatches` is an array with something in it.
    pub ai_patches: bool,
}

impl Facts {
    /// The edit's value for `key`, null when it has none.
    pub fn get(&self, key: &str) -> &Value {
        self.values.get(key).unwrap_or(&Value::Null)
    }
}

pub(crate) struct Edit {
    pub facts: Facts,
    pub fingerprint: Fingerprint,
}

pub(crate) fn edit(bytes: &[u8]) -> Result<Edit, Error> {
    // Argentum reads a sidecar as text first: one that is not UTF-8 is no edit,
    // even where the bad bytes sit in a part this pass would skip.
    let text = std::str::from_utf8(bytes).map_err(|_| Error::NotText)?;
    let mut de = serde_json::Deserializer::from_str(text);
    let mut scratch = Scratch::new();
    let edit = de.deserialize_map(SidecarVisitor(&mut scratch)).map_err(Error::NotJson)?;
    de.end().map_err(Error::NotJson)?;
    Ok(edit)
}

struct SidecarVisitor<'s>(&'s mut Scratch);

impl<'de> Visitor<'de> for SidecarVisitor<'_> {
    type Value = Edit;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an Argentum sidecar (a JSON object)")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Edit, A::Error> {
        let mut edit = None;
        while let Some(key) = map.next_key::<KeyName>()? {
            match key {
                // A sidecar without one reads as an empty (null) edit.
                KeyName::Adjustments => edit = Some(map.next_value_seed(EditSeed(&mut *self.0))?),
                KeyName::Other => map.next_value::<Skip>().map(drop)?,
            }
        }
        Ok(edit.unwrap_or_else(|| Edit { facts: Facts::default(), fingerprint: Fingerprint::of(&fingerprint::null()) }))
    }
}

/// A top-level key, compared without being copied.
enum KeyName {
    Adjustments,
    Other,
}

impl<'de> serde::Deserialize<'de> for KeyName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = KeyName;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a key")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<KeyName, E> {
                Ok(if v == "adjustments" { KeyName::Adjustments } else { KeyName::Other })
            }
        }
        deserializer.deserialize_identifier(V)
    }
}

/// A value read and dropped. Through `deserialize_any`, so its strings are
/// unescaped and its numbers converted, and fail where Argentum's reader does.
struct Skip;

impl<'de> serde::Deserialize<'de> for Skip {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Skip)
    }
}

impl<'de> Visitor<'de> for Skip {
    type Value = Skip;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Skip, E> {
        Ok(Skip)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Skip, A::Error> {
        while seq.next_element::<Skip>()?.is_some() {}
        Ok(Skip)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Skip, A::Error> {
        while map.next_entry::<Skip, Skip>()?.is_some() {}
        Ok(Skip)
    }
}

/// Reads the edit: hashes all of it, keeps what the rule needs.
struct EditSeed<'s>(&'s mut Scratch);

impl<'de> DeserializeSeed<'de> for EditSeed<'_> {
    type Value = Edit;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Edit, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl EditSeed<'_> {
    /// An edit that is not an object: hashed, and no edit to the rule.
    fn not_an_object(hashed: Hashed) -> Edit {
        Edit { facts: Facts::default(), fingerprint: Fingerprint::of(&hashed) }
    }
}

impl<'de> Visitor<'de> for EditSeed<'_> {
    type Value = Edit;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an edit")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Edit, A::Error> {
        let mut facts = Facts { object: true, ..Facts::default() };
        let s = self.0;
        let mut members = Members::default();
        while let Some(Key(key)) = map.next_key::<Key>()? {
            let read = edited::reads(&key);
            let mut hashed = if read {
                map.next_value_seed(Hash::<true>(&mut *s))?
            } else {
                map.next_value_seed(Hash::<false>(&mut *s))?
            };
            match &*key {
                _ if read => {
                    facts.values.insert(key.to_string(), std::mem::take(&mut hashed.value));
                }
                "masks" => facts.masks = hashed.non_empty_array,
                "aiPatches" => facts.ai_patches = hashed.non_empty_array,
                _ => {}
            }
            members.add(key, hashed);
        }
        let edit = members.finish(s, false);
        Ok(Edit { facts, fingerprint: Fingerprint::of(&edit) })
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Edit, E> {
        Hash::<false>(self.0).visit_unit().map(Self::not_an_object)
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Edit, E> {
        Hash::<false>(self.0).visit_bool(v).map(Self::not_an_object)
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Edit, E> {
        Hash::<false>(self.0).visit_u64(v).map(Self::not_an_object)
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Edit, E> {
        Hash::<false>(self.0).visit_i64(v).map(Self::not_an_object)
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Edit, E> {
        Hash::<false>(self.0).visit_f64(v).map(Self::not_an_object)
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Edit, E> {
        Hash::<false>(self.0).visit_str(v).map(Self::not_an_object)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Edit, A::Error> {
        Hash::<false>(self.0).visit_seq(seq).map(Self::not_an_object)
    }
}
