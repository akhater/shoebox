//! Which photos Argentum reads as RAW files, by their name: what
//! [`crate::status`]'s `is_raw` must be.

/// The extensions of the files Argentum reads as RAW, lower case.
const RAW_EXTENSIONS: [&str; 31] = [
    "3fr", "ari", "arw", "bay", "cr2", "cr3", "crw", "dcr", "dcs", "dng", "erf", "fff", "iiq", "k25", "kdc", "mef",
    "mos", "mrw", "nef", "nrw", "orf", "pef", "pro", "ptx", "raf", "raw", "rw2", "rwl", "sr2", "srf", "srw",
];

/// Whether Argentum reads the photo named `file_name` as a RAW file: its
/// extension, in any case, is one of its RAW extensions. A `.tif` or `.tiff`
/// that is a camera's RAW reads as one in Argentum too; by name alone it does
/// not here.
pub fn is_raw(file_name: &str) -> bool {
    file_name.rsplit_once('.').is_some_and(|(_, ext)| RAW_EXTENSIONS.iter().any(|raw| raw.eq_ignore_ascii_case(ext)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_files_are_told_by_their_extension_in_any_case() {
        assert!(RAW_EXTENSIONS.windows(2).all(|w| w[0] < w[1]));
        assert!(is_raw("IMG_1.CR3") && is_raw("img_1.cr3") && is_raw("a.b.Pro"));
        assert!(!is_raw("IMG_1.JPG") && !is_raw("IMG_1.tif") && !is_raw("cr3") && !is_raw("IMG_1.CR3.agdata"));
    }
}
