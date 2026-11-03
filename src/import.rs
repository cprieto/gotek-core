use crate::{
    GotekError,
    fat12::Bpb,
    inspect::{detect_geometry, read_sector},
    layout::{METADATA_OFFSET_BYTES, slot_offset},
    metadata::{UpdateTime, parse_count, parse_update},
};
use std::{
    fs::{self, OpenOptions},
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Import one local file into a slot's root directory.
///
/// `image` and `source` accept host filesystem paths. `name` is a UTF-8 FAT
/// filename and defaults to the source basename. Only a single FAT filename is accepted;
/// directories must not be specified. `overwrite` truncates an existing regular
/// file before writing; directory entries with that name are never replaced.
/// `update_time` is a local wall-clock timestamp without timezone information.
/// The image must be a regular file path, not a symlink or device. The source is
/// read completely before any image writes. A later I/O error may leave partial
/// changes; back up valuable images before importing.
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
    // Only the default FAT name needs a UTF-8 conversion. Explicit names allow
    // arbitrary host paths, including non-UTF-8 source basenames.
    let name = match name {
        Some(name) => name,
        None => source.file_name().and_then(|value| value.to_str()).ok_or(
            GotekError::InvalidFormatOptions(
                "source has no UTF-8 basename; specify a FAT filename",
            ),
        )?,
    };
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
        update_time.second
    );
    let mut terminated = [0u8; 32];
    if record.len() != 31 {
        return Err(GotekError::InvalidFormatOptions("invalid update timestamp"));
    }
    terminated[..31].copy_from_slice(record.as_bytes());
    if parse_update(&terminated).is_none() {
        return Err(GotekError::InvalidFormatOptions("invalid update timestamp"));
    }
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
    if !file
        .metadata()
        .map_err(|source| GotekError::DestinationIo {
            path: image.to_path_buf(),
            source,
        })?
        .is_file()
    {
        return Err(GotekError::NotRegularFile);
    }
    let count = parse_count(&read_sector(&mut file, METADATA_OFFSET_BYTES)?)?;
    if slot >= count {
        return Err(GotekError::InvalidSlot { index: slot, count });
    }
    let start = slot_offset(u64::from(slot))?;
    let bpb = Bpb::parse(&read_sector(&mut file, start)?, start)?;
    detect_geometry(&bpb, start)?;
    let limit = bpb.payload_bytes()?;
    if bytes.len() as u64 > limit {
        return Err(GotekError::InvalidFormatOptions(
            "source exceeds FAT12 payload",
        ));
    }
    let end = start + limit;
    let metadata_offset = start + METADATA_OFFSET_BYTES;
    let length = file
        .metadata()
        .map_err(|source| GotekError::Io { offset: 0, source })?
        .len();
    if length < metadata_offset + 512 {
        return Err(GotekError::Io {
            offset: length,
            source: io::Error::from(io::ErrorKind::UnexpectedEof),
        });
    }
    let mut payload = vec![0; limit as usize];
    file.seek(SeekFrom::Start(start))
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    file.read_exact(&mut payload)
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    let mut timestamp_sector = read_sector(&mut file, metadata_offset)?;
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
        target.write_all(&bytes).map_err(|source| GotekError::Io {
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
    file.seek(SeekFrom::Start(start))
        .map_err(|source| GotekError::Io {
            offset: start,
            source,
        })?;
    file.write_all(&payload).map_err(|source| GotekError::Io {
        offset: start,
        source,
    })?;
    debug_assert_eq!(end, start + payload.len() as u64);
    file.seek(SeekFrom::Start(metadata_offset))
        .map_err(|source| GotekError::Io {
            offset: metadata_offset,
            source,
        })?;
    file.write_all(&timestamp_sector)
        .map_err(|source| GotekError::Io {
            offset: metadata_offset,
            source,
        })?;
    file.flush().map_err(|source| GotekError::Io {
        offset: metadata_offset,
        source,
    })
}
