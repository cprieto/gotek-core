//! Linux-only safety checks for CLI formatting of whole USB block devices.
use gotek_core::{FormatOptions, format_on};
use std::{
    collections::HashSet,
    error::Error,
    fs::{self, Metadata, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    rdev: u64,
}

impl From<&Metadata> for Identity {
    fn from(value: &Metadata) -> Self {
        Self {
            dev: value.dev(),
            ino: value.ino(),
            rdev: value.rdev(),
        }
    }
}

#[derive(Clone, Copy)]
struct Safety {
    whole: bool,
    usb: bool,
    mounted: bool,
    held: bool,
}

#[derive(Clone, Copy)]
struct DeviceChecks {
    force: bool,
    safety: Safety,
    identity_matches: bool,
    capacity: u64,
    required: u64,
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason)
}

fn validate(
    force: bool,
    safety: Safety,
    identity_matches: bool,
    capacity: u64,
    required: u64,
) -> io::Result<()> {
    validate_candidate(force, safety)?;
    if !identity_matches {
        return Err(invalid("device identity changed before formatting"));
    }
    if capacity < required {
        return Err(invalid(
            "USB device capacity is insufficient for the selected format",
        ));
    }
    Ok(())
}

fn validate_candidate(force: bool, safety: Safety) -> io::Result<()> {
    if !force {
        return Err(invalid("formatting a physical device requires --force"));
    }
    if !safety.whole {
        return Err(invalid(
            "destination is a partition; select the whole USB device",
        ));
    }
    if !safety.usb {
        return Err(invalid("destination is not verified as a USB block device"));
    }
    if safety.mounted {
        return Err(invalid("device or a child partition is mounted"));
    }
    if safety.held {
        return Err(invalid("device or a child partition has active holders"));
    }
    Ok(())
}

fn dev_number(rdev: u64) -> String {
    let major = ((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfff);
    let minor = (rdev & 0xff) | ((rdev >> 12) & !0xff);
    format!("{major}:{minor}")
}

fn sysfs_dir(number: &str) -> io::Result<PathBuf> {
    let path = fs::canonicalize(format!("/sys/dev/block/{number}"))?;
    if fs::read_to_string(path.join("dev"))?.trim() != number {
        return Err(invalid(
            "sysfs device identity does not match the device node",
        ));
    }
    Ok(path)
}

fn is_usb_device(sysfs: &Path) -> io::Result<bool> {
    let usb = fs::canonicalize("/sys/bus/usb")?;
    for ancestor in sysfs.ancestors() {
        match fs::canonicalize(ancestor.join("subsystem")) {
            Ok(subsystem) if subsystem == usb => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

fn members_and_holders(sysfs: &Path) -> io::Result<(HashSet<String>, bool)> {
    let mut members = HashSet::new();
    members.insert(fs::read_to_string(sysfs.join("dev"))?.trim().to_owned());
    let mut held = fs::read_dir(sysfs.join("holders"))?
        .next()
        .transpose()?
        .is_some();
    for entry in fs::read_dir(sysfs)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        if path.join("partition").try_exists()? {
            members.insert(fs::read_to_string(path.join("dev"))?.trim().to_owned());
            held |= fs::read_dir(path.join("holders"))?
                .next()
                .transpose()?
                .is_some();
        }
    }
    Ok((members, held))
}

fn mounted_members(members: &HashSet<String>) -> io::Result<bool> {
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    has_mounted_member(&mountinfo, members)
}

fn has_mounted_member(mountinfo: &str, members: &HashSet<String>) -> io::Result<bool> {
    for line in mountinfo.lines() {
        let number = line
            .split_whitespace()
            .nth(2)
            .ok_or_else(|| invalid("malformed mountinfo"))?;
        if members.contains(number) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn inspect_safety(sysfs: &Path) -> io::Result<Safety> {
    let whole = !sysfs.join("partition").try_exists()?;
    let usb = is_usb_device(sysfs)?;
    let (members, held) = members_and_holders(sysfs)?;
    let mounted = mounted_members(&members)?;
    Ok(Safety {
        whole,
        usb,
        mounted,
        held,
    })
}

fn ioctl_u64(file: &fs::File, request: libc::c_ulong) -> io::Result<u64> {
    let mut bytes = 0_u64;
    // SAFETY: these Linux block ioctls write one u64 to a valid pointer. The
    // opened descriptor remains alive throughout the call.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request, &mut bytes) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(bytes)
}

fn block_capacity(file: &fs::File) -> io::Result<u64> {
    // Linux BLKGETSIZE64 is _IOR(0x12, 114, size_t), returning bytes in u64.
    const BLKGETSIZE64: libc::c_ulong = 0x8000_1272 | ((size_of::<usize>() as libc::c_ulong) << 16);
    ioctl_u64(file, BLKGETSIZE64)
}

fn disk_sequence(file: &fs::File) -> io::Result<u64> {
    // Linux BLKGETDISKSEQ is _IOR(0x12, 128, u64).
    const BLKGETDISKSEQ: libc::c_ulong = 0x8008_1280;
    ioctl_u64(file, BLKGETDISKSEQ)
}

fn sysfs_sequence(sysfs: &Path) -> io::Result<u64> {
    fs::read_to_string(sysfs.join("diskseq"))?
        .trim()
        .parse()
        .map_err(|_| invalid("invalid sysfs disk sequence number"))
}

fn open_exclusive(path: &Path) -> io::Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_EXCL | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot open {} exclusively: {error}", path.display()),
            )
        })
}

pub(super) fn format_device(
    path: &Path,
    initial: &Metadata,
    options: &FormatOptions,
    force: bool,
) -> Result<u64, Box<dyn Error>> {
    let required = options.image_size_bytes()?;
    if !force {
        return Err(invalid("formatting a physical device requires --force").into());
    }
    if !initial.file_type().is_block_device() {
        return Err(invalid("destination is not a block device").into());
    }
    let initial_identity = Identity::from(initial);
    let number = dev_number(initial_identity.rdev);
    let sysfs = sysfs_dir(&number)?;
    let initial_sequence = sysfs_sequence(&sysfs)?;
    let preflight = inspect_safety(&sysfs)?;
    validate_candidate(force, preflight)?;

    let mut device = open_exclusive(path)?;
    let opened = device.metadata()?;
    let identity_matches = opened.file_type().is_block_device()
        && Identity::from(&opened) == initial_identity
        && sysfs_dir(&dev_number(opened.rdev()))? == sysfs
        && disk_sequence(&device)? == initial_sequence
        && sysfs_sequence(&sysfs)? == initial_sequence;
    if !identity_matches {
        return Err(invalid("device identity changed before formatting").into());
    }
    let capacity = block_capacity(&device)?;
    let safety = inspect_safety(&sysfs)?;
    let checks = DeviceChecks {
        force,
        safety,
        identity_matches,
        capacity,
        required,
    };
    format_checked(&mut device, path, options, checks)?;
    device.sync_all()?;
    Ok(capacity)
}

fn format_checked<W: io::Write + io::Seek>(
    target: &mut W,
    path: &Path,
    options: &FormatOptions,
    checks: DeviceChecks,
) -> Result<(), Box<dyn Error>> {
    validate(
        checks.force,
        checks.safety,
        checks.identity_matches,
        checks.capacity,
        checks.required,
    )?;
    eprintln!(
        "WARNING: destroying data on USB device {} ({} bytes); {} floppy slots, {} each.",
        path.display(),
        checks.capacity,
        options.slots.len(),
        options.geometry
    );
    format_on(target, checks.capacity, options)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gotek_core::{SlotFormatOptions, geometry::FloppyGeometry, metadata::UpdateTime};
    use std::io::Cursor;

    fn safe() -> Safety {
        Safety {
            whole: true,
            usb: true,
            mounted: false,
            held: false,
        }
    }

    #[test]
    fn refuses_unsafe_devices_before_formatting() {
        assert!(validate(false, safe(), true, 200, 100).is_err());
        assert!(
            validate(
                true,
                Safety {
                    whole: false,
                    ..safe()
                },
                true,
                200,
                100
            )
            .is_err()
        );
        assert!(
            validate(
                true,
                Safety {
                    usb: false,
                    ..safe()
                },
                true,
                200,
                100
            )
            .is_err()
        );
        assert!(
            validate(
                true,
                Safety {
                    mounted: true,
                    ..safe()
                },
                true,
                200,
                100
            )
            .is_err()
        );
        assert!(
            validate(
                true,
                Safety {
                    held: true,
                    ..safe()
                },
                true,
                200,
                100
            )
            .is_err()
        );
        assert!(validate(true, safe(), false, 200, 100).is_err());
        assert!(validate(true, safe(), true, 99, 100).is_err());
        assert!(validate(true, safe(), true, 100, 100).is_ok());
    }

    #[test]
    fn mounted_child_partition_is_detected_by_device_number() {
        let members = HashSet::from(["8:16".to_owned(), "8:17".to_owned()]);
        let mountinfo = "1 0 8:17 / /media/usb rw - vfat /dev/sdb1 rw\n";
        assert!(has_mounted_member(mountinfo, &members).unwrap());
        assert!(!has_mounted_member("1 0 8:32 / / rw - ext4 /dev/sdc rw\n", &members).unwrap());
    }

    #[test]
    fn sysfs_scan_skips_regular_files_and_finds_partitions() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let folder = std::env::temp_dir().join(format!(
            "gotek-sysfs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).unwrap();
        fs::create_dir(folder.join("holders")).unwrap();
        fs::write(folder.join("dev"), b"8:48\n").unwrap();
        fs::write(folder.join("size"), b"12345\n").unwrap();
        let partition = folder.join("sdd1");
        fs::create_dir(&partition).unwrap();
        fs::create_dir(partition.join("holders")).unwrap();
        fs::write(partition.join("partition"), b"1\n").unwrap();
        fs::write(partition.join("dev"), b"8:49\n").unwrap();

        let (members, held) = members_and_holders(&folder).unwrap();
        assert_eq!(members, HashSet::from(["8:48".into(), "8:49".into()]));
        assert!(!held);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn failed_safety_checks_never_write() {
        let time = UpdateTime {
            year: 2026,
            month: 11,
            day: 1,
            hour: 12,
            minute: 0,
            second: 0,
        };
        let options = FormatOptions::with_slot_options(
            FloppyGeometry::Kb720,
            1,
            SlotFormatOptions {
                volume_serial: 1,
                update_time: time,
                root_label_time: time,
            },
        )
        .unwrap();
        let required = options.image_size_bytes().unwrap();
        let before = vec![0x5a; required as usize];
        for (force, safety, identity, capacity) in [
            (false, safe(), true, required),
            (
                true,
                Safety {
                    whole: false,
                    ..safe()
                },
                true,
                required,
            ),
            (
                true,
                Safety {
                    mounted: true,
                    ..safe()
                },
                true,
                required,
            ),
            (true, safe(), false, required),
            (true, safe(), true, required - 1),
        ] {
            let mut target = Cursor::new(before.clone());
            assert!(
                format_checked(
                    &mut target,
                    Path::new("/dev/mock"),
                    &options,
                    DeviceChecks {
                        force,
                        safety,
                        identity_matches: identity,
                        capacity,
                        required
                    }
                )
                .is_err()
            );
            assert_eq!(target.into_inner(), before);
        }
    }

    #[test]
    fn exclusive_open_refuses_symlinks_and_never_creates_missing_paths() {
        use std::{
            os::unix::fs::symlink,
            sync::atomic::{AtomicU64, Ordering},
        };
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let folder = std::env::temp_dir().join(format!(
            "gotek-open-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).unwrap();
        let target = folder.join("target");
        let link = folder.join("link");
        fs::write(&target, b"keep").unwrap();
        symlink(&target, &link).unwrap();
        assert!(open_exclusive(&link).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        let missing = folder.join("missing");
        assert!(open_exclusive(&missing).is_err());
        assert!(!missing.exists());
        fs::remove_dir_all(folder).unwrap();
    }
}
