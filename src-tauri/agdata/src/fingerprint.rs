//! A BLAKE3 hash of the edit's JSON text as the sidecar holds it. Argentum
//! writes an edit the same way each time it holds the same values (keys
//! sorted, numbers in their shortest form, the same indentation), so its text
//! changes exactly when the edit does. The same values written another way
//! (another layout or key order, `1.0` for `1`) hash differently: at worst one
//! needless re-render.

use std::fmt;

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

    /// The fingerprint of `edit`, the text of one JSON value.
    pub(crate) fn of(edit: &str) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key("agdata 2026-11 edit fingerprint v2");
        Self(*hasher.update(edit.as_bytes()).finalize().as_bytes())
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
