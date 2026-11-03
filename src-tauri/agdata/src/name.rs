//! Sidecar names: `<photo>.agdata` for the photo, `<photo>.<id>.agdata` for each
//! virtual copy, the id being six lowercase hex digits.

/// A sidecar's extension, without the dot.
pub const EXTENSION: &str = "agdata";

const COPY_ID_LEN: usize = 6;

/// The photo and virtual copy a sidecar belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidecarName<'a> {
    /// The photo's file name.
    pub photo: &'a str,
    /// The virtual copy's id; `None` for the photo's own edit.
    pub copy: Option<&'a str>,
}

/// The sidecar of the photo named `photo` (a file name, not a path).
pub fn sidecar_name(photo: &str) -> String {
    format!("{photo}.{EXTENSION}")
}

/// The sidecar of virtual copy `id` of the photo named `photo`; `None` when
/// `id` is not a copy id.
pub fn copy_sidecar_name(photo: &str, id: &str) -> Option<String> {
    is_copy_id(id).then(|| format!("{photo}.{id}.{EXTENSION}"))
}

/// The photo and copy a sidecar named `file_name` belongs to; `None` when it is
/// not a sidecar's name. A name ending in `.` and six lowercase hex digits
/// before the extension is a virtual copy's.
pub fn parse_sidecar_name(file_name: &str) -> Option<SidecarName<'_>> {
    let base = file_name.strip_suffix(EXTENSION)?.strip_suffix('.')?;
    if let Some((photo, id)) = base.rsplit_once('.')
        && is_copy_id(id)
    {
        return Some(SidecarName { photo, copy: Some(id) });
    }
    Some(SidecarName { photo: base, copy: None })
}

fn is_copy_id(id: &str) -> bool {
    id.len() == COPY_ID_LEN && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
