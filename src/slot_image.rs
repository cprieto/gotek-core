use crate::{
    GotekError,
    fat12::Bpb,
    inspect::{detect_geometry, read_sector},
    layout::{METADATA_OFFSET_BYTES, SECTOR_BYTES, slot_offset},
    metadata::{UpdateTime, parse_count, parse_update},
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
/// `UPDATE TIME` is changed; its gap, other metadata, other slots, and boundary
/// are preserved. Back up the destination: a write failure can leave partial data.
pub fn load_image(
    image: impl AsRef<Path>,
    slot: u16,
    source: impl AsRef<Path>,
    update_time: UpdateTime,
) -> Result<(), GotekError> {
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
    let record = format!(
        "UPDATE TIME:{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
        update_time.year,
        update_time.month,
        update_time.day,
        update_time.hour,
        update_time.minute,
        update_time.second
    );
    let mut checked = [0; 32];
    if record.len() != 31 {
        return Err(GotekError::InvalidFormatOptions("invalid update timestamp"));
    }
    checked[..31].copy_from_slice(record.as_bytes());
    if parse_update(&checked).is_none() {
        return Err(GotekError::InvalidFormatOptions("invalid update timestamp"));
    }
    let mut destination = OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .map_err(|source| GotekError::DestinationIo {
            path: image.to_path_buf(),
            source,
        })?;
    if !destination
        .metadata()
        .map_err(|source| GotekError::Io { offset: 0, source })?
        .is_file()
    {
        return Err(GotekError::NotRegularFile);
    }
    let mut source_file = File::open(source).map_err(|source_error| GotekError::DestinationIo {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let count = parse_count(&read_sector(&mut destination, METADATA_OFFSET_BYTES)?)?;
    if slot >= count {
        return Err(GotekError::InvalidSlot { index: slot, count });
    }
    let start = slot_offset(u64::from(slot))?;
    let dest_bpb = Bpb::parse(&read_sector(&mut destination, start)?, start)?;
    let dest_geometry = detect_geometry(&dest_bpb, start)?;
    let source_bpb = Bpb::parse(&read_sector(&mut source_file, 0)?, 0)?;
    let source_geometry = detect_geometry(&source_bpb, 0)?;
    if source_geometry != dest_geometry {
        return Err(GotekError::GeometryMismatch {
            expected: dest_geometry,
            actual: source_geometry,
        });
    }
    let expected = source_geometry.payload_bytes();
    if source_info.len() != expected {
        return Err(GotekError::ImageSizeMismatch {
            expected,
            actual: source_info.len(),
        });
    }
    let metadata_offset = start
        .checked_add(METADATA_OFFSET_BYTES)
        .ok_or(GotekError::ArithmeticOverflow)?;
    let required = metadata_offset
        .checked_add(SECTOR_BYTES as u64)
        .ok_or(GotekError::ArithmeticOverflow)?;
    let actual = destination
        .metadata()
        .map_err(|source| GotekError::Io { offset: 0, source })?
        .len();
    if actual < required {
        return Err(GotekError::ImageTooSmall {
            actual_bytes: actual,
            required_bytes: required,
        });
    }
    let mut metadata = read_sector(&mut destination, metadata_offset)?;
    // Hold the complete source in memory so a short read cannot partially replace a slot.
    source_file
        .seek(SeekFrom::Start(0))
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    let mut payload = vec![0; expected as usize];
    source_file
        .read_exact(&mut payload)
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    metadata[..31].copy_from_slice(record.as_bytes());
    destination
        .seek(SeekFrom::Start(start))
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    destination
        .write_all(&payload)
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    destination
        .seek(SeekFrom::Start(metadata_offset))
        .map_err(|source| GotekError::Io {
            offset: metadata_offset,
            source,
        })?;
    destination
        .write_all(&metadata)
        .map_err(|source| GotekError::Io {
            offset: metadata_offset,
            source,
        })?;
    destination.flush().map_err(|source| GotekError::Io {
        offset: metadata_offset,
        source,
    })
}

/// Save a slot's complete FAT12 payload as a standalone floppy image.
///
/// Validates the Gotek container and selected slot before creating the output.
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
    let info = crate::inspect(&mut source)?;
    if slot >= info.floppy_count {
        return Err(GotekError::InvalidSlot {
            index: slot,
            count: info.floppy_count,
        });
    }
    let selected = &info.slots[usize::from(slot)];
    let start = selected.offset;
    let length = selected.geometry.payload_bytes();
    let end = start
        .checked_add(length)
        .ok_or(GotekError::ArithmeticOverflow)?;
    let actual = source
        .metadata()
        .map_err(|error| GotekError::Io {
            offset: 0,
            source: error,
        })?
        .len();
    if actual < end {
        return Err(GotekError::ImageTooSmall {
            actual_bytes: actual,
            required_bytes: end,
        });
    }
    // An existing path, including a symlink or hard link to the source, is
    // rejected by create_new. Check it before touching the output directory.
    if fs::symlink_metadata(destination).is_ok() {
        return Err(GotekError::DestinationIo {
            path: destination.to_path_buf(),
            source: std::io::Error::from(std::io::ErrorKind::AlreadyExists),
        });
    }
    source
        .seek(SeekFrom::Start(start))
        .map_err(|error| GotekError::Io {
            offset: start,
            source: error,
        })?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| GotekError::DestinationIo {
            path: destination.to_path_buf(),
            source: error,
        })?;
    let result = (|| -> std::io::Result<()> {
        let copied = std::io::copy(&mut Read::by_ref(&mut source).take(length), &mut output)?;
        if copied != length {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        output.flush()
    })();
    drop(output);
    if let Err(error) = result {
        let _ = fs::remove_file(destination);
        return Err(GotekError::DestinationIo {
            path: destination.to_path_buf(),
            source: error,
        });
    }
    Ok(())
}
