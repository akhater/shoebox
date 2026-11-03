//! A BLAKE3 hash of a JSON value that depends on what it holds, not on how it
//! is written: key order and spacing are ignored, a key given twice counts
//! once (the later, as Argentum reads it). Numbers keep their JSON kind, since
//! Argentum reads `1` and `1.0` differently in places (`as_u64`).
//!
//! Computed while the value streams past. Each value is reduced to a digest:
//! its canonical bytes while they are short (most numbers, keys and small
//! objects), their BLAKE3 hash once they are not, so a sidecar costs a few
//! hashers rather than one per number. Nothing else of the value is kept but
//! one digest per member of the object being read.

use std::borrow::Cow;
use std::fmt;

use serde::Deserialize;
use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

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

    pub(crate) fn of(value: Hashed) -> Self {
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

/// A value's digest, and whether it is an array with something in it.
#[derive(Clone, Copy)]
pub(crate) struct Hashed {
    pub digest: Digest,
    pub non_empty_array: bool,
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

/// Canonical bytes, kept while short and hashed from the moment they are not.
enum Sink {
    Short(usize, [u8; SHORT]),
    Long(Box<Spill>),
}

/// A hasher fed in blocks: BLAKE3 is fast on kilobytes, slow on a byte at a time.
struct Spill {
    hasher: blake3::Hasher,
    len: usize,
    block: [u8; BLOCK],
}

const BLOCK: usize = 1024;

impl Spill {
    fn write(&mut self, data: &[u8]) {
        if self.len + data.len() > BLOCK {
            self.hasher.update(&self.block[..self.len]);
            self.len = 0;
        }
        if data.len() >= BLOCK {
            self.hasher.update(data);
        } else {
            self.block[self.len..self.len + data.len()].copy_from_slice(data);
            self.len += data.len();
        }
    }

    fn finish(mut self) -> blake3::Hash {
        self.hasher.update(&self.block[..self.len]);
        self.hasher.finalize()
    }
}

impl Sink {
    fn new(tag: u8) -> Self {
        let mut bytes = [0; SHORT];
        bytes[0] = tag;
        Sink::Short(1, bytes)
    }

    fn write(&mut self, data: &[u8]) {
        match self {
            Sink::Short(len, bytes) if *len + data.len() <= SHORT => {
                bytes[*len..*len + data.len()].copy_from_slice(data);
                *len += data.len();
            }
            Sink::Short(len, bytes) => {
                let mut spill = Box::new(Spill { hasher: blake3::Hasher::new(), len: 0, block: [0; BLOCK] });
                spill.write(&bytes[..*len]);
                spill.write(data);
                *self = Sink::Long(spill);
            }
            Sink::Long(spill) => spill.write(data),
        }
    }

    /// A member's digest, framed by its length.
    fn write_digest(&mut self, d: &Digest) {
        self.write(&[d.len]);
        self.write(d.as_slice());
    }

    fn finish(self, non_empty_array: bool) -> Hashed {
        let digest = match self {
            Sink::Short(len, bytes) => Digest { len: len as u8, bytes },
            Sink::Long(spill) => {
                let mut bytes = [0; SHORT];
                bytes[0] = HASHED;
                bytes[1..33].copy_from_slice(spill.finish().as_bytes());
                Digest { len: 33, bytes }
            }
        };
        Hashed { digest, non_empty_array }
    }
}

fn leaf(tag: u8, payload: &[u8]) -> Hashed {
    let mut sink = Sink::new(tag);
    sink.write(payload);
    sink.finish(false)
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
pub(crate) struct Members<'de>(Vec<(Cow<'de, str>, Digest)>);

impl<'de> Members<'de> {
    pub fn add(&mut self, key: Cow<'de, str>, value: Hashed) {
        self.0.push((key, value.digest));
    }

    pub fn finish(mut self) -> Hashed {
        // Stable: of a key given twice, the later stays last and is kept.
        self.0.sort_by(|a, b| a.0.cmp(&b.0));
        let mut sink = Sink::new(OBJECT);
        let members = &self.0;
        for (i, (key, digest)) in members.iter().enumerate() {
            if members.get(i + 1).is_some_and(|next| next.0 == *key) {
                continue;
            }
            sink.write(&(key.len() as u64).to_le_bytes());
            sink.write(key.as_bytes());
            sink.write_digest(digest);
        }
        sink.finish(false)
    }
}

/// Hashes the value it is given.
#[derive(Clone, Copy)]
pub(crate) struct Hash;

impl<'de> DeserializeSeed<'de> for Hash {
    type Value = Hashed;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Hashed, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Hash {
    type Value = Hashed;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Hashed, E> {
        Ok(leaf(NULL, &[]))
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Hashed, E> {
        Ok(leaf(NULL, &[]))
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Hashed, E> {
        Ok(leaf(BOOL, &[v as u8]))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Hashed, E> {
        Ok(leaf(UNSIGNED, &v.to_le_bytes()))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Hashed, E> {
        Ok(leaf(SIGNED, &v.to_le_bytes()))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Hashed, E> {
        Ok(leaf(FLOAT, &v.to_bits().to_le_bytes()))
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Hashed, E> {
        Ok(leaf(STRING, v.as_bytes()))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Hashed, A::Error> {
        let mut sink = Sink::new(ARRAY);
        let mut len = 0u64;
        while let Some(item) = seq.next_element_seed(Hash)? {
            sink.write_digest(&item.digest);
            len += 1;
        }
        sink.write(&len.to_le_bytes());
        Ok(sink.finish(len > 0))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Hashed, A::Error> {
        let mut members = Members::default();
        while let Some(Key(key)) = map.next_key::<Key>()? {
            let value = map.next_value_seed(Hash)?;
            members.add(key, value);
        }
        Ok(members.finish())
    }
}

/// The hash of `null`.
pub(crate) fn null() -> Hashed {
    leaf(NULL, &[])
}
