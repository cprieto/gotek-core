use gotek_core::{
    FormatOptions, SlotFormatOptions, format_image, geometry::FloppyGeometry, inspect,
    layout::SLOT_STRIDE_BYTES, metadata::UpdateTime, set_slot_label, set_slot_label_on,
};
use std::{fs, io::Cursor, path::PathBuf};

fn image() -> Vec<u8> {
    let time = UpdateTime {
        year: 2026,
        month: 11,
        day: 4,
        hour: 12,
        minute: 0,
        second: 0,
    };
    let slot = SlotFormatOptions {
        volume_serial: 42,
        update_time: time,
        root_label_time: time,
    };
    let mut output = Cursor::new(Vec::new());
    format_image(
        &mut output,
        &FormatOptions::new(FloppyGeometry::Kb720, 2, slot).unwrap(),
    )
    .unwrap();
    output.into_inner()
}

fn temporary_image() -> (PathBuf, PathBuf) {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "gotek-label-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let path = directory.join("image.img");
    fs::write(&path, image()).unwrap();
    (directory, path)
}

#[test]
fn changes_only_label_bytes_and_keeps_boundary_copy_in_sync() {
    let (directory, path) = temporary_image();
    let mut expected = fs::read(&path).unwrap();
    set_slot_label(&path, 0, "data disk").unwrap();
    expected[43..54].copy_from_slice(b"DATA DISK  ");
    // 720K root directory begins after the boot sector and two 3-sector FATs.
    expected[7 * 512..7 * 512 + 11].copy_from_slice(b"DATA DISK  ");
    assert_eq!(fs::read(&path).unwrap(), expected);

    set_slot_label(&path, 1, "BACKUP").unwrap();
    let slot = SLOT_STRIDE_BYTES as usize;
    expected[slot + 43..slot + 54].copy_from_slice(b"BACKUP     ");
    expected[slot + 7 * 512..slot + 7 * 512 + 11].copy_from_slice(b"BACKUP     ");
    expected[2 * slot + 43..2 * slot + 54].copy_from_slice(b"BACKUP     ");
    assert_eq!(fs::read(&path).unwrap(), expected);
    let inspected = inspect(&mut fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(inspected.slots[0].volume_label, "DATA DISK");
    assert_eq!(inspected.slots[1].volume_label, "BACKUP");
    assert!(inspected.warnings.is_empty());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rejects_bad_inputs_without_writing() {
    let (directory, path) = temporary_image();
    let original = fs::read(&path).unwrap();
    for label in ["", "TOO LONG NAME", "BAD/NAME", "BADé", " SPACE"] {
        assert!(set_slot_label(&path, 0, label).is_err());
    }
    assert!(set_slot_label(&path, 2, "VALID").is_err());
    assert_eq!(fs::read(&path).unwrap(), original);

    let mut no_root_label = original.clone();
    no_root_label[7 * 512 + 11] = 0;
    fs::write(&path, &no_root_label).unwrap();
    assert!(set_slot_label(&path, 0, "VALID").is_err());
    assert_eq!(fs::read(&path).unwrap(), no_root_label);
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn rejects_symlink_image() {
    let (directory, path) = temporary_image();
    let alias = directory.join("alias.img");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(set_slot_label(&alias, 0, "VALID").is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn descriptor_updates_only_selected_slot_and_final_boundary() {
    let original = image();
    let capacity = original.len() as u64;
    let mut device = Cursor::new(original.clone());
    device.set_position(1234);
    set_slot_label_on(&mut device, capacity, 1, "DEVICE").unwrap();
    let mut expected = original;
    let slot = SLOT_STRIDE_BYTES as usize;
    expected[slot + 43..slot + 54].copy_from_slice(b"DEVICE     ");
    expected[slot + 7 * 512..slot + 7 * 512 + 11].copy_from_slice(b"DEVICE     ");
    expected[2 * slot + 43..2 * slot + 54].copy_from_slice(b"DEVICE     ");
    assert_eq!(device.get_ref(), &expected);
    assert_eq!(
        inspect(&mut device).unwrap().slots[1].volume_label,
        "DEVICE"
    );
}

#[test]
fn descriptor_rejects_insufficient_capacity_and_invalid_slot_without_writing() {
    let original = image();
    let capacity = original.len() as u64;
    let mut device = Cursor::new(original.clone());
    for invalid_capacity in [0, SLOT_STRIDE_BYTES, capacity - 1] {
        assert!(set_slot_label_on(&mut device, invalid_capacity, 1, "DEVICE").is_err());
        assert_eq!(device.get_ref(), &original);
    }
    assert!(set_slot_label_on(&mut device, capacity, 2, "DEVICE").is_err());
    assert_eq!(device.get_ref(), &original);
}
