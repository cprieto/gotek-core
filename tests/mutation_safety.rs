use gotek_core::{
    FormatOptions, GotekDisk, GotekError, SlotFormatOptions, format_image,
    geometry::FloppyGeometry, metadata::UpdateTime,
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
        &FormatOptions::new(FloppyGeometry::Kb720, 2, options).unwrap(),
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
            slot.load_image(&mut Cursor::new(payload), time()),
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
        .load_image(&mut Cursor::new(payload), time())
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
            .import_file("A.TXT", b"abc", false, time()),
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
        assert!(slot.import_file("A.TXT", b"x", false, bad_time).is_err());
        assert!(slot.set_label("TOO LONG A LABEL").is_err());
        assert!(
            slot.load_image(&mut Cursor::new(&wrong[..10]), time())
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
