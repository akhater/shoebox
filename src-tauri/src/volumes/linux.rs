//! Linux: the mounts come from `/proc/self/mountinfo`; each disk's UUID, label and size from `/dev/disk/by-uuid`,
//! `/dev/disk/by-label` and `/sys/class/block`.
//!
//! A disk is a local file system on a block device (`/dev/sdb1`, `/dev/nvme0n1p2`, `/dev/mapper/...`), which
//! includes the NTFS and exFAT disks the `fuseblk` driver serves. Such a file system is reported as `fuseblk`, not
//! as `ntfs` or `exfat`, because the mount table doesn't say which it is. Loop devices (snap packages), RAM disks
//! and the system's own file systems are not disks.
//!
//! A mount of only part of a file system (a btrfs subvolume, a bind mount of a folder) has a root other than `/`
//! in the mount table; it counts as a disk of its own (see `serial_for`).

use super::{DiskInfo, MountEntry, with_trailing_slash};
use std::path::Path;
use std::process::Command;

const MOUNT_TABLE: &str = "/proc/self/mountinfo";
const MOUNTS: &str = "/proc/self/mounts";

/// File systems that are never a disk to catalog photos on.
const NOT_DISKS: &[&str] = &[
    "tmpfs",
    "ramfs",
    "devtmpfs",
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "overlay",
    "squashfs",
    "aufs",
    "devpts",
    "securityfs",
    "debugfs",
    "tracefs",
    "configfs",
    "fusectl",
    "pstore",
    "bpf",
    "mqueue",
    "hugetlbfs",
    "autofs",
    "binfmt_misc",
    "efivarfs",
    "rpc_pipefs",
    "nsfs",
    "selinuxfs",
];

/// Why a disk can't be used when it has no file system UUID (a container, for one, has no `/dev/disk`).
pub(super) const NO_ID: &str = "no file system UUID is listed for it under /dev/disk/by-uuid";

/// Every mount the kernel lists.
pub(super) fn all_mounts() -> Vec<MountEntry> {
    std::fs::read_to_string(MOUNT_TABLE).map(|text| parse_mountinfo(&text)).unwrap_or_default()
}

/// A mount of a block device that holds files, as opposed to the system's own file systems, snap images and the like.
pub(super) fn is_local_disk(entry: &MountEntry) -> bool {
    entry.source.starts_with("/dev/")
        && !entry.source.starts_with("/dev/loop")
        && !entry.source.starts_with("/dev/ram")
        && !NOT_DISKS.contains(&entry.fstype.as_str())
}

/// UUID, label and size of the disk behind a mount. `None` if the mount point isn't a folder (a bind-mounted
/// file), or nothing tells this disk from others.
///
/// The block device is found by the number the mount table gives it (`/sys/dev/block/8:1` leads to `sda1`): a root
/// file system is often listed as `/dev/root`, which is no file at all. The UUID and label are the links to that
/// device under `/dev/disk`; where there are none (no udev, a container) the kernel's own id of the file system
/// stands in for the UUID, and the size comes from `df`.
pub(super) fn disk_info(entry: &MountEntry) -> Option<DiskInfo> {
    if !Path::new(&entry.mount).is_dir() {
        return None;
    }
    let block = block_name(&entry.device)
        .or_else(|| std::fs::canonicalize(&entry.source).ok()?.file_name().map(|n| n.to_string_lossy().into_owned()));
    let by_link = |dir: &str| block.as_deref().and_then(|b| link_name_for(Path::new(dir), b));
    let uuid = by_link("/dev/disk/by-uuid").or_else(|| file_system_id(&entry.mount))?;
    let label = by_link("/dev/disk/by-label").map(|name| decode_hex_escapes(&name));
    let size = block.as_deref().and_then(size_of_block).or_else(|| total_bytes(&entry.mount))?;
    Some(DiskInfo { uuid, label: label.unwrap_or_default(), size })
}

/// The name of the block device with this `major:minor` number (`sda1`, `dm-0`, `nvme0n1p2`), from the link
/// `/sys/dev/block/<major:minor>`.
fn block_name(device: &str) -> Option<String> {
    let target = std::fs::read_link(Path::new("/sys/dev/block").join(device)).ok()?;
    name_of_link_target(&target)
}

/// The last part of where a link leads: `sda1` for `../../devices/pci0000:00/ata1/block/sda/sda1`.
fn name_of_link_target(target: &Path) -> Option<String> {
    target.file_name().map(|name| name.to_string_lossy().into_owned())
}

/// The bytes of the block device `block`, from `/sys/class/block`.
fn size_of_block(block: &str) -> Option<u64> {
    bytes_from_sectors(&std::fs::read_to_string(Path::new("/sys/class/block").join(block).join("size")).ok()?)
}

/// The kernel's id of the file system at `mount` (`stat -f`), derived from its UUID or volume serial, so it is the
/// same every time the disk is mounted. Written `fsid-<hex>` so it can't be taken for a UUID.
fn file_system_id(mount: &str) -> Option<String> {
    parse_file_system_id(&super::output_of(Command::new("stat").args(["-f", "-c", "%i", "--"]).arg(mount))?)
}

fn parse_file_system_id(text: &str) -> Option<String> {
    let id = u64::from_str_radix(text.trim(), 16).ok().filter(|&id| id != 0)?;
    Some(format!("fsid-{id:x}"))
}

/// The size of the file system at `mount`, from `df`.
fn total_bytes(mount: &str) -> Option<u64> {
    super::parse_df_total(&super::output_of(Command::new("df").args(["-Pk", "--"]).arg(mount))?)
}

/// Paths need no change here (macOS has to undo a firmlink).
pub(super) fn normalise(path: &str) -> String {
    path.to_string()
}

/// A number standing for the list of mounts, which changes when a disk is plugged in or out; 0 if it can't be read.
pub(super) fn drive_mask() -> u32 {
    std::fs::read(MOUNTS).map_or(0, |bytes| super::hash_u32(&bytes))
}

/// The name of the link in `dir` (`/dev/disk/by-uuid`, ...) that leads to the block device `block`, if there is one.
fn link_name_for(dir: &Path, block: &str) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|link| {
            std::fs::canonicalize(link.path()).is_ok_and(|target| target.file_name().is_some_and(|n| n == block))
        })
        .map(|link| link.file_name().to_string_lossy().into_owned())
}

/// The bytes of a device whose `size` file holds `text`: a number of 512-byte sectors, whatever the device's own
/// block size.
fn bytes_from_sectors(text: &str) -> Option<u64> {
    text.trim().parse::<u64>().ok()?.checked_mul(512).filter(|&bytes| bytes > 0)
}

/// The mounts in `/proc/self/mountinfo`: one per line,
/// `id parent major:minor root mount-point options [optional fields] - type source super-options`.
/// Spaces, tabs, newlines and backslashes in a path are written as octal escapes (`\040`).
fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    text.lines().filter_map(parse_mountinfo_line).collect()
}

fn parse_mountinfo_line(line: &str) -> Option<MountEntry> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    // The optional fields (`shared:1`, `master:2`, ...) end at a lone dash.
    let dash = 6 + fields.iter().skip(6).position(|&f| f == "-")?;
    Some(MountEntry {
        mount: with_trailing_slash(&decode_octal_escapes(fields.get(4)?)),
        source: decode_octal_escapes(fields.get(dash + 2)?),
        device: fields.get(2)?.to_string(),
        fstype: fields.get(dash + 1)?.to_lowercase(),
        root: decode_octal_escapes(fields.get(3)?),
        options: fields.get(5)?.to_string(),
    })
}

/// `text` with every `\NNN` (three octal digits) replaced by the byte it stands for.
fn decode_octal_escapes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && let Some(byte) = bytes.get(i + 1..i + 4).and_then(octal_byte)
        {
            out.push(byte);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn octal_byte(digits: &[u8]) -> Option<u8> {
    if digits.len() != 3 || !digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
        return None;
    }
    let value = digits.iter().fold(0u32, |n, d| n * 8 + u32::from(d - b'0'));
    u8::try_from(value).ok()
}

/// `text` with every `\xHH` (two hex digits) replaced by the byte it stands for, as udev writes the names in
/// `/dev/disk/by-label` (`My\x20Photos`).
fn decode_hex_escapes(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes.get(i + 1) == Some(&b'x')
            && let Some(byte) = bytes.get(i + 2..i + 4).and_then(hex_byte)
        {
            out.push(byte);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_byte(digits: &[u8]) -> Option<u8> {
    if digits.len() != 2 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u8::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::volumes::{serial_for, volume_from};

    /// `/proc/self/mountinfo` of a PC with two internal disks, a btrfs `@home` subvolume, an exFAT stick named
    /// `My Photos`, an NTFS disk, a snap package, and the system's own file systems.
    const MOUNTINFO: &str = "\
24 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw,errors=remount-ro
25 24 0:22 / /sys rw,nosuid,nodev,noexec,relatime shared:7 - sysfs sysfs rw
26 24 0:6 / /dev rw,nosuid shared:2 - devtmpfs udev rw,size=8000000k,nr_inodes=2000000,mode=755
27 24 0:24 / /run rw,nosuid,nodev,noexec,relatime shared:5 - tmpfs tmpfs rw,size=1600000k,mode=755
28 24 259:1 / /boot/efi rw,relatime shared:14 - vfat /dev/nvme0n1p1 rw,fmask=0077,dmask=0077
29 24 8:34 /@home /home rw,relatime shared:2 - btrfs /dev/sdc2 rw,ssd,space_cache=v2,subvolid=256,subvol=/@home
30 24 8:17 / /media/me/My\\040Photos rw,nosuid,nodev,relatime shared:5 - exfat /dev/sdb1 rw,iocharset=utf8
31 24 8:49 / /mnt/ntfs\\134old rw,nosuid,nodev,relatime - fuseblk /dev/sdd1 rw,user_id=0,group_id=0,allow_other
32 24 7:0 / /snap/core/17200 ro,nodev,relatime shared:9 - squashfs /dev/loop0 ro,errors=continue
33 24 8:49 / /home/me/Backup rw,relatime shared:6 - ext4 /dev/sdb2 rw
";

    fn entry(mount: &str, source: &str, fstype: &str, root: &str) -> MountEntry {
        MountEntry {
            mount: mount.to_string(),
            source: source.to_string(),
            device: String::new(),
            fstype: fstype.to_string(),
            root: root.to_string(),
            options: String::new(),
        }
    }

    fn parsed(mount: &str) -> MountEntry {
        parse_mountinfo(MOUNTINFO).into_iter().find(|e| e.mount == mount).unwrap_or_else(|| panic!("no {mount}"))
    }

    #[test]
    fn a_mount_keeps_the_number_of_its_device() {
        let line = "36 1 8:1 / / rw,relatime shared:1 - ext4 /dev/root rw,errors=remount-ro";
        let mount = parse_mountinfo_line(line).unwrap();
        assert_eq!((mount.device.as_str(), mount.source.as_str()), ("8:1", "/dev/root"));
    }

    #[test]
    fn a_block_device_is_named_by_the_end_of_its_sys_link() {
        let target = Path::new("../../devices/pci0000:00/0000:00:1f.2/ata1/host0/target0:0:0/0:0:0:0/block/sda/sda1");
        assert_eq!(name_of_link_target(target).as_deref(), Some("sda1"));
        assert_eq!(name_of_link_target(Path::new("../../devices/virtual/block/dm-0")).as_deref(), Some("dm-0"));
        assert_eq!(name_of_link_target(Path::new("")), None);
    }

    #[test]
    fn the_kernels_file_system_id_is_read_as_hex() {
        assert_eq!(parse_file_system_id("a1b2c3d4e5f60718\n").as_deref(), Some("fsid-a1b2c3d4e5f60718"));
        assert_eq!(parse_file_system_id("0\n"), None);
        assert_eq!(parse_file_system_id("stat: cannot read file system information\n"), None);
    }

    #[test]
    fn every_line_of_the_mount_table_is_read() {
        assert_eq!(parse_mountinfo(MOUNTINFO).len(), 10);
    }

    #[test]
    fn a_mount_line_is_cut_into_its_parts() {
        let mut root = entry("/", "/dev/nvme0n1p2", "ext4", "/");
        root.options = "rw,relatime".to_string();
        root.device = "259:2".to_string();
        assert_eq!(parsed("/"), root);
        let boot = parsed("/boot/efi/");
        assert_eq!((boot.source.as_str(), boot.fstype.as_str(), boot.root.as_str()), ("/dev/nvme0n1p1", "vfat", "/"));
    }

    #[test]
    fn a_mount_without_optional_fields_is_read() {
        let e = parse_mountinfo_line("25 24 0:22 / /x rw - tmpfs tmpfs rw").unwrap();
        assert_eq!(
            (e.mount.as_str(), e.source.as_str(), e.fstype.as_str(), e.options.as_str()),
            ("/x/", "tmpfs", "tmpfs", "rw")
        );
        let e = parse_mountinfo_line(
            "25 24 0:22 / /y rw,nosuid shared:1 master:2 propagate_from:3 unbindable - ext4 /dev/sdz1 rw",
        )
        .unwrap();
        assert_eq!((e.mount.as_str(), e.source.as_str(), e.fstype.as_str()), ("/y/", "/dev/sdz1", "ext4"));
    }

    #[test]
    fn escapes_in_paths_are_decoded() {
        assert_eq!(parsed("/media/me/My Photos/").source, "/dev/sdb1");
        assert_eq!(parsed("/mnt/ntfs\\old/").source, "/dev/sdd1");
    }

    #[test]
    fn a_subvolume_keeps_its_root() {
        assert_eq!(parsed("/home/").root, "/@home");
        assert_eq!(parsed("/").root, "/");
    }

    #[test]
    fn lines_that_are_not_mounts_are_skipped() {
        for line in ["", "garbage", "24 1 259:2 / / rw,relatime shared:1 ext4 /dev/sda1 rw", "24 1 259:2 / / rw - ext4"]
        {
            assert_eq!(parse_mountinfo_line(line), None, "{line:?}");
        }
    }

    #[test]
    fn only_block_devices_that_hold_files_are_disks() {
        let local: Vec<String> =
            parse_mountinfo(MOUNTINFO).into_iter().filter(is_local_disk).map(|e| e.mount).collect();
        assert_eq!(local, ["/", "/boot/efi/", "/home/", "/media/me/My Photos/", "/mnt/ntfs\\old/", "/home/me/Backup/"]);
        // The NTFS and exFAT drivers that run in user space are disks too; images and RAM disks are not.
        assert!(is_local_disk(&entry("/mnt/n/", "/dev/sdd1", "fuseblk", "/")));
        assert!(is_local_disk(&entry("/mnt/c/", "/dev/mapper/luks-1234", "ext4", "/")));
        assert!(!is_local_disk(&entry("/mnt/l/", "/dev/loop3", "ext4", "/")));
        assert!(!is_local_disk(&entry("/mnt/r/", "/dev/ram0", "ext2", "/")));
        assert!(!is_local_disk(&entry("/mnt/t/", "/dev/sda9", "tmpfs", "/")));
        assert!(!is_local_disk(&entry("/mnt/s/", "//nas/share", "cifs", "/")));
    }

    #[test]
    fn octal_escapes_stand_for_bytes() {
        assert_eq!(decode_octal_escapes("My\\040Photos"), "My Photos");
        assert_eq!(decode_octal_escapes("a\\011b\\012c"), "a\tb\nc");
        assert_eq!(decode_octal_escapes("back\\134slash"), "back\\slash");
        assert_eq!(decode_octal_escapes("\\303\\251t\\303\\251"), "\u{e9}t\u{e9}");
        assert_eq!(decode_octal_escapes("plain"), "plain");
    }

    #[test]
    fn what_is_not_an_octal_escape_stays_as_written() {
        for text in ["a\\b", "a\\", "a\\04", "a\\048", "a\\0x1", "trailing\\"] {
            assert_eq!(decode_octal_escapes(text), text, "{text:?}");
        }
        // An escaped backslash is one byte, and what follows it is not decoded twice.
        assert_eq!(decode_octal_escapes("\\134040"), "\\040");
    }

    #[test]
    fn hex_escapes_stand_for_bytes() {
        assert_eq!(decode_hex_escapes("My\\x20Photos"), "My Photos");
        assert_eq!(decode_hex_escapes("Caf\\xc3\\xa9"), "Caf\u{e9}");
        assert_eq!(decode_hex_escapes("plain"), "plain");
        for text in ["a\\x2", "a\\xzz", "a\\x+1", "a\\y20", "a\\", "a\\x"] {
            assert_eq!(decode_hex_escapes(text), text, "{text:?}");
        }
    }

    #[test]
    fn a_device_size_is_counted_in_512_byte_sectors() {
        assert_eq!(bytes_from_sectors("1953525168\n"), Some(1_000_204_886_016));
        assert_eq!(bytes_from_sectors("2048"), Some(1_048_576));
        assert_eq!(bytes_from_sectors("0\n"), None);
        assert_eq!(bytes_from_sectors(""), None);
        assert_eq!(bytes_from_sectors("many"), None);
        assert_eq!(bytes_from_sectors("18446744073709551615"), None);
    }

    #[test]
    fn disks_are_told_apart_by_uuid_and_the_part_of_the_file_system_mounted() {
        let uuid = "6b1f0a52-3c1e-4a58-9d3c-5f6a7e8b9c0d";
        let info = |label: &str| DiskInfo { uuid: uuid.to_string(), label: label.to_string(), size: 1_000_000_000 };
        let whole = volume_from(&entry("/mnt/a/", "/dev/sdc2", "btrfs", "/"), info(""));
        let bound = volume_from(&entry("/mnt/b/", "/dev/sdc2", "btrfs", "/"), info(""));
        let home = volume_from(&entry("/home/", "/dev/sdc2", "btrfs", "/@home"), info(""));
        // The same disk mounted in two places is one disk; a subvolume of it is another.
        assert_eq!(whole.serial, bound.serial);
        assert_ne!(whole.serial, home.serial);
        assert_eq!(home.serial, serial_for(uuid, "/@home"));
        assert_eq!(home.fs, "btrfs");
    }

    #[test]
    fn a_fat_disk_keeps_the_serial_windows_shows() {
        let boot = volume_from(
            &entry("/boot/efi/", "/dev/nvme0n1p1", "vfat", "/"),
            DiskInfo { uuid: "ABCD-1234".to_string(), label: "EFI".to_string(), size: 536_870_912 },
        );
        assert_eq!(boot.serial, 0xABCD_1234);
    }
}
