use crate::{
    GotekError,
    layout::{SECTOR_BYTES, checked_offset},
};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Change a slot's FAT12 volume label in an existing regular Gotek image file.
///
/// `slot` is zero-based. `label` must contain 1–11 ASCII FAT volume-label
/// characters; lowercase letters are converted to uppercase. The boot-sector
/// label and root-directory volume-label entry are updated together. For the
/// final slot, the extra boundary boot-sector copy is updated as well. Other
/// slot data and Gotek metadata are left unchanged.
///
/// The image must already contain a root volume-label entry. Validation is
/// completed before writing, but an I/O failure during writing can leave the
/// label copies inconsistent; keep a backup before editing an image.
///
/// ```no_run
/// gotek_core::set_slot_label("disk.img", 5, "DATA DISK")?;
/// # Ok::<(), gotek_core::GotekError>(())
/// ```
pub fn set_slot_label(image: impl AsRef<Path>, slot: u16, label: &str) -> Result<(), GotekError> {
    let name = encode_label(label)?;
    let image = image.as_ref();
    let metadata = fs::symlink_metadata(image).map_err(|source| GotekError::DestinationIo {
        path: image.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(GotekError::NotRegularFile);
    }
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
        .map_err(|source| GotekError::Io { offset: 0, source })?
        .is_file()
    {
        return Err(GotekError::NotRegularFile);
    }

    let info = crate::inspect(&mut file)?;
    if slot >= info.floppy_count {
        return Err(GotekError::InvalidSlot {
            index: slot,
            count: info.floppy_count,
        });
    }
    let selected = &info.slots[usize::from(slot)];
    let bpb = &selected.bpb;
    let fat_sectors = u64::from(bpb.fat_count) * u64::from(bpb.sectors_per_fat);
    let root_sector = u64::from(bpb.reserved_sectors) + fat_sectors;
    let root_offset = checked_offset(selected.offset, root_sector * SECTOR_BYTES as u64)?;
    let root_bytes = u64::from(bpb.root_entries) * 32;
    let root_end = checked_offset(root_offset, root_bytes)?;
    let payload_end = checked_offset(selected.offset, selected.geometry.payload_bytes())?;
    if root_end > payload_end {
        return Err(GotekError::InvalidBpb {
            offset: selected.offset,
            reason: "root directory exceeds floppy payload",
        });
    }
    let mut root = vec![0; root_bytes as usize];
    file.seek(SeekFrom::Start(root_offset))
        .map_err(|source| GotekError::Io {
            offset: root_offset,
            source,
        })?;
    file.read_exact(&mut root)
        .map_err(|source| GotekError::Io {
            offset: root_offset,
            source,
        })?;
    let mut label_entry = None;
    for (index, entry) in root.chunks_exact(32).enumerate() {
        if entry[0] == 0 {
            break;
        }
        if entry[0] != 0xe5 && entry[11] == 0x08 && label_entry.replace(index).is_some() {
            return Err(GotekError::InvalidFormatOptions(
                "multiple root volume-label entries",
            ));
        }
    }
    let label_entry = label_entry.ok_or(GotekError::InvalidFormatOptions(
        "root volume-label entry is missing",
    ))?;
    let entry_offset = checked_offset(root_offset, (label_entry * 32) as u64)?;
    let boot_offset = checked_offset(selected.offset, 43)?;
    let boundary_offset = checked_offset(info.boundary_offset, 43)?;

    write_label(&mut file, entry_offset, &name)?;
    write_label(&mut file, boot_offset, &name)?;
    if slot + 1 == info.floppy_count {
        write_label(&mut file, boundary_offset, &name)?;
    }
    file.flush().map_err(|source| GotekError::Io {
        offset: boot_offset,
        source,
    })
}

fn write_label(
    file: &mut (impl Write + Seek),
    offset: u64,
    name: &[u8; 11],
) -> Result<(), GotekError> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|source| GotekError::Io { offset, source })?;
    file.write_all(name)
        .map_err(|source| GotekError::Io { offset, source })
}

fn encode_label(label: &str) -> Result<[u8; 11], GotekError> {
    if label.is_empty() || label.len() > 11 || label.trim() != label {
        return Err(GotekError::InvalidFormatOptions(
            "volume label must be 1–11 characters without surrounding spaces",
        ));
    }
    let mut encoded = [b' '; 11];
    for (index, byte) in label.bytes().enumerate() {
        let upper = byte.to_ascii_uppercase();
        if !(upper.is_ascii_alphanumeric() || b" !#$%&'()-@^_`{}~".contains(&upper)) {
            return Err(GotekError::InvalidFormatOptions(
                "volume label contains a character unsupported by FAT",
            ));
        }
        encoded[index] = upper;
    }
    Ok(encoded)
}
