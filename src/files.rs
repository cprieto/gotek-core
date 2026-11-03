use std::{
    fs::OpenOptions,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

use crate::{
    GotekError,
    fat12::Bpb,
    inspect::{detect_geometry, read_sector},
    layout::{METADATA_OFFSET_BYTES, slot_offset},
    metadata::parse_count,
};

/// A root-directory file or subdirectory (volume labels and dot entries are omitted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// VFAT long filename when available, otherwise the short filename.
    pub name: String,
    /// File length in bytes; directories normally report zero.
    pub size: u64,
    /// Raw FAT attribute bits: read-only=1, hidden=2, system=4, volume=8,
    /// directory=16, archive=32.
    pub attributes: u8,
}

/// List a zero-based slot's root directory in on-disk order, without writing.
///
/// Validates the declared slot count and selected BPB, independently of other
/// slots and the boundary copy. The reader's final position is unspecified.
/// This is filesystem access, not a complete FAT integrity check.
pub fn list_files<R: Read + Seek + ?Sized>(
    reader: &mut R,
    slot: u16,
) -> Result<Vec<FileInfo>, GotekError> {
    let volume = SlotReader::new(reader, slot)?;
    let offset = volume.start;
    let fs = fatfs::FileSystem::new(volume, fatfs::FsOptions::new().update_accessed_date(false))
        .map_err(|source| GotekError::Io { offset, source })?;
    fs.root_dir()
        .iter()
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_name() == "." || entry.file_name() == ".." => None,
            Ok(entry) => Some(Ok(FileInfo {
                name: entry.file_name(),
                size: entry.len(),
                attributes: entry.attributes().bits(),
            })),
            Err(source) => Some(Err(GotekError::Io { offset, source })),
        })
        .collect()
}

/// Read a file from a zero-based slot without modifying the source.
///
/// Accepts ASCII case-insensitive short or VFAT names and slash-separated relative
/// paths. Empty, absolute, dot, and parent components are rejected. File sizes
/// larger than the volume and truncated chains return errors. Memory use is
/// bounded by the selected floppy's payload size. Reader position is unspecified.
pub fn read_file<R: Read + Seek + ?Sized>(
    reader: &mut R,
    slot: u16,
    filename: &str,
) -> Result<Vec<u8>, GotekError> {
    let volume = SlotReader::new(reader, slot)?;
    let offset = volume.start;
    let limit = volume.length;
    let run = || -> io::Result<Vec<u8>> {
        if filename
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
            || filename.contains('\\')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a relative FAT file path",
            ));
        }
        let fs =
            fatfs::FileSystem::new(volume, fatfs::FsOptions::new().update_accessed_date(false))?;
        let mut directory = fs.root_dir();
        let mut components = filename.split('/').peekable();
        let mut budget = limit / 32;
        let entry = loop {
            let component = components.next().unwrap();
            let mut found = None;
            for candidate in directory.iter() {
                if budget == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "directory traversal exceeds volume size",
                    ));
                }
                budget -= 1;
                let candidate = candidate?;
                if candidate.file_name().eq_ignore_ascii_case(component)
                    || candidate.short_file_name().eq_ignore_ascii_case(component)
                {
                    found = Some(candidate);
                    break;
                }
            }
            let entry =
                found.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))?;
            if components.peek().is_none() {
                break entry;
            }
            if !entry.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path component is not a directory",
                ));
            }
            directory = entry.to_dir();
        };
        if !entry.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path is not a file",
            ));
        }
        let length = entry.len();
        if length > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file exceeds volume size",
            ));
        }
        let mut file = entry.to_file();
        let mut bytes = vec![0; length as usize];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    };
    run().map_err(|source| GotekError::Io { offset, source })
}

/// Extract a slot file to a new host filesystem path without modifying the source.
/// `filename` is a `/`-separated FAT path; `destination` accepts any host path.
///
/// Reuses [`read_file`], buffering at most one floppy payload in memory. The
/// destination is created atomically and existing paths (including symlinks) are
/// refused. Parent directories must already exist. Source errors occur before
/// destination creation; destination I/O errors may leave a partial file.
/// Reader position is unspecified. Successful writes are flushed, not synced.
///
/// ```no_run
/// let mut image = std::fs::File::open("image.img")?;
/// gotek_core::extract_file(&mut image, 5, "HELLO.TXT", std::path::Path::new("output/HELLO.TXT"))?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn extract_file<R: Read + Seek + ?Sized>(
    reader: &mut R,
    slot: u16,
    filename: &str,
    destination: impl AsRef<Path>,
) -> Result<(), GotekError> {
    let bytes = read_file(reader, slot, filename)?;
    let path = destination.as_ref();
    let save = || -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(&bytes)?;
        file.flush()
    };
    save().map_err(|source| GotekError::DestinationIo {
        path: path.to_path_buf(),
        source,
    })
}

// fatfs requires Write even for readers. Never forward writes to the source.
struct SlotReader<'a, R: ?Sized> {
    reader: &'a mut R,
    start: u64,
    length: u64,
    position: u64,
}

impl<'a, R: Read + Seek + ?Sized> SlotReader<'a, R> {
    fn new(reader: &'a mut R, index: u16) -> Result<Self, GotekError> {
        let count = parse_count(&read_sector(reader, METADATA_OFFSET_BYTES)?)?;
        if index >= count {
            return Err(GotekError::InvalidSlot { index, count });
        }
        let start = slot_offset(u64::from(index))?;
        let bpb = Bpb::parse(&read_sector(reader, start)?, start)?;
        detect_geometry(&bpb, start)?;
        Ok(Self {
            reader,
            start,
            length: bpb.payload_bytes()?,
            position: 0,
        })
    }
}

impl<R: Read + Seek + ?Sized> Read for SlotReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = buffer.len().min((self.length - self.position) as usize);
        if count == 0 {
            return Ok(0);
        }
        self.reader
            .seek(SeekFrom::Start(self.start + self.position))?;
        let read = self.reader.read(&mut buffer[..count])?;
        self.position += read as u64;
        Ok(read)
    }
}

impl<R: ?Sized> Seek for SlotReader<'_, R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.length) + i128::from(delta),
        };
        if !(0..=i128::from(self.length)).contains(&position) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek outside FAT12 volume",
            ));
        }
        self.position = position as u64;
        Ok(self.position)
    }
}

impl<R: ?Sized> Write for SlotReader<'_, R> {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "read-only FAT12 volume",
        ))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn adapter_caps_reads_and_rejects_seeks_and_writes() {
        let mut source = Cursor::new(vec![0x5a; 2048]);
        let mut volume = SlotReader {
            reader: &mut source,
            start: 512,
            length: 512,
            position: 0,
        };
        assert_eq!(volume.seek(SeekFrom::End(-1)).unwrap(), 511);
        let mut buffer = [0; 1024];
        assert_eq!(volume.read(&mut buffer).unwrap(), 1);
        assert_eq!(volume.read(&mut buffer).unwrap(), 0);
        for seek in [
            SeekFrom::Start(513),
            SeekFrom::End(1),
            SeekFrom::Current(i64::MAX),
            SeekFrom::Start(u64::MAX),
        ] {
            assert!(volume.seek(seek).is_err());
        }
        assert_eq!(volume.stream_position().unwrap(), 512);
        assert!(volume.seek(SeekFrom::Current(-513)).is_err());
        assert_eq!(
            volume.write(b"x").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(source.position(), 1024);
        assert!(source.get_ref().iter().all(|&byte| byte == 0x5a));
    }
}
