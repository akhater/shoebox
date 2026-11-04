//! Sidecar file names: `<photo>.agdata` holds a photo's edit, and
//! `<photo>.<id>.agdata` the edit of one of its virtual copies, where `id` is
//! six lowercase hex digits.

/// The sidecar's file extension, without the dot.
pub const EXTENSION: &str = "agdata";

/// The end of every sidecar's name.
const ENDING: &str = ".agdata";

/// What a sidecar's file name says: the photo it belongs to and, for a virtual
/// copy, the copy's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidecarName<'a> {
    /// The photo's file name.
    pub photo: &'a str,
    /// The virtual copy's id; `None` for the photo's own edit.
    pub copy: Option<&'a str>,
}

/// The name of the sidecar holding the edit of the photo named `photo`.
pub fn sidecar_name(photo: &str) -> String {
    format!("{photo}{ENDING}")
}

/// The photo and copy a sidecar's file name stands for; `None` when the name is
/// not a sidecar's. A last part that is not a copy id is part of the photo's
/// own name.
pub fn parse_sidecar_name(file_name: &str) -> Option<SidecarName<'_>> {
    let base = file_name.strip_suffix(ENDING)?;
    // A copy id holds no dot, so the last dot is the one before it.
    Some(match base.rsplit_once('.') {
        Some((photo, id)) if is_copy_id(id) => SidecarName { photo, copy: Some(id) },
        _ => SidecarName { photo: base, copy: None },
    })
}

/// Six characters, each a digit or a lowercase `a` to `f`.
fn is_copy_id(id: &str) -> bool {
    id.len() == 6 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
