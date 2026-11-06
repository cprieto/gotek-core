use gotek_core::{
    FormatOptions, GotekDisk, SlotFormatOptions, format_image,
    geometry::FloppyGeometry,
    layout::{METADATA_OFFSET_BYTES, SLOT_STRIDE_BYTES},
    metadata::UpdateTime,
};
use std::io::{Cursor, Write};

fn time() -> UpdateTime {
    UpdateTime {
        year: 2026,
        month: 11,
        day: 4,
        hour: 12,
        minute: 0,
        second: 0,
    }
}

fn bank(geometry: FloppyGeometry) -> Vec<u8> {
    let options = SlotFormatOptions {
        volume_serial: 42,
        update_time: time(),
        root_label_time: time(),
    };
    let mut output = Cursor::new(Vec::new());
    format_image(
        &mut output,
        &FormatOptions::new(geometry, 2, options).unwrap(),
    )
    .unwrap();
    output.into_inner()
}

#[test]
fn read_only_slot_reports_files_usage_metadata_and_exact_payload() {
    let bytes = bank(FloppyGeometry::Kb720);
    let mut disk = GotekDisk::open(Cursor::new(bytes.as_slice()), bytes.len() as u64).unwrap();
    let mut slot = disk.slot(1).unwrap();
    assert_eq!(slot.label().unwrap(), "FLPPY1");
    assert_eq!(slot.metadata().unwrap().update_time, Some(time()));
    assert!(slot.files().unwrap().is_empty());
    assert_eq!(slot.usage().unwrap().used_clusters, 0);
    assert!(slot.read_file("MISSING.TXT").is_err());
    let mut output = Vec::new();
    slot.save_image(&mut output).unwrap();
    let start = SLOT_STRIDE_BYTES as usize;
    assert_eq!(
        output,
        bytes[start..start + FloppyGeometry::Kb720.payload_bytes() as usize]
    );
}

#[test]
fn slot_usage_rejects_mismatched_fat_copies() {
    let mut bytes = bank(FloppyGeometry::Kb720);
    // In the 720K profile, FAT 2 starts after the boot sector and 3 FAT 1 sectors.
    bytes[4 * 512 + 12] ^= 1;
    let mut disk = GotekDisk::open(Cursor::new(bytes.as_slice()), bytes.len() as u64).unwrap();
    assert!(disk.slot(0).unwrap().usage().is_err());
}

#[test]
fn importing_then_reading_reuses_the_same_disk_and_preserves_other_bytes() {
    let before = bank(FloppyGeometry::Kb720);
    let mut disk = GotekDisk::open(Cursor::new(before.clone()), before.len() as u64).unwrap();
    let updated = UpdateTime {
        second: 1,
        ..time()
    };
    let mut slot = disk.slot_mut(1).unwrap();
    slot.import_file("HELLO.TXT", b"hello\0world", false, updated)
        .unwrap();
    assert_eq!(
        slot.as_slot().unwrap().read_file("hello.txt").unwrap(),
        b"hello\0world"
    );
    assert!(
        slot.as_slot()
            .unwrap()
            .files()
            .unwrap()
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case("HELLO.TXT"))
    );
    assert_eq!(slot.as_slot().unwrap().usage().unwrap().used_clusters, 1);
    assert_eq!(
        slot.as_slot().unwrap().metadata().unwrap().update_time,
        Some(updated)
    );
    assert!(
        slot.import_file("HELLO.TXT", b"other", false, updated)
            .is_err()
    );
    let after = disk.into_inner().into_inner();
    let start = SLOT_STRIDE_BYTES as usize;
    let payload_end = start + FloppyGeometry::Kb720.payload_bytes() as usize;
    let metadata = start + METADATA_OFFSET_BYTES as usize;
    assert_eq!(&after[..start], &before[..start]);
    assert_eq!(
        &after[payload_end..metadata],
        &before[payload_end..metadata]
    );
    assert_eq!(&after[metadata + 31..], &before[metadata + 31..]);
}

#[test]
fn load_export_refreshes_cached_bpb_and_rejects_mismatched_geometry() {
    let before = bank(FloppyGeometry::Kb720);
    let mut disk = GotekDisk::open(Cursor::new(before.clone()), before.len() as u64).unwrap();
    let wrong = bank(FloppyGeometry::Mb1440);
    let wrong_payload = wrong[..FloppyGeometry::Mb1440.payload_bytes() as usize].to_vec();
    assert!(
        disk.slot_mut(1)
            .unwrap()
            .load_image(&mut Cursor::new(wrong_payload), time())
            .is_err()
    );

    let mut source = before[..FloppyGeometry::Kb720.payload_bytes() as usize].to_vec();
    source[17..19].copy_from_slice(&96u16.to_le_bytes());
    {
        let fs =
            fatfs::FileSystem::new(Cursor::new(&mut source[..]), fatfs::FsOptions::new()).unwrap();
        fs.root_dir()
            .create_file("BOOT.TXT")
            .unwrap()
            .write_all(b"boot")
            .unwrap();
        fs.unmount().unwrap();
    }
    let updated = UpdateTime {
        second: 2,
        ..time()
    };
    disk.slot_mut(1)
        .unwrap()
        .load_image(&mut Cursor::new(source.clone()), updated)
        .unwrap();
    let mut slot = disk.slot(1).unwrap();
    assert_eq!(slot.bpb().root_entries, 96);
    assert_eq!(slot.read_file("BOOT.TXT").unwrap(), b"boot");
    assert_eq!(slot.metadata().unwrap().update_time, Some(updated));
    let mut exported = Vec::new();
    slot.save_image(&mut exported).unwrap();
    assert_eq!(exported, source);

    let after = disk.into_inner().into_inner();
    let start = SLOT_STRIDE_BYTES as usize;
    let end = start + source.len();
    let metadata = start + METADATA_OFFSET_BYTES as usize;
    assert_eq!(&after[..start], &before[..start]);
    assert_eq!(&after[start..end], source);
    assert_eq!(&after[end..metadata], &before[end..metadata]);
    let boundary = start + SLOT_STRIDE_BYTES as usize;
    assert_eq!(
        &after[metadata + 31..boundary],
        &before[metadata + 31..boundary]
    );
    assert_eq!(&after[boundary..boundary + 3], &[0, 0, 0]);
    assert_eq!(&after[boundary + 3..boundary + 512], &source[3..512]);
}
