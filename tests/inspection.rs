use gotek_core::{
    GotekError, WarningKind,
    fat12::Bpb,
    geometry::FloppyGeometry,
    inspect,
    layout::{METADATA_OFFSET_BYTES, SECTOR_BYTES, SLOT_STRIDE_BYTES, slot_offset},
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Cursor, Read, Seek, SeekFrom},
};

fn boot(geometry: FloppyGeometry, index: u16) -> [u8; SECTOR_BYTES] {
    let mut sector = [0; SECTOR_BYTES];
    sector[..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    sector[3..11].copy_from_slice(b"MSDOS5.0");
    put_word(&mut sector, 11, 512);
    let (clusters, entries, total, media, fat, track) = match geometry {
        FloppyGeometry::Kb720 => (2, 112, 1440, 0xf9, 3, 9),
        FloppyGeometry::Mb1200 => (2, 224, 2400, 0xf9, 7, 15),
        FloppyGeometry::Mb1440 => (1, 224, 2880, 0xf0, 9, 18),
    };
    sector[13] = clusters;
    put_word(&mut sector, 14, 1);
    sector[16] = 2;
    put_word(&mut sector, 17, entries);
    put_word(&mut sector, 19, total);
    sector[21] = media;
    put_word(&mut sector, 22, fat);
    put_word(&mut sector, 24, track);
    put_word(&mut sector, 26, 2);
    sector[38] = 0x29;
    sector[43..54].fill(b' ');
    let label = format!("FLPPY{index}");
    sector[43..43 + label.len()].copy_from_slice(label.as_bytes());
    sector[54..62].copy_from_slice(b"FAT12   ");
    sector[510..512].copy_from_slice(&[0x55, 0xaa]);
    sector
}

fn put_word(sector: &mut [u8; SECTOR_BYTES], offset: usize, value: u16) {
    sector[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

/// Stores only the sectors the inspector should read; all gaps read as zero.
/// Deliberately implements no Write and refuses SeekFrom::End, like a source
/// whose size cannot be queried. Reads are short to exercise read_exact's loop.
struct SparseImage {
    sectors: BTreeMap<u64, [u8; SECTOR_BYTES]>,
    position: u64,
    length: u64,
    bytes_read: usize,
    fail_seek: bool,
    fail_read: bool,
}

impl SparseImage {
    fn new(geometry: FloppyGeometry, count: u16) -> Self {
        let mut sectors = BTreeMap::new();
        for index in 0..count {
            let offset = slot_offset(u64::from(index)).unwrap();
            sectors.insert(offset, boot(geometry, index));
            let mut metadata = [0; SECTOR_BYTES];
            metadata[..31].copy_from_slice(b"UPDATE TIME:2026-10-31 13-12-19");
            if index == 0 {
                let record = format!("MAX BLOCK:{count:03}");
                metadata[0x50..0x5d].copy_from_slice(record.as_bytes());
            }
            sectors.insert(offset + METADATA_OFFSET_BYTES, metadata);
        }
        let boundary_offset = slot_offset(u64::from(count)).unwrap();
        let mut boundary = boot(geometry, count - 1);
        boundary[..3].fill(0);
        sectors.insert(boundary_offset, boundary);
        Self {
            sectors,
            position: 0,
            length: boundary_offset + 512,
            bytes_read: 0,
            fail_seek: false,
            fail_read: false,
        }
    }

    fn metadata(&mut self) -> &mut [u8; SECTOR_BYTES] {
        self.sectors.get_mut(&METADATA_OFFSET_BYTES).unwrap()
    }
}

impl Read for SparseImage {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.fail_read {
            return Err(io::Error::other("injected read failure"));
        }
        let remaining = self.length.saturating_sub(self.position);
        let count = buffer.len().min(remaining.min(37) as usize);
        for byte in &mut buffer[..count] {
            let base = self.position / 512 * 512;
            *byte = self
                .sectors
                .get(&base)
                .map_or(0, |sector| sector[(self.position % 512) as usize]);
            self.position += 1;
        }
        self.bytes_read += count;
        Ok(count)
    }
}

impl Seek for SparseImage {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        if self.fail_seek {
            return Err(io::Error::other("injected seek failure"));
        }
        self.position = match from {
            SeekFrom::Start(position) => position,
            SeekFrom::Current(delta) => self
                .position
                .checked_add_signed(delta)
                .ok_or_else(|| io::Error::other("seek overflow"))?,
            SeekFrom::End(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "length unavailable",
                ));
            }
        };
        Ok(self.position)
    }
}

#[test]
fn all_geometries_and_counts_read_only_the_required_sectors() {
    for (geometry, clusters) in [
        (FloppyGeometry::Kb720, 713),
        (FloppyGeometry::Mb1200, 1185),
        (FloppyGeometry::Mb1440, 2847),
    ] {
        for count in [1, 50, 100] {
            let mut image = SparseImage::new(geometry, count);
            image.position = 123; // Inspection always starts at absolute zero.
            image.length += 10_000_000_000; // A device may extend far beyond the boundary.
            let info = inspect(&mut image).unwrap();
            assert_eq!(info.geometry, geometry);
            assert_eq!(info.floppy_count, count);
            assert_eq!(info.slots.len(), usize::from(count));
            assert_eq!(info.boundary_offset, u64::from(count) * SLOT_STRIDE_BYTES);
            assert!(info.warnings.is_empty());
            for slot in &info.slots {
                assert_eq!(slot.offset, u64::from(slot.index) * SLOT_STRIDE_BYTES);
                assert_eq!(slot.volume_label, format!("FLPPY{}", slot.index));
                assert_eq!(slot.bpb.payload_bytes().unwrap(), geometry.payload_bytes());
                assert_eq!(slot.bpb.cluster_count, clusters);
                assert_eq!(slot.metadata.update_time.unwrap().year, 2026);
                assert!(slot.warnings.is_empty());
            }
            assert_eq!(image.bytes_read, (usize::from(count) * 2 + 1) * 512);
        }
    }
}

#[test]
fn cursor_input_is_inspected_without_modifying_its_bytes() {
    let sparse = SparseImage::new(FloppyGeometry::Mb1440, 1);
    let mut bytes = vec![0; sparse.length as usize];
    for (offset, sector) in sparse.sectors {
        bytes[offset as usize..offset as usize + 512].copy_from_slice(&sector);
    }
    let original = bytes.clone();
    let mut cursor = Cursor::new(bytes);
    inspect(&mut cursor).unwrap();
    assert_eq!(cursor.into_inner(), original);
}

#[test]
fn offsets_are_independent_of_payload_geometry_and_checked_for_overflow() {
    assert_eq!(slot_offset(0).unwrap(), 0);
    assert_eq!(slot_offset(49).unwrap(), 0x04980000);
    assert_eq!(slot_offset(50).unwrap(), 0x04b00000);
    assert_eq!(slot_offset(100).unwrap(), 0x09600000);
    assert!(matches!(
        slot_offset(u64::MAX),
        Err(GotekError::ArithmeticOverflow)
    ));
}

#[test]
fn malformed_count_records_are_errors_rather_than_size_based_guesses() {
    for record in [
        b"MAX BLOCK:50\0\0".as_slice(),
        b"MAX BLOCK:0x1\0",
        b"MAX BLOCK:001!",
        b"BAD BLOCK:001\0",
    ] {
        let mut image = SparseImage::new(FloppyGeometry::Kb720, 1);
        image.metadata()[0x50..0x5e].copy_from_slice(record);
        assert!(matches!(
            inspect(&mut image),
            Err(GotekError::InvalidCountRecord)
        ));
    }
    for (text, count) in [(b"000", 0), (b"101", 101), (b"999", 999)] {
        let mut image = SparseImage::new(FloppyGeometry::Kb720, 1);
        image.metadata()[0x5a..0x5d].copy_from_slice(text);
        assert!(
            matches!(inspect(&mut image), Err(GotekError::InvalidSlotCount(value)) if value == count)
        );
    }
}

#[test]
fn malformed_or_missing_update_times_warn_without_losing_slot_information() {
    for record in [
        b"UPDATE TIME:2026-02-29 13-12-19".as_slice(),
        b"UPDATE TIME:2026-10-31 24-00-00",
        b"UPDATE TIME:2026-13-31 13-12-19",
        b"UPDATE TIME:0000-10-31 13-12-19",
        b"UPDATE TIME:2026-10-31 13:12:19",
        b"UPDATE TIME:2026-10-31 13-60-19",
        b"UPDATE TIME:2026-10-31 13-12-60",
        b"UPDATE TIME:2026-10-00 13-12-19",
        b"UPDATE TIME:2026-10-31 13-12-xx",
        b"",
        &[0xff; 32],
    ] {
        let mut image = SparseImage::new(FloppyGeometry::Kb720, 1);
        image.metadata()[..32].fill(0);
        image.metadata()[..record.len()].copy_from_slice(record);
        let info = inspect(&mut image).unwrap();
        assert!(info.slots[0].metadata.update_time.is_none());
        assert!(
            info.slots[0]
                .warnings
                .iter()
                .any(|warning| warning.kind == WarningKind::InvalidUpdateTime)
        );
    }
}

#[test]
fn leap_day_and_local_timestamp_components_are_preserved() {
    let mut image = SparseImage::new(FloppyGeometry::Kb720, 1);
    image.metadata()[..31].copy_from_slice(b"UPDATE TIME:2024-02-29 23-59-59");
    let time = inspect(&mut image).unwrap().slots[0]
        .metadata
        .update_time
        .unwrap();
    assert_eq!(
        (
            time.year,
            time.month,
            time.day,
            time.hour,
            time.minute,
            time.second
        ),
        (2024, 2, 29, 23, 59, 59)
    );
}

#[test]
fn unsupported_payload_and_structurally_invalid_bpbs_are_errors() {
    let mut image = SparseImage::new(FloppyGeometry::Mb1440, 1);
    put_word(image.sectors.get_mut(&0).unwrap(), 19, 2800);
    assert!(matches!(
        inspect(&mut image),
        Err(GotekError::UnsupportedGeometry { .. })
    ));
    for (offset, value) in [
        (12, 0),
        (13, 0),
        (13, 3),
        (14, 0),
        (16, 0),
        (17, 0),
        (22, 0),
        (22, 1),
        (510, 0),
    ] {
        let mut image = SparseImage::new(FloppyGeometry::Mb1440, 1);
        image.sectors.get_mut(&0).unwrap()[offset] = value;
        assert!(
            matches!(inspect(&mut image), Err(GotekError::InvalidBpb { .. })),
            "field {offset}"
        );
    }
    let mut sector = boot(FloppyGeometry::Mb1440, 0);
    put_word(&mut sector, 19, 20);
    assert!(matches!(
        Bpb::parse(&sector, 0),
        Err(GotekError::InvalidBpb { .. })
    ));
    put_word(&mut sector, 19, 65535);
    assert!(matches!(
        Bpb::parse(&sector, 0),
        Err(GotekError::InvalidBpb { .. })
    ));
    sector[32] = 1;
    assert!(matches!(
        Bpb::parse(&sector, 0),
        Err(GotekError::InvalidBpb { .. })
    ));
}

#[test]
fn corruption_in_a_later_slot_reports_its_absolute_offset() {
    let mut image = SparseImage::new(FloppyGeometry::Kb720, 50);
    let offset = slot_offset(49).unwrap();
    image.sectors.get_mut(&offset).unwrap()[510] = 0;
    assert!(
        matches!(inspect(&mut image), Err(GotekError::InvalidBpb { offset: actual, .. }) if actual == offset)
    );
}

#[test]
fn nonessential_conventions_and_mixed_geometries_produce_warnings() {
    let mut image = SparseImage::new(FloppyGeometry::Mb1440, 2);
    let first = image.sectors.get_mut(&0).unwrap();
    first[3] = b'X';
    first[43] = 0xff;
    first[54] = b'X';
    first[38] = 0;
    image
        .sectors
        .insert(SLOT_STRIDE_BYTES, boot(FloppyGeometry::Kb720, 1));
    let info = inspect(&mut image).unwrap();
    assert!(
        info.slots[0]
            .warnings
            .iter()
            .any(|warning| warning.kind == WarningKind::NonAsciiLabel)
    );
    assert!(
        info.slots[1]
            .warnings
            .iter()
            .any(|warning| warning.kind == WarningKind::MixedGeometry)
    );
    assert_eq!(info.slots[1].geometry, FloppyGeometry::Kb720);
}

#[test]
fn boundary_copy_deviations_warn_but_invalid_structure_is_an_error() {
    let mut image = SparseImage::new(FloppyGeometry::Mb1440, 1);
    let boundary = image.sectors.get_mut(&SLOT_STRIDE_BYTES).unwrap();
    boundary[0] = 0xeb;
    boundary[100] ^= 1;
    let info = inspect(&mut image).unwrap();
    assert!(
        info.warnings
            .iter()
            .any(|warning| warning.kind == WarningKind::BoundaryJump)
    );
    assert!(
        info.warnings
            .iter()
            .any(|warning| warning.kind == WarningKind::BoundaryCopyMismatch)
    );
    image.sectors.get_mut(&SLOT_STRIDE_BYTES).unwrap()[510] = 0;
    assert!(matches!(
        inspect(&mut image),
        Err(GotekError::InvalidBpb {
            offset: SLOT_STRIDE_BYTES,
            ..
        })
    ));
}

#[test]
fn truncated_boot_metadata_later_slots_and_boundary_return_unexpected_eof() {
    for length in [
        0,
        511,
        METADATA_OFFSET_BYTES + 511,
        SLOT_STRIDE_BYTES + 511,
        SLOT_STRIDE_BYTES * 2 + 511,
    ] {
        let mut image = SparseImage::new(FloppyGeometry::Mb1440, 2);
        image.length = length;
        match inspect(&mut image) {
            Err(GotekError::Io { source, .. }) => {
                assert_eq!(source.kind(), io::ErrorKind::UnexpectedEof)
            }
            other => panic!("expected truncation error, got {other:?}"),
        }
    }
}

#[test]
fn seek_and_read_failures_keep_the_original_io_error() {
    for fail_seek in [false, true] {
        let mut image = SparseImage::new(FloppyGeometry::Mb1440, 1);
        image.fail_seek = fail_seek;
        image.fail_read = !fail_seek;
        let error = inspect(&mut image).unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        assert!(matches!(error, GotekError::Io { offset: 0, .. }));
    }
}

#[test]
fn available_ipcas_captures_are_inspected_without_writes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, geometry, count) in [
        ("image01.img", FloppyGeometry::Mb1440, 100),
        ("image02.img", FloppyGeometry::Mb1440, 100),
        ("image03.img", FloppyGeometry::Mb1440, 100),
        ("image04.img", FloppyGeometry::Mb1440, 50),
        ("image05.img", FloppyGeometry::Kb720, 100),
        ("image06.img", FloppyGeometry::Mb1200, 100),
    ] {
        let mut file = match File::open(root.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                eprintln!("Skipping optional local reference capture {name}");
                continue;
            }
            Err(error) => panic!("cannot open {name}: {error}"),
        };
        let info = inspect(&mut file).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            (info.geometry, info.floppy_count),
            (geometry, count),
            "{name}"
        );
        assert!(info.warnings.is_empty(), "{name}: {:?}", info.warnings);
        assert_eq!(info.slots[0].metadata.declared_slot_count, Some(count));
        for slot in info.slots {
            assert_eq!(slot.geometry, geometry);
            assert_eq!(slot.volume_label, format!("FLPPY{}", slot.index));
            assert!(slot.metadata.update_time.is_some());
            assert!(slot.warnings.is_empty(), "{name}: {:?}", slot.warnings);
        }
    }
}
