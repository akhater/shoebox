//! What an Argentum sidecar (`.agdata`) says about its photo's edit: none, an
//! edit, or an edit with a film look on (Argentum's two marks), and a
//! fingerprint of the edit that changes only when the edit does.
//!
//! The answer is Argentum's own: the same rule its library uses to show a photo
//! as edited (see the README for how that is kept true), with Argentum's
//! default settings. Written from the sidecar format; it contains no Argentum
//! code.
//!
//! ```
//! let sidecar = br#"{"version":1,"rating":0,"adjustments":{"exposure":0.5}}"#;
//! let status = agdata::status(sidecar, true).unwrap();
//! assert_eq!(status.kind, agdata::Kind::Edit);
//! ```
//!
//! The sidecar is read in one pass: its EXIF copy, history and snapshots are
//! skipped without being built, and of the edit only the few small values the
//! rule needs are kept.

mod edited;
mod fingerprint;
mod name;
mod read;

use std::fmt;

pub use fingerprint::Fingerprint;
pub use name::{EXTENSION, SidecarName, copy_sidecar_name, parse_sidecar_name, sidecar_name};

/// How Argentum marks a photo in its library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// No mark: no edit, or one that leaves the photo as it was.
    None,
    /// The edit mark (sliders).
    Edit,
    /// The film mark: an edit with a provider look on.
    Film,
}

/// What a sidecar says about its photo's edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Status {
    pub kind: Kind,
    /// Changes when the edit changes; stays when only the rest of the sidecar
    /// (rating, tags, EXIF copy, history, snapshots) or its layout does.
    pub fingerprint: Fingerprint,
}

/// A sidecar that does not read as one. Argentum shows its photo unedited.
#[derive(Debug)]
pub enum Error {
    /// Not UTF-8 text.
    NotText,
    /// Not a JSON object.
    NotJson(serde_json::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotText => f.write_str("the sidecar is not UTF-8 text"),
            Error::NotJson(e) => write!(f, "the sidecar is not a JSON object: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::NotText => None,
            Error::NotJson(e) => Some(e),
        }
    }
}

/// What the sidecar `bytes` says about its photo's edit. `is_raw`: whether the
/// photo is a RAW file, which changes what an untouched edit renders as.
pub fn status(bytes: &[u8], is_raw: bool) -> Result<Status, Error> {
    let edit = read::edit(bytes)?;
    Ok(Status { kind: edited::kind(&edit.facts, is_raw), fingerprint: edit.fingerprint })
}
