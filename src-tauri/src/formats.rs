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

/// How a look file's name ends (`IMG_0001.CR3.<name>.lut.png`).
pub const LOOK_END: &str = ".lut.png";

/// A look file: generated beside a photo and going where it goes (organize.rs),
/// never a photo itself.
pub fn is_look(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(LOOK_END)
}

pub fn kind_of(name: &str) -> Option<Kind> {
    // A Mac's note about a file (`._IMG_0001.CR2`), left beside it on a disk that
    // can't keep it itself (FAT, exFAT, a network share): not a photo.
    if cfg!(not(windows)) && name.starts_with("._") {
        return None;
    }
    if is_look(name) {
        return None;
    }
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

/// Folders never worth walking: hidden ones (digiKam's `.dtrash` included, and
/// the `.Trashes` and `.Spotlight-V100` of a Mac's disks), the recycle bin and
/// Windows' own, and the `lost+found` Linux file systems keep at their root
/// (readable by root only).
pub fn skip_dir(name: &str) -> bool {
    name.starts_with('.')
        || name.eq_ignore_ascii_case("$RECYCLE.BIN")
        || name.eq_ignore_ascii_case("System Volume Information")
        || name.eq_ignore_ascii_case("$WINDOWS.~BT")
        || name == "lost+found"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn photos_are_told_by_their_extension() {
        assert_eq!(kind_of("IMG_1.CR3"), Some(Kind::Raw));
        assert_eq!(kind_of("holiday.2026.JPG"), Some(Kind::Image));
        assert_eq!(kind_of("notes.txt"), None);
        assert_eq!(kind_of("README"), None);
    }

    #[test]
    fn a_look_file_beside_a_photo_is_not_a_photo() {
        for name in ["IMG_1.CR3.warm.lut.png", "IMG_1.CR3.warm.LUT.PNG", "scan.lut.png"] {
            assert_eq!(kind_of(name), None, "{name}");
        }
        // Only the whole ending counts.
        assert_eq!(kind_of("salut.png"), Some(Kind::Image));
        assert_eq!(kind_of("IMG_1.lut.jpg"), Some(Kind::Image));
        assert_eq!(kind_of("IMG_1.lut.CR3"), Some(Kind::Raw));
    }

    /// Off Windows, where a disk may carry a Mac's notes beside the photos.
    #[test]
    fn a_macs_note_beside_a_photo_is_not_a_photo_off_windows() {
        let expected = if cfg!(windows) { Some(Kind::Raw) } else { None };
        assert_eq!(kind_of("._IMG_1.CR3"), expected);
        assert_eq!(kind_of("IMG_1._.CR3"), Some(Kind::Raw));
    }

    #[test]
    fn system_folders_are_not_walked() {
        for name in
            [".Trashes", ".Spotlight-V100", ".Trash-1000", "lost+found", "$RECYCLE.BIN", "System Volume Information"]
        {
            assert!(skip_dir(name), "{name}");
        }
        assert!(!skip_dir("Photos") && !skip_dir("lost found") && !skip_dir("2026-10-17 Trip"));
    }
}
