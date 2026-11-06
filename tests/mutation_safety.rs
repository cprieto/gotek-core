use gotek_core::{
    FormatOptions, GotekDisk, GotekError, SlotFormatOptions, format_image,
    geometry::FloppyGeometry, import_into_with_time, metadata::UpdateTime,
};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

fn time() -> UpdateTime {
    UpdateTime {
        year: 2026,
        month: 11,
        day: 6,
        hour: 12,
        minute: 0,
        second: 0,
    }
}

fn bank() -> Vec<u8> {
    let options = SlotFormatOptions {
        volume_serial: 42,
        update_time: time(),
        root_label_time: time(),
    };
    let mut output = Cursor::new(Vec::new());
    format_image(
        &mut output,
        &FormatOptions::with_slot_options(FloppyGeometry::Kb720, 2, options).unwrap(),
    )
    .unwrap();
    output.into_inner()
}

struct FailingTarget {
    inner: Cursor<Vec<u8>>,
    remaining: usize,
}

impl Read for FailingTarget {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}
impl Seek for FailingTarget {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}
impl Write for FailingTarget {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let count = self.remaining.min(buf.len());
        let written = self.inner.write(&buf[..count])?;
        self.remaining -= written;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn failing_disk(limit: usize) -> GotekDisk<FailingTarget> {
    let bytes = bank();
    let len = bytes.len() as u64;
    GotekDisk::open(
        FailingTarget {
            inner: Cursor::new(bytes),
            remaining: limit,
        },
        len,
    )
    .unwrap()
}

#[test]
fn partial_load_write_poisons_disk() {
    let mut disk = failing_disk(100);
    let source = bank();
    let payload = &source[..FloppyGeometry::Kb720.payload_bytes() as usize];
    {
        let mut slot = disk.slot_mut(1).unwrap();
        assert!(matches!(
            slot.load_image_with_time(&mut Cursor::new(payload), time()),
            Err(GotekError::Io { .. })
        ));
        assert!(matches!(slot.as_slot(), Err(GotekError::DiskNeedsReopen)));
    }
    assert!(matches!(disk.slot(0), Err(GotekError::DiskNeedsReopen)));
    assert!(matches!(disk.slot_mut(1), Err(GotekError::DiskNeedsReopen)));
}

#[test]
fn failed_final_boundary_update_poisons_disk() {
    let source = bank();
    let payload = &source[..FloppyGeometry::Kb720.payload_bytes() as usize];
    let mut disk = failing_disk(payload.len() + 512 + 10);
    let error = disk
        .slot_mut(1)
        .unwrap()
        .load_image_with_time(&mut Cursor::new(payload), time())
        .unwrap_err();
    assert!(matches!(error, GotekError::Io { .. }));
    assert!(matches!(disk.slot(0), Err(GotekError::DiskNeedsReopen)));
}

#[test]
fn failed_import_and_label_writes_poison_disk() {
    let mut disk = failing_disk(12);
    assert!(matches!(
        disk.slot_mut(0)
            .unwrap()
            .import_file_with_time("A.TXT", b"abc", false, time()),
        Err(GotekError::Io { .. })
    ));
    assert!(matches!(disk.slot(0), Err(GotekError::DiskNeedsReopen)));

    let mut disk = failing_disk(5);
    assert!(matches!(
        disk.slot_mut(0).unwrap().set_label("NEW LABEL"),
        Err(GotekError::Io { .. })
    ));
    assert!(matches!(disk.slot(1), Err(GotekError::DiskNeedsReopen)));
}

#[test]
fn validation_errors_keep_disk_usable_and_unchanged() {
    let before = bank();
    let mut disk = GotekDisk::open(Cursor::new(before.clone()), before.len() as u64).unwrap();
    let bad_time = UpdateTime {
        year: 10_000,
        ..time()
    };
    let wrong = bank();
    {
        let mut slot = disk.slot_mut(1).unwrap();
        assert!(
            slot.import_file_with_time("A.TXT", b"x", false, bad_time)
                .is_err()
        );
        assert!(slot.set_label("TOO LONG A LABEL").is_err());
        assert!(
            slot.load_image_with_time(&mut Cursor::new(&wrong[..10]), time())
                .is_err()
        );
        assert_eq!(slot.as_slot().unwrap().label().unwrap(), "FLPPY1");
    }
    assert!(disk.slot(0).is_ok());
    assert_eq!(disk.into_inner().into_inner(), before);
}

struct FailOutput;
impl Write for FailOutput {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::PermissionDenied.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn export_distinguishes_output_and_source_failures() {
    let bytes = bank();
    let mut disk = GotekDisk::open(Cursor::new(&bytes), bytes.len() as u64).unwrap();
    assert!(matches!(
        disk.slot(0).unwrap().save_image(&mut FailOutput),
        Err(GotekError::OutputIo { .. })
    ));

    // Opening needs to read slot zero. Fail only after the disk has opened.
    struct ToggleRead {
        inner: Cursor<Vec<u8>>,
        fail: std::rc::Rc<std::cell::Cell<bool>>,
    }
    impl Read for ToggleRead {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.fail.get() {
                Err(io::ErrorKind::PermissionDenied.into())
            } else {
                self.inner.read(buf)
            }
        }
    }
    impl Seek for ToggleRead {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
    }
    let fail = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut disk = GotekDisk::open(
        ToggleRead {
            inner: Cursor::new(bytes.clone()),
            fail: fail.clone(),
        },
        bytes.len() as u64,
    )
    .unwrap();
    let mut slot = disk.slot(0).unwrap();
    fail.set(true);
    assert!(matches!(
        slot.save_image(&mut Vec::new()),
        Err(GotekError::Io { .. })
    ));
}

#[test]
fn export_retries_interrupted_source_reads() {
    use std::{cell::Cell, rc::Rc};
    struct InterruptedReader {
        inner: Cursor<Vec<u8>>,
        interruptions: Rc<Cell<u8>>,
    }
    impl Read for InterruptedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let left = self.interruptions.get();
            if left > 0 {
                self.interruptions.set(left - 1);
                Err(io::ErrorKind::Interrupted.into())
            } else {
                self.inner.read(buf)
            }
        }
    }
    impl Seek for InterruptedReader {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
    }
    let bytes = bank();
    let interruptions = Rc::new(Cell::new(0));
    let reader = InterruptedReader {
        inner: Cursor::new(bytes.clone()),
        interruptions: interruptions.clone(),
    };
    let mut disk = GotekDisk::open(reader, bytes.len() as u64).unwrap();
    let mut slot = disk.slot(0).unwrap();
    interruptions.set(3);
    let mut output = Vec::new();
    slot.save_image(&mut output).unwrap();
    assert_eq!(interruptions.get(), 0);
    assert_eq!(
        output,
        bytes[..FloppyGeometry::Kb720.payload_bytes() as usize]
    );
}

#[test]
fn descriptor_and_slot_import_validate_consistently() {
    let before = bank();
    let capacity = before.len() as u64;
    let cases = [
        (1, "BAD/NAME", b"x".as_slice(), time()),
        (2, "A.TXT", b"x".as_slice(), time()),
        (2, "BAD/NAME", b"x".as_slice(), time()),
        (
            1,
            "A.TXT",
            b"x".as_slice(),
            UpdateTime { year: 0, ..time() },
        ),
    ];
    for (slot, name, data, timestamp) in cases {
        let mut legacy = Cursor::new(before.clone());
        let legacy_error =
            import_into_with_time(&mut legacy, capacity, slot, name, data, false, timestamp)
                .unwrap_err();
        let mut disk = GotekDisk::open(Cursor::new(before.clone()), capacity).unwrap();
        let slot_error = match disk.slot_mut(slot) {
            Ok(mut selected) => selected
                .import_file_with_time(name, data, false, timestamp)
                .unwrap_err(),
            Err(error) => error,
        };
        assert_eq!(legacy_error.to_string(), slot_error.to_string());
        assert_eq!(legacy.into_inner(), before);
        assert_eq!(disk.into_inner().into_inner(), before);
    }
    let mut legacy = Cursor::new(before.clone());
    import_into_with_time(
        &mut legacy,
        capacity,
        1,
        "HELLO.TXT",
        b"hello",
        false,
        time(),
    )
    .unwrap();
    let mut disk = GotekDisk::open(Cursor::new(before), capacity).unwrap();
    disk.slot_mut(1)
        .unwrap()
        .import_file_with_time("HELLO.TXT", b"hello", false, time())
        .unwrap();
    assert_eq!(
        disk.slot(1).unwrap().read_file("HELLO.TXT").unwrap(),
        b"hello"
    );
    let legacy_duplicate = import_into_with_time(
        &mut legacy,
        capacity,
        1,
        "HELLO.TXT",
        b"other",
        false,
        time(),
    )
    .unwrap_err();
    let slot_duplicate = disk
        .slot_mut(1)
        .unwrap()
        .import_file_with_time("HELLO.TXT", b"other", false, time())
        .unwrap_err();
    assert_eq!(legacy_duplicate.to_string(), slot_duplicate.to_string());
    assert_eq!(
        disk.slot(1).unwrap().read_file("HELLO.TXT").unwrap(),
        b"hello"
    );
}
