use gotek_core::{
    FormatOptions, GotekDisk, GotekError, SlotFormatOptions, format_image,
    geometry::FloppyGeometry, inspect, layout::SLOT_STRIDE_BYTES, list_files, metadata::UpdateTime,
    read_file, save_image, slot_usage,
};
use std::{
    fs,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
};

fn image(count: u16) -> Vec<u8> {
    let time = UpdateTime {
        year: 2026,
        month: 11,
        day: 6,
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
        &FormatOptions::with_slot_options(FloppyGeometry::Kb720, count, slot).unwrap(),
    )
    .unwrap();
    output.into_inner()
}

#[test]
fn bad_later_slot_does_not_block_healthy_slot_or_convenience_readers() {
    let mut bytes = image(3);
    {
        let payload = &mut bytes[..FloppyGeometry::Kb720.payload_bytes() as usize];
        let fs = fatfs::FileSystem::new(Cursor::new(payload), fatfs::FsOptions::new()).unwrap();
        fs.root_dir()
            .create_file("HELLO.TXT")
            .unwrap()
            .write_all(b"hello")
            .unwrap();
        fs.unmount().unwrap();
    }
    bytes[2 * SLOT_STRIDE_BYTES as usize + 510] = 0;
    let original = bytes.clone();
    let mut disk = GotekDisk::open(Cursor::new(bytes.as_slice()), bytes.len() as u64).unwrap();
    assert_eq!(
        disk.slot(0).unwrap().read_file("HELLO.TXT").unwrap(),
        b"hello"
    );
    assert_eq!(disk.slot(0).unwrap().usage().unwrap().used_clusters, 1);
    assert!(matches!(disk.slot(2), Err(GotekError::InvalidBpb { .. })));
    assert_eq!(disk.into_inner().into_inner(), original.as_slice());
    assert!(inspect(&mut Cursor::new(bytes.as_slice())).is_err());

    let mut writable = GotekDisk::open(Cursor::new(bytes.clone()), bytes.len() as u64).unwrap();
    writable.slot_mut(0).unwrap();
    assert_eq!(writable.into_inner().into_inner(), original);

    assert_eq!(
        read_file(&mut Cursor::new(bytes.as_slice()), 0, "HELLO.TXT").unwrap(),
        b"hello"
    );
    assert!(
        list_files(&mut Cursor::new(bytes.as_slice()), 0)
            .unwrap()
            .iter()
            .any(|f| f.name == "HELLO.TXT")
    );
    assert_eq!(
        slot_usage(&mut Cursor::new(bytes.as_slice()), 0)
            .unwrap()
            .used_clusters,
        1
    );
    assert_eq!(bytes, original);
}

#[test]
fn missing_boundary_allows_reads_and_import_but_rejects_boundary_writes() {
    let mut bytes = image(1);
    bytes.truncate(SLOT_STRIDE_BYTES as usize);
    let original = bytes.clone();
    let mut disk = GotekDisk::open(Cursor::new(bytes.clone()), bytes.len() as u64).unwrap();
    let mut slot = disk.slot(0).unwrap();
    assert_eq!(slot.label().unwrap(), "FLPPY0");
    assert!(slot.files().unwrap().is_empty());
    assert_eq!(slot.usage().unwrap().used_clusters, 0);
    let mut exported = Vec::new();
    slot.save_image(&mut exported).unwrap();
    assert_eq!(
        exported,
        bytes[..FloppyGeometry::Kb720.payload_bytes() as usize]
    );
    let time = UpdateTime {
        year: 2026,
        month: 11,
        day: 6,
        hour: 12,
        minute: 0,
        second: 0,
    };
    let mut writable = disk.slot_mut(0).unwrap();
    assert!(matches!(
        writable.set_label("NEW LABEL"),
        Err(GotekError::ImageTooSmall { .. })
    ));
    assert!(matches!(
        writable.load_image_with_time(&mut Cursor::new(&exported), time),
        Err(GotekError::ImageTooSmall { .. })
    ));
    writable
        .import_file_with_time("HELLO.TXT", b"hello", false, time)
        .unwrap();
    assert_eq!(
        writable.as_slot().unwrap().read_file("HELLO.TXT").unwrap(),
        b"hello"
    );
    assert!(matches!(disk.slot(1), Err(GotekError::InvalidSlot { .. })));
    let after = disk.into_inner().into_inner();
    assert_eq!(
        &after[FloppyGeometry::Kb720.payload_bytes() as usize..],
        &original[FloppyGeometry::Kb720.payload_bytes() as usize..]
    );
    assert!(inspect(&mut Cursor::new(bytes.as_slice())).is_err());

    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "gotek-lazy-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let source = directory.join("source.img");
    let target = directory.join("slot.img");
    fs::write(&source, &bytes).unwrap();
    save_image(&source, 0, &target).unwrap();
    assert_eq!(fs::read(&target).unwrap(), exported);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn malformed_boundary_allows_import_but_blocks_boundary_writes() {
    let mut bytes = image(1);
    bytes[SLOT_STRIDE_BYTES as usize + 510] = 0;
    let mut disk = GotekDisk::open(Cursor::new(bytes.clone()), bytes.len() as u64).unwrap();
    assert_eq!(disk.slot(0).unwrap().geometry(), FloppyGeometry::Kb720);
    let time = UpdateTime {
        year: 2026,
        month: 11,
        day: 6,
        hour: 12,
        minute: 0,
        second: 0,
    };
    let mut slot = disk.slot_mut(0).unwrap();
    assert!(matches!(
        slot.set_label("NEW LABEL"),
        Err(GotekError::InvalidBpb { .. })
    ));
    slot.import_file_with_time("HELLO.TXT", b"hello", false, time)
        .unwrap();
    assert_eq!(
        slot.as_slot().unwrap().read_file("HELLO.TXT").unwrap(),
        b"hello"
    );
    let after = disk.into_inner().into_inner();
    assert_eq!(
        &after[SLOT_STRIDE_BYTES as usize..],
        &bytes[SLOT_STRIDE_BYTES as usize..]
    );
    assert!(inspect(&mut Cursor::new(bytes.as_slice())).is_err());
}

#[test]
fn nonfinal_mutations_do_not_require_a_boundary() {
    for damaged in [false, true] {
        let mut bytes = image(2);
        let boundary = 2 * SLOT_STRIDE_BYTES as usize;
        if damaged {
            bytes[boundary + 510] = 0;
        } else {
            bytes.truncate(boundary);
        }
        let before = bytes.clone();
        let time = UpdateTime {
            year: 2026,
            month: 11,
            day: 6,
            hour: 12,
            minute: 0,
            second: 0,
        };
        let payload = before[..FloppyGeometry::Kb720.payload_bytes() as usize].to_vec();
        let mut disk = GotekDisk::open(Cursor::new(bytes), before.len() as u64).unwrap();
        {
            let mut first = disk.slot_mut(0).unwrap();
            first.set_label("FIRST").unwrap();
            first
                .import_file_with_time("A.TXT", b"abc", false, time)
                .unwrap();
            assert_eq!(first.as_slot().unwrap().read_file("A.TXT").unwrap(), b"abc");
            first
                .load_image_with_time(&mut Cursor::new(&payload), time)
                .unwrap();
        }
        {
            let mut last = disk.slot_mut(1).unwrap();
            last.import_file_with_time("B.TXT", b"xyz", false, time)
                .unwrap();
            assert_eq!(last.as_slot().unwrap().read_file("B.TXT").unwrap(), b"xyz");
            assert!(last.set_label("LAST").is_err());
            assert!(
                last.load_image_with_time(&mut Cursor::new(&payload), time)
                    .is_err()
            );
        }
        let after = disk.into_inner().into_inner();
        assert_eq!(&after[boundary..], &before[boundary..]);
        assert_eq!(
            &after[FloppyGeometry::Kb720.payload_bytes() as usize..SLOT_STRIDE_BYTES as usize],
            &before[FloppyGeometry::Kb720.payload_bytes() as usize..SLOT_STRIDE_BYTES as usize]
        );
    }
}

#[test]
fn explicit_capacity_is_authoritative() {
    let bytes = image(1);
    assert!(matches!(
        GotekDisk::open(Cursor::new(bytes.as_slice()), SLOT_STRIDE_BYTES - 1),
        Err(GotekError::ImageTooSmall { .. })
    ));
    let mut disk = GotekDisk::open(Cursor::new(bytes), SLOT_STRIDE_BYTES).unwrap();
    assert_eq!(disk.slot(0).unwrap().geometry(), FloppyGeometry::Kb720);
    assert!(matches!(
        disk.slot_mut(0).unwrap().set_label("DATA"),
        Err(GotekError::ImageTooSmall { .. })
    ));
}

struct CountingReader {
    inner: Cursor<Vec<u8>>,
    boot_zero_reads: usize,
    boot_two_reads: usize,
    metadata_reads: usize,
    boundary_reads: usize,
}

impl Read for CountingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let offset = self.inner.position();
        if offset == 0 {
            self.boot_zero_reads += 1;
        }
        if offset == 2 * SLOT_STRIDE_BYTES {
            self.boot_two_reads += 1;
        }
        if offset == 0x17fe00 {
            self.metadata_reads += 1;
        }
        if offset == 3 * SLOT_STRIDE_BYTES {
            self.boundary_reads += 1;
        }
        self.inner.read(buf)
    }
}

impl Seek for CountingReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.inner.seek(from)
    }
}

#[test]
fn opening_and_repeated_access_read_each_boot_only_once() {
    let bytes = image(3);
    let reader = CountingReader {
        inner: Cursor::new(bytes.clone()),
        boot_zero_reads: 0,
        boot_two_reads: 0,
        metadata_reads: 0,
        boundary_reads: 0,
    };
    let mut disk = GotekDisk::open(reader, bytes.len() as u64).unwrap();
    disk.slot(0).unwrap();
    disk.slot(2).unwrap();
    disk.slot(2).unwrap();
    let reader = disk.into_inner();
    assert_eq!(reader.boot_zero_reads, 1);
    assert_eq!(reader.metadata_reads, 1);
    assert_eq!(reader.boot_two_reads, 1);
    assert_eq!(reader.boundary_reads, 0);
}
