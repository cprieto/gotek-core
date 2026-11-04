use crate::{
    GotekError,
    fat12::Bpb,
    inspect::{detect_geometry, read_sector},
    layout::{METADATA_OFFSET_BYTES, slot_offset},
    metadata::{UpdateTime, parse_count, parse_update},
};
use std::{
    fs::{self, OpenOptions},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Import one local file into a slot's root directory.
///
/// `image` and `source` accept host filesystem paths. `name` is a UTF-8 FAT
/// filename and defaults to the source basename. Only a single FAT filename is
/// accepted. `overwrite` truncates an existing regular file before writing.
/// The image must be an existing regular file, not a symlink or device.
/// Errors during writing can leave partial changes.
pub fn import_file(
    image: impl AsRef<Path>,
    slot: u16,
    source: impl AsRef<Path>,
    name: Option<&str>,
    overwrite: bool,
    update_time: UpdateTime,
) -> Result<(), GotekError> {
    let image = image.as_ref();
    let source = source.as_ref();
    let metadata = fs::symlink_metadata(image).map_err(|source| GotekError::DestinationIo {
        path: image.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(GotekError::NotRegularFile);
    }
    let name = match name {
        Some(name) => name,
        None => source.file_name().and_then(|value| value.to_str()).ok_or(
            GotekError::InvalidFormatOptions(
                "source has no UTF-8 basename; specify a FAT filename",
            ),
        )?,
    };
    // Keep the path API's source-read-before-target-open ordering.
    validate_name_and_time(name, update_time)?;
    let bytes = fs::read(source).map_err(|error| GotekError::DestinationIo {
        path: source.to_path_buf(),
        source: error,
    })?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(image)
        .map_err(|source| GotekError::DestinationIo {
            path: image.to_path_buf(),
            source,
        })?;
    let capacity = file
        .metadata()
        .map_err(|source| GotekError::DestinationIo {
            path: image.to_path_buf(),
            source,
        })?;
    if !capacity.is_file() {
        return Err(GotekError::NotRegularFile);
    }
    import_into(
        &mut file,
        capacity.len(),
        slot,
        name,
        &bytes,
        overwrite,
        update_time,
    )
}

/// Import bytes into an already-open Gotek image or device descriptor.
///
/// `capacity` is the descriptor's total accessible byte length, including the
/// selected slot's metadata sector. The target must point at slot zero at byte
/// zero; its initial cursor position is ignored. `name` is one FAT filename,
/// without directory components. `overwrite` truncates an existing regular FAT
/// file before replacing it. All feasible checks and FAT changes are completed
/// in memory before the first target write. Writes are limited to the selected
/// BPB payload and its `UPDATE TIME` metadata sector. A write failure can leave
/// part of either region updated; physical-device writes are not power-failure
/// atomic. The caller must ensure exclusive access and accurate capacity.
pub fn import_into<T: Read + Write + Seek + ?Sized>(
    target: &mut T,
    capacity: u64,
    slot: u16,
    name: &str,
    bytes: &[u8],
    overwrite: bool,
    update_time: UpdateTime,
) -> Result<(), GotekError> {
    let record = validate_name_and_time(name, update_time)?;
    let minimum = METADATA_OFFSET_BYTES
        .checked_add(512)
        .ok_or(GotekError::ArithmeticOverflow)?;
    if capacity < minimum {
        return Err(GotekError::ImageTooSmall {
            actual_bytes: capacity,
            required_bytes: minimum,
        });
    }
    let count = parse_count(&read_sector(target, METADATA_OFFSET_BYTES)?)?;
    if slot >= count {
        return Err(GotekError::InvalidSlot { index: slot, count });
    }
    let start = slot_offset(u64::from(slot))?;
    let bpb = Bpb::parse(&read_sector(target, start)?, start)?;
    detect_geometry(&bpb, start)?;
    let limit = bpb.payload_bytes()?;
    if bytes.len() as u64 > limit {
        return Err(GotekError::InvalidFormatOptions(
            "source exceeds FAT12 payload",
        ));
    }
    let metadata_offset = start
        .checked_add(METADATA_OFFSET_BYTES)
        .ok_or(GotekError::ArithmeticOverflow)?;
    let required = metadata_offset
        .checked_add(512)
        .ok_or(GotekError::ArithmeticOverflow)?;
    if capacity < required {
        return Err(GotekError::ImageTooSmall {
            actual_bytes: capacity,
            required_bytes: required,
        });
    }
    let mut payload = vec![0; limit as usize];
    target
        .seek(SeekFrom::Start(start))
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    target
        .read_exact(&mut payload)
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    let mut timestamp_sector = read_sector(target, metadata_offset)?;
    {
        let fs = fatfs::FileSystem::new(
            Cursor::new(&mut payload[..]),
            fatfs::FsOptions::new().update_accessed_date(false),
        )
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
        let root = fs.root_dir();
        let existing = root
            .iter()
            .find_map(|entry| match entry {
                Ok(e)
                    if e.file_name().eq_ignore_ascii_case(name)
                        || e.short_file_name().eq_ignore_ascii_case(name) =>
                {
                    Some(Ok(e.is_file()))
                }
                Err(e) => Some(Err(e)),
                _ => None,
            })
            .transpose()
            .map_err(|source| GotekError::Io {
                offset: start,
                source,
            })?;
        if let Some(is_file) = existing
            && (!is_file || !overwrite)
        {
            return Err(GotekError::InvalidFormatOptions(
                "destination already exists",
            ));
        }
        let stats = fs.stats().map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
        let free_bytes = u64::from(stats.free_clusters()) * u64::from(stats.cluster_size());
        if existing.is_none() && bytes.len() as u64 > free_bytes {
            return Err(GotekError::InvalidFormatOptions(
                "insufficient FAT12 free space",
            ));
        }
        let mut target = root.create_file(name).map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
        if existing.is_some() {
            target.truncate().map_err(|source| GotekError::Io {
                offset: start,
                source,
            })?;
        }
        target.write_all(bytes).map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
        target.flush().map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
        drop(target);
        drop(root);
        fs.unmount().map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    }
    timestamp_sector[..31].copy_from_slice(record.as_bytes());
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
        .write_all(&timestamp_sector)
        .map_err(|source| GotekError::Io {
            offset: metadata_offset,
            source,
        })?;
    target.flush().map_err(|source| GotekError::Io {
        offset: metadata_offset,
        source,
    })
}

fn validate_name_and_time(name: &str, update_time: UpdateTime) -> Result<String, GotekError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', ':'])
        || name.chars().any(|c| c.is_control())
    {
        return Err(GotekError::InvalidFormatOptions(
            "expected one FAT filename without path separators",
        ));
    }
    let record = format!(
        "UPDATE TIME:{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
        update_time.year,
        update_time.month,
        update_time.day,
        update_time.hour,
        update_time.minute,
        update_time.second,
    );
    if record.len() != 31 {
        return Err(GotekError::InvalidFormatOptions("invalid update timestamp"));
    }
    let mut terminated = [0u8; 32];
    terminated[..31].copy_from_slice(record.as_bytes());
    if parse_update(&terminated).is_none() {
        return Err(GotekError::InvalidFormatOptions("invalid update timestamp"));
    }
    Ok(record)
}
