//! macOS: the mounted volumes come from `mount`; each volume's UUID, name and size from `diskutil`.
//!
//! Since macOS 10.15 the system volume (`/`) is read-only and the user's files live on the Data volume, mounted at
//! `/System/Volumes/Data` and joined to the system volume by firmlinks (`/Users`, `/Applications`, ...). That Data
//! volume is reported as the volume mounted at `/`, which is where the user sees their files.

use super::{DiskInfo, MountEntry, hash_u32, with_trailing_slash};
use std::process::Command;

/// Where macOS mounts the Data volume.
const DATA_MOUNT: &str = "/System/Volumes/Data/";

/// File systems that aren't disks, even when macOS lists a `/dev/` source for them.
const NOT_DISKS: &[&str] = &["devfs", "autofs", "nullfs", "fdesc", "volfs", "synthfs", "mtmfs"];

/// Why a volume can't be used when `diskutil` gives it no UUID.
pub(super) const NO_ID: &str = "macOS reports no volume UUID for it";

/// Every mount `mount` lists, with the Data volume standing in for `/`.
pub(super) fn all_mounts() -> Vec<MountEntry> {
    let Some(text) = super::output_of(&mut Command::new("/sbin/mount")) else { return Vec::new() };
    fold_data_volume(parse_mount_output(&text))
}

/// A volume the user can put photos on: a disk of this Mac (not a network share, not a system or hidden volume).
pub(super) fn is_local_disk(entry: &MountEntry) -> bool {
    entry.source.starts_with("/dev/")
        && !NOT_DISKS.contains(&entry.fstype.as_str())
        && !has_option(&entry.options, "nobrowse")
}

/// UUID, name and size of the volume, from `diskutil info` about its device.
pub(super) fn disk_info(entry: &MountEntry) -> Option<DiskInfo> {
    let text = super::output_of(Command::new("/usr/sbin/diskutil").args(["info", "-plist"]).arg(&entry.source))?;
    parse_disk_info(&text)
}

/// A path given through the Data volume's own mount point, written the way the user sees it (`/Users/...`).
pub(super) fn normalise(path: &str) -> String {
    match path.strip_prefix("/System/Volumes/Data") {
        Some("") => "/".to_string(),
        Some(rest) if rest.starts_with('/') => rest.to_string(),
        _ => path.to_string(),
    }
}

/// A number standing for the volumes shown in `/Volumes`, which changes when a disk is plugged in or out;
/// 0 if the folder can't be read.
pub(super) fn drive_mask() -> u32 {
    let Ok(dir) = std::fs::read_dir("/Volumes") else { return 0 };
    mask_of_names(dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
}

/// The same set of names always gives the same number, in whatever order they are listed.
fn mask_of_names(mut names: Vec<String>) -> u32 {
    names.sort();
    hash_u32(names.join("\0").as_bytes())
}

/// The mounts in the output of `mount`: one `<from> on <mount point> (<type>, <option>, ...)` per line.
fn parse_mount_output(text: &str) -> Vec<MountEntry> {
    text.lines().filter_map(parse_mount_line).collect()
}

/// A mount point may hold spaces and parentheses, so the line is cut at the first ` on ` (a device name has none)
/// and at the last ` (` (the options hold none).
fn parse_mount_line(line: &str) -> Option<MountEntry> {
    let (source, rest) = line.trim_end().split_once(" on ")?;
    let (mount, settings) = rest.rsplit_once(" (")?;
    let mut settings = settings.strip_suffix(')')?.split(", ");
    let fstype = settings.next()?.trim().to_lowercase();
    let options = settings.collect::<Vec<_>>().join(",");
    if source.is_empty() || mount.is_empty() || fstype.is_empty() {
        return None;
    }
    Some(MountEntry {
        mount: with_trailing_slash(mount),
        source: source.to_string(),
        fstype,
        root: "/".to_string(),
        options,
    })
}

/// When the Data volume is mounted, it is the volume at `/` and the read-only system volume is left out.
fn fold_data_volume(mounts: Vec<MountEntry>) -> Vec<MountEntry> {
    let is_data = |e: &MountEntry| e.mount == DATA_MOUNT && e.source.starts_with("/dev/");
    if !mounts.iter().any(is_data) {
        return mounts;
    }
    mounts
        .into_iter()
        .filter_map(|e| {
            if is_data(&e) {
                // The Data volume is mounted hidden; as the volume at `/` it is the one to show.
                let options = without_option(&e.options, "nobrowse");
                Some(MountEntry { mount: "/".to_string(), options, ..e })
            } else if e.mount == "/" {
                None
            } else {
                Some(e)
            }
        })
        .collect()
}

fn has_option(options: &str, name: &str) -> bool {
    options.split(',').any(|o| o == name)
}

fn without_option(options: &str, name: &str) -> String {
    options.split(',').filter(|o| *o != name).collect::<Vec<_>>().join(",")
}

/// UUID, name and size from the property list `diskutil info -plist` prints. Without a UUID (or a size) the
/// volume can't be told apart from other disks.
fn parse_disk_info(plist: &str) -> Option<DiskInfo> {
    let uuid = plist_value(plist, "VolumeUUID").filter(|u| !u.trim().is_empty())?;
    let size = plist_value(plist, "TotalSize").or_else(|| plist_value(plist, "Size"))?.trim().parse::<u64>().ok()?;
    if size == 0 {
        return None;
    }
    let label = plist_value(plist, "VolumeName").unwrap_or_default();
    Some(DiskInfo { uuid: uuid.trim().to_string(), label, size })
}

/// The value of the first `<key>` called `key` in an XML property list, as text: a string, a number, `true` or
/// `false`. `None` if there is no such key or its value is a dictionary, an array or data.
fn plist_value(plist: &str, key: &str) -> Option<String> {
    let marker = format!("<key>{key}</key>");
    let after = plist[plist.find(&marker)? + marker.len()..].trim_start();
    let tag_and_rest = after.strip_prefix('<')?;
    let tag_end = tag_and_rest.find('>')?;
    let tag = &tag_and_rest[..tag_end];
    if let Some(empty) = tag.strip_suffix('/') {
        // `<true/>`, `<false/>`, `<string/>`
        return match empty {
            "true" | "false" => Some(empty.to_string()),
            "string" => Some(String::new()),
            _ => None,
        };
    }
    if !matches!(tag, "string" | "integer" | "real" | "date") {
        return None;
    }
    let body = &tag_and_rest[tag_end + 1..];
    let close = format!("</{tag}>");
    Some(xml_unescape(&body[..body.find(&close)?]))
}

/// Replaces `&amp;`, `&lt;`, `&gt;`, `&quot;`, `&apos;` and numeric references (`&#233;`, `&#xE9;`).
fn xml_unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let decoded = tail.find(';').and_then(|end| entity(&tail[1..end]).map(|c| (c, end)));
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mount` on a Mac with one external exFAT disk (its name holds spaces and parentheses), one SMB share, and
    /// the volumes macOS keeps hidden.
    const MOUNT_OUTPUT: &str = "\
/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)
devfs on /dev (devfs, local, nobrowse)
/dev/disk3s6 on /System/Volumes/VM (apfs, local, noexec, journaled, noatime, nobrowse)
/dev/disk3s2 on /System/Volumes/Preboot (apfs, local, journaled, nobrowse)
/dev/disk3s4 on /System/Volumes/Update (apfs, local, journaled, nobrowse)
/dev/disk1s2 on /System/Volumes/xarts (apfs, local, noexec, journaled, noatime, nobrowse)
/dev/disk1s1 on /System/Volumes/iSCPreboot (apfs, local, journaled, nobrowse)
/dev/disk1s3 on /System/Volumes/Hardware (apfs, local, journaled, nobrowse)
/dev/disk3s5 on /System/Volumes/Data (apfs, local, journaled, nobrowse, protect, root data)
map auto_home on /System/Volumes/Data/home (autofs, automounted, nobrowse)
/dev/disk4s2 on /Volumes/My Photos (2024) (exfat, local, nodev, nosuid, noowners)
//user@nas._smb._tcp.local/photos on /Volumes/photos (smbfs, nodev, nosuid, mounted by user)
";

    fn entry(mount: &str, source: &str, fstype: &str, options: &str) -> MountEntry {
        MountEntry {
            mount: mount.to_string(),
            source: source.to_string(),
            fstype: fstype.to_string(),
            root: "/".to_string(),
            options: options.to_string(),
        }
    }

    fn disks(text: &str) -> Vec<(String, String)> {
        fold_data_volume(parse_mount_output(text))
            .into_iter()
            .filter(is_local_disk)
            .map(|e| (e.mount, e.source))
            .collect()
    }

    #[test]
    fn a_mount_line_is_cut_into_its_parts() {
        assert_eq!(
            parse_mount_line("/dev/disk4s1 on /Volumes/Photos (exfat, local, nodev, nosuid, noowners)"),
            Some(entry("/Volumes/Photos/", "/dev/disk4s1", "exfat", "local,nodev,nosuid,noowners"))
        );
        assert_eq!(
            parse_mount_line("/dev/disk1s1 on / (apfs, local, journaled)"),
            Some(entry("/", "/dev/disk1s1", "apfs", "local,journaled"))
        );
    }

    #[test]
    fn a_mount_point_may_hold_spaces_parentheses_and_the_word_on() {
        let line = "/dev/disk4s2 on /Volumes/Shot on (Nikon) (2024) (ntfs, local, read-only)";
        let parsed = parse_mount_line(line).unwrap();
        assert_eq!(parsed.mount, "/Volumes/Shot on (Nikon) (2024)/");
        assert_eq!(parsed.source, "/dev/disk4s2");
        assert_eq!(parsed.fstype, "ntfs");
        assert_eq!(parsed.options, "local,read-only");
    }

    #[test]
    fn the_file_system_type_is_lowercase() {
        let parsed = parse_mount_line("/dev/disk5s1 on /Volumes/X (UFSD_NTFS, local)").unwrap();
        assert_eq!(parsed.fstype, "ufsd_ntfs");
    }

    #[test]
    fn lines_that_are_not_mounts_are_skipped() {
        for line in ["", "garbage", "/dev/disk1s1 on / apfs", "on /x (apfs)", "/dev/disk1s1 on  (apfs)"] {
            assert_eq!(parse_mount_line(line), None, "{line:?}");
        }
        assert!(parse_mount_output("garbage\n\n").is_empty());
    }

    #[test]
    fn the_data_volume_stands_in_for_the_root_and_hidden_volumes_are_left_out() {
        assert_eq!(
            disks(MOUNT_OUTPUT),
            [
                ("/".to_string(), "/dev/disk3s5".to_string()),
                ("/Volumes/My Photos (2024)/".to_string(), "/dev/disk4s2".to_string()),
            ]
        );
    }

    #[test]
    fn without_a_data_volume_the_root_is_the_root() {
        let text = "/dev/disk1s1 on / (apfs, local, journaled)\ndevfs on /dev (devfs, local, nobrowse)\n";
        assert_eq!(disks(text), [("/".to_string(), "/dev/disk1s1".to_string())]);
    }

    #[test]
    fn the_folded_data_volume_keeps_its_other_options() {
        let folded = fold_data_volume(parse_mount_output(MOUNT_OUTPUT));
        let root: Vec<&MountEntry> = folded.iter().filter(|e| e.mount == "/").collect();
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].options, "local,journaled,protect,root data");
        // The mount points under the Data volume's own are not renamed.
        assert!(folded.iter().any(|e| e.mount == "/System/Volumes/Data/home/"));
    }

    #[test]
    fn only_disks_of_this_mac_count() {
        assert!(is_local_disk(&entry("/Volumes/P/", "/dev/disk4s1", "exfat", "local")));
        assert!(!is_local_disk(&entry("/dev/", "devfs", "devfs", "local,nobrowse")));
        assert!(!is_local_disk(&entry("/System/Volumes/VM/", "/dev/disk3s6", "apfs", "local,nobrowse")));
        assert!(!is_local_disk(&entry("/Volumes/photos/", "//user@nas/photos", "smbfs", "nodev")));
        assert!(!is_local_disk(&entry("/x/", "/dev/disk9", "autofs", "")));
    }

    #[test]
    fn data_volume_paths_are_written_the_way_the_user_sees_them() {
        assert_eq!(normalise("/System/Volumes/Data/Users/me/Pictures"), "/Users/me/Pictures");
        assert_eq!(normalise("/System/Volumes/Data"), "/");
        assert_eq!(normalise("/System/Volumes/DataExtra/x"), "/System/Volumes/DataExtra/x");
        assert_eq!(normalise("/Users/me"), "/Users/me");
        assert_eq!(normalise("/Volumes/Photos/x"), "/Volumes/Photos/x");
    }

    const EXFAT_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Bootable</key>
	<true/>
	<key>DeviceIdentifier</key>
	<string>disk4s2</string>
	<key>FilesystemName</key>
	<string>ExFAT</string>
	<key>FilesystemType</key>
	<string>exfat</string>
	<key>MountPoint</key>
	<string>/Volumes/Photos &amp; Video</string>
	<key>TotalSize</key>
	<integer>1000169537536</integer>
	<key>VolumeName</key>
	<string>Photos &amp; Video</string>
	<key>VolumeUUID</key>
	<string>9F2C1B7A-4D3E-4C55-8A6B-0123456789AB</string>
</dict>
</plist>
"#;

    #[test]
    fn diskutil_output_gives_uuid_name_and_size() {
        assert_eq!(
            parse_disk_info(EXFAT_PLIST),
            Some(DiskInfo {
                uuid: "9F2C1B7A-4D3E-4C55-8A6B-0123456789AB".to_string(),
                label: "Photos & Video".to_string(),
                size: 1_000_169_537_536,
            })
        );
    }

    #[test]
    fn a_volume_without_a_uuid_or_a_size_gives_nothing() {
        let no_uuid = EXFAT_PLIST.replace("VolumeUUID", "SomethingElse");
        assert_eq!(parse_disk_info(&no_uuid), None);
        let empty_uuid = EXFAT_PLIST.replace("<string>9F2C1B7A-4D3E-4C55-8A6B-0123456789AB</string>", "<string/>");
        assert_eq!(parse_disk_info(&empty_uuid), None);
        let no_size = EXFAT_PLIST.replace("TotalSize", "Other");
        assert_eq!(parse_disk_info(&no_size), None);
        let zero = EXFAT_PLIST.replace("1000169537536", "0");
        assert_eq!(parse_disk_info(&zero), None);
        assert_eq!(parse_disk_info(""), None);
        assert_eq!(parse_disk_info("not a plist"), None);
    }

    #[test]
    fn a_volume_without_a_name_has_an_empty_label() {
        let unnamed = EXFAT_PLIST
            .replace("<string>Photos &amp; Video</string>\n\t<key>VolumeUUID", "<string/>\n\t<key>VolumeUUID");
        assert_eq!(parse_disk_info(&unnamed).map(|d| d.label), Some(String::new()));
        let none = EXFAT_PLIST.replace("VolumeName", "Whatever");
        assert_eq!(parse_disk_info(&none).map(|d| d.label), Some(String::new()));
    }

    #[test]
    fn size_falls_back_to_the_size_key() {
        let plist = EXFAT_PLIST.replace("TotalSize", "Size");
        assert_eq!(parse_disk_info(&plist).map(|d| d.size), Some(1_000_169_537_536));
    }

    #[test]
    fn plist_values_of_every_simple_kind_are_read() {
        let plist = "<dict>\n<key>A</key><string>text</string><key>B</key>\n\t<integer> 42 </integer>\
                     <key>C</key><true/><key>D</key><false/><key>E</key><real>1.5</real>\
                     <key>F</key><dict><key>G</key><string>inner</string></dict><key>H</key><array></array>\
                     <key>I</key><string/><key>AB</key><string>other</string></dict>";
        assert_eq!(plist_value(plist, "A").as_deref(), Some("text"));
        assert_eq!(plist_value(plist, "B").as_deref(), Some(" 42 "));
        assert_eq!(plist_value(plist, "C").as_deref(), Some("true"));
        assert_eq!(plist_value(plist, "D").as_deref(), Some("false"));
        assert_eq!(plist_value(plist, "E").as_deref(), Some("1.5"));
        assert_eq!(plist_value(plist, "F"), None); // a dictionary
        assert_eq!(plist_value(plist, "H"), None); // an array
        assert_eq!(plist_value(plist, "I").as_deref(), Some(""));
        assert_eq!(plist_value(plist, "AB").as_deref(), Some("other")); // not mixed up with A
        assert_eq!(plist_value(plist, "Z"), None);
        assert_eq!(plist_value("<key>A</key>", "A"), None);
        assert_eq!(plist_value("<key>A</key><string>unterminated", "A"), None);
    }

    #[test]
    fn xml_entities_are_decoded() {
        assert_eq!(xml_unescape("a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&apos;"), "a & b <c> \"d\" 'e'");
        assert_eq!(xml_unescape("caf&#233; &#xE9; &#XE9;"), "caf\u{e9} \u{e9} \u{e9}");
        assert_eq!(xml_unescape("no entities"), "no entities");
        // What isn't a known entity stays as written.
        assert_eq!(xml_unescape("fish & chips; &unknown; &#zz; &"), "fish & chips; &unknown; &#zz; &");
        assert_eq!(xml_unescape("&amp;amp;"), "&amp;");
    }

    #[test]
    fn the_mask_follows_the_set_of_names_not_their_order() {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let two = mask_of_names(names(&["Macintosh HD", "Photos"]));
        assert_eq!(two, mask_of_names(names(&["Photos", "Macintosh HD"])));
        assert_ne!(two, mask_of_names(names(&["Macintosh HD"])));
        assert_ne!(two, mask_of_names(names(&["Macintosh HD", "Photos 1"])));
        assert_ne!(mask_of_names(names(&["ab"])), mask_of_names(names(&["a", "b"])));
    }

    #[test]
    fn options_are_matched_whole() {
        assert!(has_option("local,nobrowse,protect", "nobrowse"));
        assert!(!has_option("local,nobrowse2", "nobrowse"));
        assert!(!has_option("", "nobrowse"));
        assert_eq!(without_option("local,nobrowse,protect", "nobrowse"), "local,protect");
        assert_eq!(without_option("nobrowse", "nobrowse"), "");
    }
}
