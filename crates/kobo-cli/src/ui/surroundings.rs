//! What this computer can say about a reader beyond the files on it.
//!
//! Three things an owner runs into that no file on the card records: the
//! reader is plugged in but was never told to connect, the system will not
//! let this program read the card, and the card is full or mounted
//! read-only. Each used to surface as "no reader found" or as the first
//! write failing with whatever the system happened to say. Here each is
//! asked of the system directly, so the window can say which it is and what
//! clears it.
//!
//! None of this has a portable answer, and the standard library offers
//! none, so each question is put to the tool the system already ships for
//! it and the answer is read from its output. Every one of them is allowed
//! to come back as "cannot tell", which a front end treats as nothing to
//! say, never as a problem.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::setup;

/// How much a volume can still take, and whether it takes anything.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Room {
    /// Bytes free, when the system says.
    pub free: Option<u64>,
    /// Whether the volume is mounted for writing, when the system says.
    pub writable: Option<bool>,
}

/// Everything a command printed, or nothing if it could not be run.
fn said(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The room on the volume holding `path`.
///
/// Asked each time rather than remembered, because the two moments it
/// matters are straight after something was written and straight after a
/// card was swapped, and a remembered answer is wrong at both.
#[must_use]
pub fn room(path: &Path) -> Room {
    if cfg!(windows) {
        return Room::default();
    }
    let Some(listing) = said("df", &["-Pk", &path.to_string_lossy()]) else {
        return Room::default();
    };
    let Some((free, mounted_on)) = free_from_df(&listing) else {
        return Room::default();
    };
    let writable = if cfg!(target_os = "macos") {
        said("mount", &[]).and_then(|mounts| writable_from_mount(&mounts, &mounted_on))
    } else {
        fs::read_to_string("/proc/mounts")
            .ok()
            .and_then(|mounts| writable_from_proc_mounts(&mounts, &mounted_on))
    };
    Room {
        free: Some(free),
        writable,
    }
}

/// The free bytes and the mount point in `df -Pk` output for one path.
///
/// The portable format is one header line and one line per filesystem:
/// device, size, used, available, capacity, mount point, sizes in kibibytes.
/// A device name or a mount point can contain spaces, and a reader mounted
/// twice is called `KOBOeReader 1`, so the columns are found by the three
/// numbers and the percentage rather than by counting from either end.
fn free_from_df(listing: &str) -> Option<(u64, String)> {
    let line = listing.lines().nth(1)?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    let capacity = fields
        .iter()
        .position(|field| field.ends_with('%') && field.len() > 1)?;
    let available: u64 = fields.get(capacity.checked_sub(1)?)?.parse().ok()?;
    let mounted_on = fields.get(capacity + 1..)?.join(" ");
    (!mounted_on.is_empty()).then(|| (available.saturating_mul(1024), mounted_on))
}

/// Whether the volume at `mounted_on` is writable, from macOS `mount`.
///
/// Each line reads `device on /mount/point (flags, ...)`, and a volume
/// mounted for reading only carries the flag `read-only`.
fn writable_from_mount(mounts: &str, mounted_on: &str) -> Option<bool> {
    let wanted = format!(" on {mounted_on} (");
    let line = mounts.lines().find(|line| line.contains(&wanted))?;
    let flags = line.rsplit_once('(')?.1.trim_end_matches(')');
    Some(!flags.split(',').any(|flag| flag.trim() == "read-only"))
}

/// The same from Linux's `/proc/mounts`, whose lines read
/// `device mount-point type options ...` with a space in a mount point
/// written as `\040`.
fn writable_from_proc_mounts(mounts: &str, mounted_on: &str) -> Option<bool> {
    let wanted = mounted_on.replace(' ', "\\040");
    mounts.lines().find_map(|line| {
        let mut fields = line.split(' ');
        let (_device, point, _kind, options) = (
            fields.next()?,
            fields.next()?,
            fields.next()?,
            fields.next()?,
        );
        (point == wanted).then(|| !options.split(',').any(|option| option == "ro"))
    })
}

/// The USB vendor number every Kobo reader announces itself with.
const KOBO_VENDOR: u32 = 0x2237;

/// How many Kobos are on the USB bus, mounted or not, or `None` where this
/// system offers no way to ask.
///
/// A reader that is plugged in and showing its "Computer detected" question
/// is on the bus and has no drive. To everything else in this program that
/// is indistinguishable from no reader at all, and the owner is told to
/// plug in a reader that is already plugged in. Counting the bus is what
/// tells the two apart: more Kobos attached than mounted means one of them
/// is waiting for Connect to be tapped.
#[must_use]
pub fn kobos_attached() -> Option<usize> {
    if cfg!(target_os = "macos") {
        said("ioreg", &["-p", "IOUSB", "-l", "-w", "0"]).map(|tree| kobos_in_ioreg(&tree))
    } else if cfg!(target_os = "linux") {
        let devices = fs::read_dir("/sys/bus/usb/devices").ok()?;
        Some(
            devices
                .flatten()
                .filter(|device| {
                    fs::read_to_string(device.path().join("idVendor")).is_ok_and(|vendor| {
                        u32::from_str_radix(vendor.trim(), 16) == Ok(KOBO_VENDOR)
                    })
                })
                .count(),
        )
    } else {
        None
    }
}

/// The Kobos in the registry's USB plane, as `ioreg -p IOUSB -l` prints it:
/// one `"idVendor" = <decimal>` line for each device, behind the bars and
/// spaces the tree is drawn with. Only a line that is that property counts,
/// so the same words inside some other device's description do not.
fn kobos_in_ioreg(tree: &str) -> usize {
    tree.lines()
        .filter_map(|line| {
            line.trim_start_matches(['|', ' '])
                .strip_prefix("\"idVendor\" = ")
        })
        .filter(|vendor| vendor.trim().parse() == Ok(KOBO_VENDOR))
        .count()
}

/// Every mounted volume that could be a reader by its name.
///
/// A reader's card is labelled `KOBOeReader`, and a second one mounts as
/// `KOBOeReader 1`. Only those are looked at for [`refused`], because the
/// places removable volumes are mounted also hold network shares and backup
/// disks this account was never meant to read, and telling an owner that
/// the system is keeping Cobalt out of their backup disk would be both true
/// and beside the point.
#[must_use]
pub fn removable_volumes() -> Vec<PathBuf> {
    if cfg!(windows) {
        return Vec::new();
    }
    let mut volumes: Vec<PathBuf> = setup::mount_roots()
        .iter()
        .filter_map(|root| fs::read_dir(root).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
        .filter(|volume| {
            volume.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with("kobo")
            })
        })
        .collect();
    volumes.sort();
    volumes.dedup();
    volumes
}

/// The volumes among `volumes` that the system would not let this process
/// read.
///
/// On a Mac, a program has to be allowed to read removable volumes, once,
/// in a dialog that is easy to dismiss. A reader whose card is refused that
/// way is mounted and plugged in and invisible, and nothing but this says
/// why. The refusal is recognised by its kind, permission denied, on the
/// one file every reader has, so a volume that is merely empty or not a
/// reader is not mistaken for one that is being kept from us.
#[must_use]
pub fn refused(volumes: &[PathBuf]) -> Vec<PathBuf> {
    volumes
        .iter()
        .filter(|volume| {
            let version = volume.join(setup::SYSTEM_FOLDER).join("version");
            matches!(fs::read(&version), Err(error) if error.kind() == ErrorKind::PermissionDenied)
                || matches!(fs::read_dir(volume), Err(error) if error.kind() == ErrorKind::PermissionDenied)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_is_read_from_the_portable_listing_whatever_the_names_contain() {
        let plain = "Filesystem   1024-blocks  Used Available Capacity Mounted on\n\
                     /dev/disk5s1      429056 73296    355760    18% /Volumes/KOBOeReader\n";
        assert_eq!(
            free_from_df(plain),
            Some((355_760 * 1024, "/Volumes/KOBOeReader".to_owned()))
        );
        let spaced = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                      /dev/disk6s1 40112 2 40110 1% /Volumes/KOBOeReader 1\n";
        assert_eq!(
            free_from_df(spaced),
            Some((40_110 * 1024, "/Volumes/KOBOeReader 1".to_owned()))
        );
        let shared = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                      //me@nas.local/my share 100 60 40 60% /Volumes/my share\n";
        assert_eq!(
            free_from_df(shared),
            Some((40 * 1024, "/Volumes/my share".to_owned()))
        );
        let full = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                    /dev/sdb1 7654321 7654321 0 100% /media/me/KOBOeReader\n";
        assert_eq!(
            free_from_df(full),
            Some((0, "/media/me/KOBOeReader".to_owned()))
        );
        for unreadable in [
            "",
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n",
            "a\nb c d\n",
        ] {
            assert_eq!(free_from_df(unreadable), None, "{unreadable:?}");
        }
    }

    #[test]
    fn a_card_mounted_read_only_is_told_from_one_that_takes_writes_on_a_mac() {
        let mounts = "/dev/disk3s1s1 on / (apfs, sealed, local, read-only, journaled)\n\
                      /dev/disk5s1 on /Volumes/KOBOeReader (msdos, local, nodev, nosuid, noowners, noatime, fskit)\n\
                      /dev/disk6s1 on /Volumes/KOBOeReader 1 (msdos, local, nodev, nosuid, noowners, read-only)\n";
        assert_eq!(
            writable_from_mount(mounts, "/Volumes/KOBOeReader"),
            Some(true)
        );
        assert_eq!(
            writable_from_mount(mounts, "/Volumes/KOBOeReader 1"),
            Some(false)
        );
        assert_eq!(writable_from_mount(mounts, "/"), Some(false));
        // Not a prefix match: the second reader is not the first.
        assert_eq!(writable_from_mount(mounts, "/Volumes/KOBO"), None);
        assert_eq!(writable_from_mount(mounts, "/Volumes/Elsewhere"), None);
    }

    #[test]
    fn the_same_is_read_from_the_mount_table_on_linux() {
        let mounts = "/dev/sda2 / ext4 rw,relatime 0 0\n\
                      /dev/sdb1 /media/me/KOBOeReader vfat rw,nosuid,nodev,relatime,uid=1000,showexec,flush 0 0\n\
                      /dev/sdc1 /media/me/KOBOeReader\\0401 vfat ro,nosuid,nodev,relatime,errors=remount-ro 0 0\n";
        assert_eq!(
            writable_from_proc_mounts(mounts, "/media/me/KOBOeReader"),
            Some(true)
        );
        // `errors=remount-ro` is an option that mentions "ro"; it is not it.
        assert_eq!(
            writable_from_proc_mounts(mounts, "/media/me/KOBOeReader 1"),
            Some(false)
        );
        assert_eq!(writable_from_proc_mounts(mounts, "/media/me/Other"), None);
    }

    #[test]
    fn kobos_on_the_bus_are_counted_by_their_vendor_number_and_nothing_else_is() {
        let tree = "+-o Root  <class IORegistryEntry>\n\
                    +-o AppleT8132USBXHCI@00000000  <class AppleT8132USBXHCI>\n\
                    | +-o eReader-4.45.23697@00100000  <class IOUSBHostDevice>\n\
                    |     {\n\
                    |       \"idProduct\" = 16959\n\
                    |       \"idVendor\" = 8759\n\
                    |       \"USB Product Name\" = \"eReader-4.45.23697\"\n\
                    |     }\n\
                    | +-o Keyboard@00200000  <class IOUSBHostDevice>\n\
                    |     {\n\
                    |       \"idVendor\" = 1452\n\
                    |       \"kUSBVendorString\" = \"\\\"idVendor\\\" = 8759\"\n\
                    |     }\n";
        // One Kobo. The keyboard's description quotes the same property and
        // is not one.
        assert_eq!(kobos_in_ioreg(tree), 1);
        assert_eq!(kobos_in_ioreg(""), 0);
        assert_eq!(
            kobos_in_ioreg("\"idVendor\" = 8759\n  \"idVendor\" = 8759\n"),
            2
        );
        assert_eq!(kobos_in_ioreg("\"idVendor\" = 87590\n"), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_volume_the_system_will_not_let_us_read_is_told_from_one_that_is_merely_not_a_reader() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = std::env::temp_dir().join(format!("kobo-ui-refused-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (closed, open, empty) = (root.join("closed"), root.join("open"), root.join("empty"));
        for volume in [&closed, &open] {
            fs::create_dir_all(volume.join(".kobo")).expect("system folder");
            fs::write(volume.join(".kobo/version"), b"N365410043013,x,4.45\n").expect("version");
        }
        fs::create_dir_all(&empty).expect("empty volume");
        fs::set_permissions(closed.join(".kobo"), fs::Permissions::from_mode(0o000))
            .expect("close the folder");
        let volumes = [closed.clone(), open, empty, root.join("unplugged")];
        // An account that can read anything, as some build machines run
        // under, has nothing kept from it and nothing to test.
        if fs::read(closed.join(".kobo/version")).is_err() {
            assert_eq!(refused(&volumes), [closed.clone()]);
        }
        fs::set_permissions(closed.join(".kobo"), fs::Permissions::from_mode(0o755))
            .expect("open it again");
        assert!(refused(&volumes).is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
