//! A BLAKE3 hash of a JSON value that depends on what it holds, not on how it
//! is written: key order and spacing are ignored, a key given twice counts
//! once (the later, as Argentum reads it). Numbers keep their JSON kind, since
//! Argentum reads `1` and `1.0` differently in places (`as_u64`).
//!
//! Computed while the value streams past. Each value is reduced to a digest:
//! its canonical bytes while they are short (most numbers, keys and small
//! objects), their BLAKE3 hash once they are not. The canonical bytes of the
//! values being read are written one after the other in a single scratch
//! buffer, each truncated away once its digest is taken, so a sidecar costs one
//! buffer of a few kilobytes and a hash per long value; nothing else of the
//! value is kept but one digest per member of the object being read.
//! The same pass can also build the value (`Hash<true>`), for the few the
//! edited rule reads.

use std::borrow::Cow;
use std::fmt;

use serde::Deserialize;
use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

/// The fingerprint of an edit.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }

    pub(crate) fn of(value: &Hashed) -> Self {
        let mut h = blake3::Hasher::new_derive_key("agdata 2026-11 edit fingerprint v1");
        h.update(value.digest.as_slice());
        Self(*h.finalize().as_bytes())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.to_hex())
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A value read: its digest, whether it is an array with something in it, and
/// the value itself when it was asked for (else null).
pub(crate) struct Hashed {
    pub digest: Digest,
    pub non_empty_array: bool,
    pub value: Value,
}

/// The longest canonical encoding kept as it is.
const SHORT: usize = 64;

/// A value's canonical bytes (a tag, then what it holds) while at most
/// `SHORT` long, else `HASHED` and their BLAKE3 hash. The two cannot be
/// mistaken: a tag is never `HASHED`.
#[derive(Clone, Copy)]
pub(crate) struct Digest {
    len: u8,
    bytes: [u8; SHORT],
}

impl Digest {
    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    fn short(encoding: &[u8]) -> Self {
        let mut bytes = [0; SHORT];
        bytes[..encoding.len()].copy_from_slice(encoding);
        Digest { len: encoding.len() as u8, bytes }
    }

    fn hashed(hash: blake3::Hash) -> Self {
        let mut bytes = [0; SHORT];
        bytes[0] = HASHED;
        bytes[1..33].copy_from_slice(hash.as_bytes());
        Digest { len: 33, bytes }
    }
}

const NULL: u8 = 0;
const BOOL: u8 = 1;
const UNSIGNED: u8 = 2;
const SIGNED: u8 = 3;
const FLOAT: u8 = 4;
const STRING: u8 = 5;
const ARRAY: u8 = 6;
const OBJECT: u8 = 7;
const HASHED: u8 = 0xff;

/// Where the canonical bytes of the values being read are written, innermost
/// last.
pub(crate) struct Scratch(Vec<u8>);

impl Scratch {
    pub fn new() -> Self {
        Scratch(Vec::with_capacity(4096))
    }
}

/// How much of one value's bytes waits in the scratch buffer before going to
/// its hasher in one block.
const BLOCK: usize = 16 * 1024;

/// One array's or object's canonical bytes, from `start` in the scratch buffer.
struct Frame {
    start: usize,
    hasher: Option<Box<blake3::Hasher>>,
}

impl Frame {
    fn open(s: &mut Scratch, tag: u8) -> Frame {
        let start = s.0.len();
        s.0.push(tag);
        Frame { start, hasher: None }
    }

    fn write(&mut self, s: &mut Scratch, data: &[u8]) {
        s.0.extend_from_slice(data);
        if s.0.len() - self.start >= BLOCK {
            let hasher = self.hasher.get_or_insert_with(|| Box::new(blake3::Hasher::new()));
            hasher.update(&s.0[self.start..]);
            s.0.truncate(self.start);
        }
    }

    /// A member's digest, framed by its length.
    fn write_digest(&mut self, s: &mut Scratch, d: &Digest) {
        self.write(s, &[d.len]);
        self.write(s, d.as_slice());
    }

    fn close(self, s: &mut Scratch) -> Digest {
        let bytes = &s.0[self.start..];
        let digest = match self.hasher {
            None if bytes.len() <= SHORT => Digest::short(bytes),
            None => Digest::hashed(blake3::hash(bytes)),
            Some(mut hasher) => Digest::hashed(hasher.update(bytes).finalize()),
        };
        s.0.truncate(self.start);
        digest
    }
}

fn leaf(tag: u8, payload: &[u8]) -> Digest {
    if 1 + payload.len() <= SHORT {
        let mut bytes = [0; SHORT];
        bytes[0] = tag;
        bytes[1..1 + payload.len()].copy_from_slice(payload);
        Digest { len: 1 + payload.len() as u8, bytes }
    } else {
        Digest::hashed(blake3::Hasher::new().update(&[tag]).update(payload).finalize())
    }
}

/// A key, borrowed from the text unless it had to be unescaped.
pub(crate) struct Key<'de>(pub Cow<'de, str>);

impl<'de> Deserialize<'de> for Key<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Key<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a key")
            }
            fn visit_borrowed_str<E: serde::de::Error>(self, v: &'de str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Borrowed(v)))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Owned(v.to_owned())))
            }
        }
        deserializer.deserialize_str(V)
    }
}

/// The members of an object, taken in key order whatever order they came in.
#[derive(Default)]
pub(crate) struct Members<'de>(Vec<(Cow<'de, str>, Hashed)>);

impl<'de> Members<'de> {
    pub fn add(&mut self, key: Cow<'de, str>, value: Hashed) {
        self.0.push((key, value));
    }

    /// The object's digest and, when `keep`, the object itself.
    pub fn finish(mut self, s: &mut Scratch, keep: bool) -> Hashed {
        // Stable: of a key given twice, the later stays last and is kept.
        self.0.sort_by(|a, b| a.0.cmp(&b.0));
        let mut frame = Frame::open(s, OBJECT);
        let mut object = Map::new();
        let mut members = self.0.into_iter().peekable();
        while let Some((key, value)) = members.next() {
            if members.peek().is_some_and(|next| next.0 == key) {
                continue;
            }
            frame.write(s, &(key.len() as u64).to_le_bytes());
            frame.write(s, key.as_bytes());
            frame.write_digest(s, &value.digest);
            if keep {
                object.insert(key.into_owned(), value.value);
            }
        }
        let value = if keep { Value::Object(object) } else { Value::Null };
        Hashed { digest: frame.close(s), non_empty_array: false, value }
    }
}

/// Hashes the value it is given and, when `KEEP`, builds it too.
pub(crate) struct Hash<'s, const KEEP: bool>(pub &'s mut Scratch);

impl<'de, const KEEP: bool> DeserializeSeed<'de> for Hash<'_, KEEP> {
    type Value = Hashed;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Hashed, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<const KEEP: bool> Hash<'_, KEEP> {
    fn leaf(tag: u8, payload: &[u8], value: impl FnOnce() -> Value) -> Hashed {
        Hashed { digest: leaf(tag, payload), non_empty_array: false, value: if KEEP { value() } else { Value::Null } }
    }
}

impl<'de, const KEEP: bool> Visitor<'de> for Hash<'_, KEEP> {
    type Value = Hashed;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Hashed, E> {
        Ok(Self::leaf(NULL, &[], || Value::Null))
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Hashed, E> {
        self.visit_unit()
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Hashed, E> {
        Ok(Self::leaf(BOOL, &[v as u8], || Value::Bool(v)))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Hashed, E> {
        Ok(Self::leaf(UNSIGNED, &v.to_le_bytes(), || Value::from(v)))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Hashed, E> {
        Ok(Self::leaf(SIGNED, &v.to_le_bytes(), || Value::from(v)))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Hashed, E> {
        Ok(Self::leaf(FLOAT, &v.to_bits().to_le_bytes(), || Value::from(v)))
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Hashed, E> {
        Ok(Self::leaf(STRING, v.as_bytes(), || Value::String(v.to_owned())))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Hashed, A::Error> {
        let s = self.0;
        let mut frame = Frame::open(s, ARRAY);
        let mut items = Vec::new();
        let mut len = 0u64;
        while let Some(item) = seq.next_element_seed(Hash::<KEEP>(&mut *s))? {
            frame.write_digest(s, &item.digest);
            if KEEP {
                items.push(item.value);
            }
            len += 1;
        }
        frame.write(s, &len.to_le_bytes());
        let value = if KEEP { Value::Array(items) } else { Value::Null };
        Ok(Hashed { digest: frame.close(s), non_empty_array: len > 0, value })
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Hashed, A::Error> {
        let s = self.0;
        let mut members = Members::default();
        while let Some(Key(key)) = map.next_key::<Key>()? {
            let value = map.next_value_seed(Hash::<KEEP>(&mut *s))?;
            members.add(key, value);
        }
        Ok(members.finish(s, KEEP))
    }
}

/// The hash of `null`.
pub(crate) fn null() -> Hashed {
    Hashed { digest: leaf(NULL, &[]), non_empty_array: false, value: Value::Null }
}
