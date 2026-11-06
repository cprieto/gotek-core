use crate::{
    GotekError,
    fat12::Bpb,
    inspect::{detect_geometry, read_sector},
    layout::{METADATA_OFFSET_BYTES, SECTOR_BYTES},
    metadata::{UpdateTime, encode_update_time},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Replace one slot's FAT12 payload with a complete floppy image of identical geometry.
///
/// Both paths must name regular files; the destination must not be a symlink or
/// the same underlying file as the source. All validation completes before writes.
/// The source boot sector and payload are copied verbatim. The selected slot's
/// `UPDATE TIME` is changed; its gap, other metadata, and other slots are
/// preserved. For the final slot, the extra boundary boot-sector copy is
/// synchronized with the imported boot sector. Back up the destination: a
/// write failure can leave partial data.
pub fn load_image(
    image: impl AsRef<Path>,
    slot: u16,
    source: impl AsRef<Path>,
    update_time: UpdateTime,
) -> Result<(), GotekError> {
    let record = encode_update_time(update_time)?;
    let image = image.as_ref();
    let source = source.as_ref();
    let destination_info =
        fs::symlink_metadata(image).map_err(|source| GotekError::DestinationIo {
            path: image.to_path_buf(),
            source,
        })?;
    if !destination_info.is_file() {
        return Err(GotekError::NotRegularFile);
    }
    let source_info = fs::metadata(source).map_err(|source_error| GotekError::DestinationIo {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if !source_info.is_file() {
        return Err(GotekError::NotRegularFile);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if destination_info.dev() == source_info.dev()
            && destination_info.ino() == source_info.ino()
        {
            return Err(GotekError::InvalidFormatOptions(
                "source and destination are the same file",
            ));
        }
    }
    if fs::canonicalize(image).ok() == fs::canonicalize(source).ok() {
        return Err(GotekError::InvalidFormatOptions(
            "source and destination are the same file",
        ));
    }
    let mut destination = OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .map_err(|source| GotekError::DestinationIo {
            path: image.to_path_buf(),
            source,
        })?;
    let opened = destination
        .metadata()
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    if !opened.is_file() {
        return Err(GotekError::NotRegularFile);
    }
    let mut source_file = File::open(source).map_err(|source_error| GotekError::DestinationIo {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let mut disk = crate::GotekDisk::open(&mut destination, opened.len())?;
    disk.slot_mut(slot)?
        .load_image_with_record(&mut source_file, &record)
}

/// Save a slot's complete FAT12 payload as a standalone floppy image.
///
/// Validates slot zero, the declared capacity, and the selected slot before
/// creating the output. Other slots and the extra boundary are not required.
/// The payload is copied byte-for-byte; Gotek metadata, slot padding, and the
/// boundary sector are excluded. Existing destinations are refused. If copying
/// or flushing fails, the incomplete output is removed when possible.
pub fn save_image(
    image: impl AsRef<Path>,
    slot: u16,
    destination: impl AsRef<Path>,
) -> Result<(), GotekError> {
    let image = image.as_ref();
    let destination = destination.as_ref();
    let mut source = File::open(image).map_err(|error| GotekError::DestinationIo {
        path: image.to_path_buf(),
        source: error,
    })?;
    let capacity = source
        .seek(SeekFrom::End(0))
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    let mut disk = crate::GotekDisk::open(source, capacity)?;
    let mut selected = disk.slot(slot)?;
    // An existing path, including a symlink or hard link to the source, is
    // rejected by create_new. Check it before touching the output directory.
    if fs::symlink_metadata(destination).is_ok() {
        return Err(GotekError::DestinationIo {
            path: destination.to_path_buf(),
            source: std::io::Error::from(std::io::ErrorKind::AlreadyExists),
        });
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| GotekError::DestinationIo {
            path: destination.to_path_buf(),
            source: error,
        })?;
    let result = selected.save_image(&mut output);
    drop(output);
    if let Err(error) = result {
        let _ = fs::remove_file(destination);
        return Err(match error {
            GotekError::OutputIo { source } => GotekError::DestinationIo {
                path: destination.to_path_buf(),
                source,
            },
            other => other,
        });
    }
    Ok(())
}

pub(crate) struct LoadTarget {
    pub(crate) start: u64,
    pub(crate) geometry: crate::geometry::FloppyGeometry,
    pub(crate) boundary_offset: Option<u64>,
}

/// Replace a slot already validated by `GotekDisk`, including its boundary when needed.
pub(crate) fn load_at<T: Read + Write + Seek + ?Sized, S: Read + Seek + ?Sized>(
    target: &mut T,
    destination: LoadTarget,
    source: &mut S,
    record: &[u8; 31],
) -> Result<Bpb, GotekError> {
    let LoadTarget {
        start,
        geometry: dest_geometry,
        boundary_offset,
    } = destination;
    let source_bpb = Bpb::parse(&read_sector(source, 0)?, 0)?;
    let source_geometry = detect_geometry(&source_bpb, 0)?;
    if source_geometry != dest_geometry {
        return Err(GotekError::GeometryMismatch {
            expected: dest_geometry,
            actual: source_geometry,
        });
    }
    let expected = source_geometry.payload_bytes();
    let actual = source
        .seek(SeekFrom::End(0))
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    if actual != expected {
        return Err(GotekError::ImageSizeMismatch { expected, actual });
    }
    let metadata_offset = start
        .checked_add(METADATA_OFFSET_BYTES)
        .ok_or(GotekError::ArithmeticOverflow)?;
    let mut metadata = read_sector(target, metadata_offset)?;
    source
        .seek(SeekFrom::Start(0))
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    let mut payload = vec![0; expected as usize];
    source
        .read_exact(&mut payload)
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    metadata[..31].copy_from_slice(record);
    let boundary_sector = boundary_offset.map(|_| {
        let mut sector = [0; SECTOR_BYTES];
        sector[3..].copy_from_slice(&payload[3..SECTOR_BYTES]);
        sector
    });
    target
        .seek(SeekFrom::Start(start))
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    target
        .write_all(&payload)
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    target
        .seek(SeekFrom::Start(metadata_offset))
        .map_err(|source| GotekError::Io {
            offset: metadata_offset,
            source,
        })?;
    target
        .write_all(&metadata)
        .map_err(|source| GotekError::Io {
            offset: metadata_offset,
            source,
        })?;
    if let (Some(offset), Some(sector)) = (boundary_offset, boundary_sector) {
        target
            .seek(SeekFrom::Start(offset))
            .map_err(|source| GotekError::Io { offset, source })?;
        target
            .write_all(&sector)
            .map_err(|source| GotekError::Io { offset, source })?;
    }
    target.flush().map_err(|source| GotekError::Io {
        offset: metadata_offset,
        source,
    })?;
    Ok(source_bpb)
}

/// Copy only a validated FAT12 payload to an independent writer.
pub(crate) fn save_at<R: Read + Seek + ?Sized, W: Write + ?Sized>(
    reader: &mut R,
    start: u64,
    length: u64,
    writer: &mut W,
) -> Result<(), GotekError> {
    reader
        .seek(SeekFrom::Start(start))
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    let mut copied = 0;
    let mut buffer = [0; 16 * 1024];
    while copied < length {
        let remaining = length - copied;
        let count = buffer.len().min(remaining as usize);
        let read = match reader.read(&mut buffer[..count]) {
            Ok(read) => read,
            Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(source) => {
                return Err(GotekError::Io {
                    offset: start + copied,
                    source,
                });
            }
        };
        if read == 0 {
            return Err(GotekError::Io {
                offset: start + copied,
                source: std::io::Error::from(std::io::ErrorKind::UnexpectedEof),
            });
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|source| GotekError::OutputIo { source })?;
        copied += read as u64;
    }
    writer
        .flush()
        .map_err(|source| GotekError::OutputIo { source })
}
