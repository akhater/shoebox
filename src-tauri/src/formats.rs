//! Which files are photos.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Raw,
    Image,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Raw => "raw",
            Kind::Image => "image",
        }
    }
}

/// Every RAW extension known to rawler, LibRaw or the Windows RAW codec.
const RAW_EXT: &[&str] = &[
    "3fr", "ari", "arw", "bay", "braw", "cap", "cr2", "cr3", "crw", "cs1", "dcr", "dcs", "dng", "drf", "eip", "erf",
    "fff", "gpr", "iiq", "k25", "kdc", "mdc", "mef", "mos", "mrw", "nef", "nrw", "orf", "ori", "pef", "ptx", "pxn",
    "qtk", "raf", "raw", "rwl", "rw2", "rwz", "sr2", "srf", "srw", "sti", "x3f",
];

/// `hif` is Canon's HEIF extension; `psb` is Photoshop's large document.
const IMAGE_EXT: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "tif", "tiff", "heic", "heif", "hif", "avif", "webp", "bmp", "gif", "jxl",
    "psd", "psb",
];

pub fn kind_of(name: &str) -> Option<Kind> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if RAW_EXT.contains(&ext.as_str()) {
        Some(Kind::Raw)
    } else if IMAGE_EXT.contains(&ext.as_str()) {
        Some(Kind::Image)
    } else {
        None
    }
}

pub fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Folders never worth walking: hidden ones (digiKam's `.dtrash` included),
/// the recycle bin and Windows' own.
pub fn skip_dir(name: &str) -> bool {
    name.starts_with('.')
        || name.eq_ignore_ascii_case("$RECYCLE.BIN")
        || name.eq_ignore_ascii_case("System Volume Information")
        || name.eq_ignore_ascii_case("$WINDOWS.~BT")
}
