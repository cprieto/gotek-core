use std::{
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use gotek_core::{
    FormatOptions, GotekError, SlotFormatOptions, create_image,
    fat12::Bpb,
    format_image, format_on, generate_boot_sector,
    geometry::FloppyGeometry,
    inspect,
    layout::{METADATA_OFFSET_BYTES, SECTOR_BYTES, SLOT_STRIDE_BYTES, slot_offset},
    metadata::UpdateTime,
};

fn timestamp() -> UpdateTime {
    UpdateTime {
        year: 2026,
        month: 10,
        day: 31,
        hour: 13,
        minute: 12,
        second: 19,
    }
}

fn options(geometry: FloppyGeometry, count: u16) -> FormatOptions {
    FormatOptions::new(
        geometry,
        count,
        SlotFormatOptions {
            volume_serial: 0x1234_5678,
            update_time: timestamp(),
            root_label_time: timestamp(),
        },
    )
    .unwrap()
}

#[test]
fn descriptor_format_checks_capacity_before_writing_and_preserves_tail() {
    let options = options(FloppyGeometry::Kb720, 1);
    let required = options.image_size_bytes().unwrap();
    let before = vec![0x5a; required as usize + 16];
    let mut target = Cursor::new(before.clone());
    assert!(matches!(
        format_on(&mut target, required - 1, &options),
        Err(GotekError::ImageTooSmall { .. })
    ));
    assert_eq!(target.into_inner(), before);

    let mut target = Cursor::new(before.clone());
    format_on(&mut target, required, &options).unwrap();
    let after = target.into_inner();
    assert_eq!(&after[required as usize..], &before[required as usize..]);
    assert_eq!(inspect(&mut Cursor::new(&after)).unwrap().floppy_count, 1);
}

/// Each test owns a create_new file. Drop closes it before deleting the path.
struct TemporaryImage {
    file: Option<File>,
    path: PathBuf,
}

impl TemporaryImage {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "gotek-format-test-{}-{}.img",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        Self {
            file: Some(file),
            path,
        }
    }
    fn file(&mut self) -> &mut File {
        self.file.as_mut().unwrap()
    }
}

impl Drop for TemporaryImage {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn read_bytes(reader: &mut (impl Read + Seek), offset: u64, count: usize) -> Vec<u8> {
    reader.seek(SeekFrom::Start(offset)).unwrap();
    let mut bytes = vec![0; count];
    reader.read_exact(&mut bytes).unwrap();
    bytes
}

fn read_sector(reader: &mut (impl Read + Seek), offset: u64) -> [u8; SECTOR_BYTES] {
    read_bytes(reader, offset, SECTOR_BYTES).try_into().unwrap()
}

fn assert_fill(reader: &mut (impl Read + Seek), offset: u64, mut count: u64, expected: u8) {
    reader.seek(SeekFrom::Start(offset)).unwrap();
    let mut buffer = [0; 16 * 1024];
    while count != 0 {
        let chunk = count.min(buffer.len() as u64) as usize;
        reader.read_exact(&mut buffer[..chunk]).unwrap();
        assert!(
            buffer[..chunk].iter().all(|&byte| byte == expected),
            "fill differs near offset {offset:#x}"
        );
        count -= chunk as u64;
    }
}

#[test]
fn each_geometry_and_slot_count_formats_a_regular_file_that_inspects_without_warnings() {
    for geometry in [
        FloppyGeometry::Kb720,
        FloppyGeometry::Mb1200,
        FloppyGeometry::Mb1440,
    ] {
        for count in [1, 50, 100] {
            let options = options(geometry, count);
            let mut image = TemporaryImage::new();
            format_image(image.file(), &options).unwrap();
            assert_eq!(
                image.file().metadata().unwrap().len(),
                options.image_size_bytes().unwrap()
            );
            assert_eq!(
                image.file().stream_position().unwrap(),
                options.image_size_bytes().unwrap()
            );
            let info = inspect(image.file()).unwrap();
            assert_eq!((info.geometry, info.floppy_count), (geometry, count));
            assert!(info.warnings.is_empty());
            for slot in &info.slots {
                assert_eq!(slot.geometry, geometry);
                assert_eq!(slot.offset, slot_offset(u64::from(slot.index)).unwrap());
                assert_eq!(slot.volume_label, format!("FLPPY{}", slot.index));
                assert_eq!(slot.metadata.update_time, Some(timestamp()));
                assert!(slot.warnings.is_empty());
            }
            let last_boot = read_sector(image.file(), slot_offset(u64::from(count - 1)).unwrap());
            let boundary = read_sector(image.file(), info.boundary_offset);
            assert_eq!(&boundary[..3], &[0; 3]);
            assert_eq!(&boundary[3..], &last_boot[3..]);
        }
    }
}

#[test]
fn complete_slot_contents_match_fat_root_fill_gap_and_metadata_rules() {
    for geometry in [
        FloppyGeometry::Kb720,
        FloppyGeometry::Mb1200,
        FloppyGeometry::Mb1440,
    ] {
        let options = options(geometry, 2);
        let mut image = Cursor::new(Vec::new());
        format_image(&mut image, &options).unwrap();
        for index in 0..2 {
            let base = slot_offset(index).unwrap();
            let boot = read_sector(&mut image, base);
            let bpb = Bpb::parse(&boot, base).unwrap();
            let fat_bytes = u64::from(bpb.sectors_per_fat) * 512;
            let first_fat = read_bytes(&mut image, base + 512, fat_bytes as usize);
            let second_fat = read_bytes(&mut image, base + 512 + fat_bytes, fat_bytes as usize);
            assert_eq!(first_fat, second_fat);
            assert_eq!(&first_fat[..3], &[bpb.media_descriptor, 0xff, 0xff]);
            assert!(first_fat[3..].iter().all(|&byte| byte == 0));
            let root_offset = base + 512 + 2 * fat_bytes;
            let root_bytes = u64::from(bpb.root_entries) * 32;
            let root = read_bytes(&mut image, root_offset, root_bytes as usize);
            assert_eq!(&root[..11], &boot[43..54]);
            assert_eq!(root[11], 0x08);
            // 13:12:19 rounds down to 13:12:18 for FAT; the time word is
            // duplicated at the observed displaced offset 13 and normal offset 22.
            assert_eq!(&root[13..15], &0x6989_u16.to_le_bytes());
            assert_eq!(&root[22..24], &root[13..15]);
            assert_eq!(&root[16..18], &0x5d5f_u16.to_le_bytes());
            assert_eq!(&root[18..20], &root[16..18]);
            assert_eq!(&root[24..26], &root[16..18]);
            for offset in [12, 15, 20, 21, 26, 27, 28, 29, 30, 31] {
                assert_eq!(root[offset], 0);
            }
            assert!(root[32..].iter().all(|&byte| byte == 0));
            let data_start = bpb.first_data_sector * 512;
            let payload_end = geometry.payload_bytes();
            assert_fill(
                &mut image,
                base + data_start,
                payload_end - data_start - 512,
                0xf6,
            );
            assert_fill(&mut image, base + payload_end - 512, 512, 0);
            assert_fill(
                &mut image,
                base + payload_end,
                METADATA_OFFSET_BYTES - payload_end,
                0,
            );
            let metadata = read_sector(&mut image, base + METADATA_OFFSET_BYTES);
            let mut expected = [0; 512];
            expected[..32].copy_from_slice(b"UPDATE TIME:2026-10-31 13-12-19\0");
            if index == 0 {
                expected[0x50..0x5e].copy_from_slice(b"MAX BLOCK:002\0");
            }
            assert_eq!(metadata, expected);
        }
    }
}

#[test]
fn per_slot_serials_and_separate_root_and_update_times_are_deterministic() {
    let mut options = options(FloppyGeometry::Mb1440, 2);
    options.slots[1].volume_serial = 0xaabb_ccdd;
    options.slots[1].update_time.second = 25;
    options.slots[1].root_label_time.second = 20;
    let mut first = Cursor::new(Vec::new());
    let mut second = Cursor::new(Vec::new());
    format_image(&mut first, &options).unwrap();
    format_image(&mut second, &options).unwrap();
    assert_eq!(first.get_ref(), second.get_ref());
    let boot = read_sector(&mut first, SLOT_STRIDE_BYTES);
    assert_eq!(&boot[39..43], &0xaabb_ccdd_u32.to_le_bytes());
    let root = read_bytes(&mut first, SLOT_STRIDE_BYTES + 0x2600, 32);
    assert_eq!(&root[22..24], &0x698a_u16.to_le_bytes());
    let info = inspect(&mut first).unwrap();
    assert_eq!(info.slots[1].metadata.update_time.unwrap().second, 25);
}

fn golden_fixtures() -> [(FloppyGeometry, &'static [u8; 62], &'static [u8; 32]); 4] {
    [
        (
            FloppyGeometry::Mb1440,
            include_bytes!("fixtures/ipcas-1440-bpb.bin"),
            include_bytes!("fixtures/ipcas-1440-root-label.bin"),
        ),
        (
            FloppyGeometry::Mb1440,
            include_bytes!("fixtures/ipcas-1440-50-bpb.bin"),
            include_bytes!("fixtures/ipcas-1440-50-root-label.bin"),
        ),
        (
            FloppyGeometry::Kb720,
            include_bytes!("fixtures/ipcas-720-bpb.bin"),
            include_bytes!("fixtures/ipcas-720-root-label.bin"),
        ),
        (
            FloppyGeometry::Mb1200,
            include_bytes!("fixtures/ipcas-1200-bpb.bin"),
            include_bytes!("fixtures/ipcas-1200-root-label.bin"),
        ),
    ]
}

fn serial(boot: &[u8]) -> u32 {
    u32::from_le_bytes(boot[39..43].try_into().unwrap())
}

fn root_time(root: &[u8]) -> UpdateTime {
    let date = u16::from_le_bytes(root[24..26].try_into().unwrap());
    let time = u16::from_le_bytes(root[22..24].try_into().unwrap());
    UpdateTime {
        year: 1980 + (date >> 9),
        month: ((date >> 5) & 15) as u8,
        day: (date & 31) as u8,
        hour: (time >> 11) as u8,
        minute: ((time >> 5) & 63) as u8,
        second: ((time & 31) * 2) as u8,
    }
}

#[test]
fn captured_bpb_and_root_label_fixtures_match_exactly() {
    for (geometry, boot, root) in golden_fixtures() {
        let generated = generate_boot_sector(geometry, 0, serial(boot)).unwrap();
        assert_eq!(&generated[..62], boot);
        assert_eq!(&generated[510..], &[0x55, 0xaa]);
        let bpb = Bpb::parse(&generated, 0).unwrap();
        assert_eq!(bpb.payload_bytes().unwrap(), geometry.payload_bytes());
        let options = FormatOptions::new(
            geometry,
            1,
            SlotFormatOptions {
                volume_serial: serial(boot),
                update_time: timestamp(),
                root_label_time: root_time(root),
            },
        )
        .unwrap();
        let mut image = Cursor::new(Vec::new());
        format_image(&mut image, &options).unwrap();
        let root_offset = (u64::from(bpb.reserved_sectors)
            + u64::from(bpb.fat_count) * u64::from(bpb.sectors_per_fat))
            * 512;
        assert_eq!(read_bytes(&mut image, root_offset, 32), root);
    }
    for geometry in [
        FloppyGeometry::Kb720,
        FloppyGeometry::Mb1200,
        FloppyGeometry::Mb1440,
    ] {
        for index in [0, 49, 99] {
            let boot = generate_boot_sector(geometry, index, 42).unwrap();
            Bpb::parse(&boot, 0).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&boot[43..54]).trim_end(),
                format!("FLPPY{index}")
            );
        }
    }
    assert!(generate_boot_sector(FloppyGeometry::Kb720, 100, 0).is_err());
}

#[test]
fn invalid_options_including_later_slots_never_write() {
    for count in [0, 101] {
        assert!(matches!(
            FormatOptions::new(
                FloppyGeometry::Kb720,
                count,
                options(FloppyGeometry::Kb720, 1).slots[0]
            ),
            Err(GotekError::InvalidSlotCount(_))
        ));
    }
    let mut invalid = vec![FormatOptions {
        geometry: FloppyGeometry::Kb720,
        slots: vec![],
    }];
    let mut too_many = options(FloppyGeometry::Kb720, 100);
    too_many.slots.push(too_many.slots[0]);
    invalid.push(too_many);
    for bad_time in [
        UpdateTime {
            year: 0,
            ..timestamp()
        },
        UpdateTime {
            year: 10000,
            ..timestamp()
        },
        UpdateTime {
            month: 13,
            ..timestamp()
        },
        UpdateTime {
            month: 0,
            ..timestamp()
        },
        UpdateTime {
            month: 2,
            day: 29,
            ..timestamp()
        },
        UpdateTime {
            day: 0,
            ..timestamp()
        },
        UpdateTime {
            hour: 24,
            ..timestamp()
        },
        UpdateTime {
            minute: 60,
            ..timestamp()
        },
        UpdateTime {
            second: 60,
            ..timestamp()
        },
    ] {
        let mut options = options(FloppyGeometry::Kb720, 2);
        options.slots[1].update_time = bad_time;
        invalid.push(options);
    }
    for bad_year in [1979, 2108] {
        let mut options = options(FloppyGeometry::Kb720, 2);
        options.slots[1].root_label_time.year = bad_year;
        invalid.push(options);
    }
    for options in invalid {
        let mut writer = Cursor::new(Vec::new());
        assert!(format_image(&mut writer, &options).is_err());
        assert!(writer.into_inner().is_empty());
    }
}

#[test]
fn minimum_maximum_fat_dates_and_leap_day_are_supported() {
    for time in [
        UpdateTime {
            year: 1980,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        },
        UpdateTime {
            year: 2107,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
        },
        UpdateTime {
            year: 2000,
            month: 2,
            day: 29,
            ..timestamp()
        },
    ] {
        let mut options = options(FloppyGeometry::Kb720, 1);
        options.slots[0].root_label_time = time;
        options.slots[0].update_time = time;
        let mut image = Cursor::new(Vec::new());
        format_image(&mut image, &options).unwrap();
        assert_eq!(
            inspect(&mut image).unwrap().slots[0].metadata.update_time,
            Some(time)
        );
    }
}

#[test]
fn nonempty_destinations_and_existing_paths_are_preserved() {
    let options = options(FloppyGeometry::Kb720, 1);
    let mut writer = Cursor::new(b"preserve me".to_vec());
    assert!(matches!(
        format_image(&mut writer, &options),
        Err(GotekError::OutputNotEmpty(11))
    ));
    assert_eq!(writer.into_inner(), b"preserve me");
    let mut image = TemporaryImage::new();
    image.file().write_all(b"existing").unwrap();
    assert!(create_image(&image.path, &options).is_err());
    assert_eq!(fs::read(&image.path).unwrap(), b"existing");
}

#[test]
fn create_image_creates_a_new_regular_file_and_validates_before_creation() {
    let mut temporary = TemporaryImage::new();
    drop(temporary.file.take());
    fs::remove_file(&temporary.path).unwrap();
    let mut options = options(FloppyGeometry::Kb720, 1);
    options.slots[0].update_time.month = 0;
    assert!(create_image(&temporary.path, &options).is_err());
    assert!(!temporary.path.exists());
    options.slots[0].update_time.month = 10;
    create_image(&temporary.path, &options).unwrap();
    assert!(temporary.path.metadata().unwrap().is_file());
    let info = inspect(&mut File::open(&temporary.path).unwrap()).unwrap();
    assert_eq!(info.floppy_count, 1);
    assert!(info.warnings.is_empty());
}

/// A short-writing sink that enforces the calculated boundary on every write.
/// Selected failure points verify that write_all and error propagation work.
struct CheckedWriter {
    position: u64,
    length: u64,
    limit: u64,
    fail_after: Option<u64>,
    fail_seek: bool,
    fail_flush: bool,
}

impl Write for CheckedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert!(self.position + bytes.len() as u64 <= self.limit);
        if self.fail_after.is_some_and(|limit| self.position >= limit) {
            return Err(io::Error::other("injected write failure"));
        }
        let mut count = bytes.len().min(137) as u64;
        if let Some(limit) = self.fail_after {
            count = count.min(limit - self.position);
        }
        self.position += count;
        self.length = self.length.max(self.position);
        Ok(count as usize)
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.fail_flush {
            Err(io::Error::other("injected flush failure"))
        } else {
            Ok(())
        }
    }
}
impl Seek for CheckedWriter {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        if self.fail_seek {
            return Err(io::Error::other("injected seek failure"));
        }
        self.position = match from {
            SeekFrom::Start(position) => position,
            SeekFrom::End(0) => self.length,
            _ => return Err(io::Error::other("unsupported test seek")),
        };
        Ok(self.position)
    }
}

#[test]
fn short_writes_stay_within_the_image_and_io_failures_preserve_the_source() {
    let options = options(FloppyGeometry::Mb1200, 1);
    let new_writer = || CheckedWriter {
        position: 0,
        length: 0,
        limit: options.image_size_bytes().unwrap(),
        fail_after: None,
        fail_seek: false,
        fail_flush: false,
    };
    let mut writer = new_writer();
    format_image(&mut writer, &options).unwrap();
    assert_eq!(writer.length, writer.limit);
    for failure in [0, 1, 512, 10000, SLOT_STRIDE_BYTES, SLOT_STRIDE_BYTES + 511] {
        let mut writer = new_writer();
        writer.fail_after = Some(failure);
        let error = format_image(&mut writer, &options).unwrap_err();
        assert!(matches!(error, GotekError::Io { .. }));
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(writer.length, failure);
    }
    let mut writer = new_writer();
    writer.fail_seek = true;
    assert!(matches!(
        format_image(&mut writer, &options),
        Err(GotekError::Io { offset: 0, .. })
    ));
    assert_eq!(writer.length, 0);
    let mut writer = new_writer();
    writer.fail_flush = true;
    assert!(matches!(
        format_image(&mut writer, &options),
        Err(GotekError::Io { .. })
    ));
    assert_eq!(writer.length, writer.limit);
}

fn compare_capture(name: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    let mut reference = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            eprintln!("SKIPPED optional formatter golden comparison: {name} is unavailable");
            return;
        }
        Err(error) => panic!("cannot open {name}: {error}"),
    };
    let info = inspect(&mut reference).unwrap();
    let mut slots = Vec::new();
    for slot in &info.slots {
        let boot = read_sector(&mut reference, slot.offset);
        let root_offset = slot.offset
            + (u64::from(slot.bpb.reserved_sectors)
                + u64::from(slot.bpb.fat_count) * u64::from(slot.bpb.sectors_per_fat))
                * 512;
        let root = read_bytes(&mut reference, root_offset, 32);
        slots.push(SlotFormatOptions {
            volume_serial: serial(&boot),
            root_label_time: root_time(&root),
            update_time: slot.metadata.update_time.unwrap(),
        });
    }
    let options = FormatOptions {
        geometry: info.geometry,
        slots,
    };
    let mut generated = TemporaryImage::new();
    format_image(generated.file(), &options).unwrap();
    reference.seek(SeekFrom::Start(0)).unwrap();
    generated.file().seek(SeekFrom::Start(0)).unwrap();
    let mut remaining = options.image_size_bytes().unwrap();
    let mut offset = 0;
    let mut actual = [0; 64 * 1024];
    let mut expected = [0; 64 * 1024];
    while remaining != 0 {
        let chunk = remaining.min(actual.len() as u64) as usize;
        generated.file().read_exact(&mut actual[..chunk]).unwrap();
        reference.read_exact(&mut expected[..chunk]).unwrap();
        assert_reference_bytes(&actual[..chunk], &expected[..chunk], offset, name);
        remaining -= chunk as u64;
        offset += chunk as u64;
    }
    eprintln!(
        "Compared {name}: {offset} bytes through boundary; only boot-code regions 62..510 excluded"
    );
}

#[test]
fn optional_image02_matches_all_non_executable_bytes_through_the_boundary() {
    compare_capture("image02.img");
}
#[test]
fn optional_image04_matches_all_non_executable_bytes_through_the_boundary() {
    compare_capture("image04.img");
}
#[test]
fn optional_image05_matches_all_non_executable_bytes_through_the_boundary() {
    compare_capture("image05.img");
}
#[test]
fn optional_image06_matches_all_non_executable_bytes_through_the_boundary() {
    compare_capture("image06.img");
}

// The boundary is stride-aligned too, so the same rule handles its code copy.
// Never exclude bytes at these offsets in any other sector within a slot.
fn is_executable_boot_byte(offset: u64) -> bool {
    (62..510).contains(&(offset % SLOT_STRIDE_BYTES))
}

fn assert_reference_bytes(actual: &[u8], expected: &[u8], offset: u64, name: &str) {
    assert_eq!(actual.len(), expected.len());
    for (index, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
        let absolute = offset + index as u64;
        if !is_executable_boot_byte(absolute) {
            assert_eq!(actual, expected, "{name}: byte mismatch at {absolute:#x}");
        }
    }
}

#[test]
fn reference_comparison_excludes_only_executable_bytes_in_boot_and_boundary_sectors() {
    for base in [0, SLOT_STRIDE_BYTES, SLOT_STRIDE_BYTES * 100] {
        for relative in [
            0,
            2,
            3,
            11,
            61,
            510,
            511,
            512,
            574,
            METADATA_OFFSET_BYTES + 62,
        ] {
            assert!(!is_executable_boot_byte(base + relative));
        }
        for relative in [62, 100, 509] {
            assert!(is_executable_boot_byte(base + relative));
        }
    }
    let mut actual = [0; 512];
    actual[62..510].fill(0xaa);
    assert_reference_bytes(&actual, &[0; 512], 0, "intentional boot-code difference");
}

#[test]
fn generated_boot_code_is_original_consistent_and_reached_by_the_jump() {
    let baseline = generate_boot_sector(FloppyGeometry::Mb1440, 0, 0).unwrap();
    for geometry in [
        FloppyGeometry::Kb720,
        FloppyGeometry::Mb1200,
        FloppyGeometry::Mb1440,
    ] {
        for index in [0, 49, 99] {
            let boot = generate_boot_sector(geometry, index, u32::from(index)).unwrap();
            assert_eq!(&boot[..3], &[0xeb, 0x3c, 0x90]);
            let target = 2 + usize::from(boot[1]);
            assert_eq!(target, 62);
            // Prologue sets up a zero-segment stack and data segment; code does
            // not depend on which CS:IP alias the BIOS used to enter the sector.
            assert_eq!(
                &boot[target..target + 12],
                &[
                    0xfa, 0x31, 0xc0, 0x8e, 0xd0, 0xbc, 0x00, 0x7c, 0x8e, 0xd8, 0xfc, 0xfb
                ]
            );
            assert_eq!(&boot[62..510], &baseline[62..510]);
            let message_address = u16::from_le_bytes([boot[75], boot[76]]);
            let message_offset = usize::from(message_address - 0x7c00);
            assert_eq!(message_offset, 95);
            assert_eq!(
                &boot[message_offset..message_offset + 16],
                b"Non-system disk\0"
            );
            // The only interrupt instruction is BIOS display, never disk I/O.
            assert_eq!(&boot[87..89], &[0xcd, 0x10]);
            assert_eq!(&boot[91..95], &[0xfa, 0xf4, 0xeb, 0xfd]);
            assert!(boot[111..510].iter().all(|&byte| byte == 0));
            assert_eq!(&boot[510..], &[0x55, 0xaa]);
            let bpb = Bpb::parse(&boot, 0).unwrap();
            assert_eq!(bpb.bytes_per_sector, 512);
            assert_eq!(bpb.reserved_sectors, 1);
            assert_eq!(bpb.fat_count, 2);
            assert_eq!(bpb.heads, 2);
            assert_eq!(bpb.hidden_sectors, 0);
            assert_eq!(bpb.payload_bytes().unwrap(), geometry.payload_bytes());
        }
    }
}

#[test]
fn existing_image_rejects_invalid_options_before_modifying_the_file() {
    let mut image = TemporaryImage::new();
    image.file().write_all(b"original bytes").unwrap();
    let mut options = options(FloppyGeometry::Kb720, 2);
    options.slots[1].update_time.month = 0;
    assert!(matches!(
        gotek_core::format_existing_image(image.file(), &options),
        Err(GotekError::InvalidFormatOptions(_))
    ));
    assert_eq!(fs::read(&image.path).unwrap(), b"original bytes");
}

#[cfg(unix)]
#[test]
fn existing_image_checks_the_opened_handle_is_regular() {
    let mut special = File::open("/dev/null").unwrap();
    assert!(matches!(
        gotek_core::format_existing_image(&mut special, &options(FloppyGeometry::Kb720, 1)),
        Err(GotekError::NotRegularFile)
    ));
}
