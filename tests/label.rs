use gotek_core::{
    FormatOptions, GotekDisk, SlotFormatOptions, format_image, geometry::FloppyGeometry, inspect,
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
        &FormatOptions::with_slot_options(FloppyGeometry::Kb720, 2, slot).unwrap(),
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

#[test]
fn slot_handle_reuses_validated_layout_and_updates_all_label_copies() {
    let original = image();
    let capacity = original.len() as u64;
    let mut disk = GotekDisk::open(Cursor::new(original.clone()), capacity).unwrap();
    let mut first = disk.slot_mut(0).unwrap();
    assert_eq!(first.as_slot().unwrap().geometry(), FloppyGeometry::Kb720);
    first.set_label("my disk").unwrap();
    disk.slot_mut(1).unwrap().set_label("TEMP").unwrap();
    disk.slot_mut(1).unwrap().set_label("FINAL").unwrap();
    assert_eq!(disk.slot(1).unwrap().label().unwrap(), "FINAL");

    let mut expected = original;
    let slot = SLOT_STRIDE_BYTES as usize;
    expected[43..54].copy_from_slice(b"MY DISK    ");
    expected[7 * 512..7 * 512 + 11].copy_from_slice(b"MY DISK    ");
    expected[slot + 43..slot + 54].copy_from_slice(b"FINAL      ");
    expected[slot + 7 * 512..slot + 7 * 512 + 11].copy_from_slice(b"FINAL      ");
    expected[2 * slot + 43..2 * slot + 54].copy_from_slice(b"FINAL      ");
    let mut target = disk.into_inner();
    assert_eq!(target.get_ref(), &expected);
    let info = inspect(&mut target).unwrap();
    assert_eq!(info.slots[0].volume_label, "MY DISK");
    assert_eq!(info.slots[1].volume_label, "FINAL");
    assert!(info.warnings.is_empty());
}

#[test]
fn read_only_disk_opens_and_exposes_validated_slots() {
    let bytes = image();
    let capacity = bytes.len() as u64;
    // Cursor<&[u8]> cannot write, so this also checks the trait bounds.
    let mut disk = GotekDisk::open(Cursor::new(bytes.as_slice()), capacity).unwrap();
    let mut slot = disk.slot(1).unwrap();
    assert_eq!(slot.index(), 1);
    assert_eq!(slot.offset(), SLOT_STRIDE_BYTES);
    assert_eq!(slot.geometry(), FloppyGeometry::Kb720);
    assert_eq!(slot.bpb().root_entries, 112);
    assert_eq!(slot.label().unwrap(), "FLPPY1");
    assert!(disk.slot(2).is_err());
    assert_eq!(disk.into_inner().into_inner(), bytes.as_slice());
}

#[test]
fn slot_handle_rejects_invalid_input_without_writing() {
    let original = image();
    let capacity = original.len() as u64;
    let mut disk = GotekDisk::open(Cursor::new(original.clone()), capacity).unwrap();
    assert!(disk.slot_mut(2).is_err());
    assert!(disk.slot_mut(0).unwrap().set_label("BAD/NAME").is_err());
    assert_eq!(disk.into_inner().into_inner(), original);

    let mut missing_label = image();
    missing_label[7 * 512 + 11] = 0;
    let mut disk = GotekDisk::open(Cursor::new(missing_label.clone()), capacity).unwrap();
    assert!(disk.slot_mut(0).unwrap().set_label("NEW").is_err());
    assert_eq!(disk.into_inner().into_inner(), missing_label);
}
