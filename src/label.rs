use crate::{
    GotekError,
    fat12::Bpb,
    inspect::read_sector,
    layout::{METADATA_OFFSET_BYTES, SECTOR_BYTES, checked_offset, slot_offset},
    metadata::parse_count,
};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

/// An already-open Gotek image or device with a validated slot layout.
///
/// Owns `target`; byte zero must be the start of slot zero. `capacity` is the
/// accessible length in bytes, including the extra boundary sector. Opening
/// checks the complete container once. Only structural layout is cached, not
/// labels or warnings, so label edits do not leave cached label data stale.
/// The caller must prevent external changes to the target while this value is
/// in use, and must supply an accurate capacity for device descriptors.
///
/// ```no_run
/// # use std::fs::OpenOptions;
/// let target = OpenOptions::new().read(true).write(true).open("disk.img")?;
/// let capacity = target.metadata()?.len();
/// let mut disk = gotek_core::GotekDisk::open(target, capacity)?;
/// disk.slot_mut(1)?.set_label("MY DISK")?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct GotekDisk<R> {
    target: R,
    slots: Vec<SlotLayout>,
    boundary_offset: u64,
}

struct SlotLayout {
    offset: u64,
    bpb: Bpb,
    payload_bytes: u64,
}

/// A mutable handle to one slot, borrowing its [`GotekDisk`] exclusively.
pub struct SlotMut<'a, R> {
    disk: &'a mut GotekDisk<R>,
    index: u16,
}

impl<R: Read + Write + Seek> GotekDisk<R> {
    /// Validate and take ownership of an already-open image or device.
    ///
    /// The initial cursor position is ignored. Invalid count records, BPBs,
    /// unsupported geometries, truncated containers, and insufficient declared
    /// capacity are rejected before a slot handle can be obtained.
    pub fn open(mut target: R, capacity: u64) -> Result<Self, GotekError> {
        let minimum = METADATA_OFFSET_BYTES + SECTOR_BYTES as u64;
        if capacity < minimum {
            return Err(GotekError::ImageTooSmall {
                actual_bytes: capacity,
                required_bytes: minimum,
            });
        }
        let count = parse_count(&read_sector(&mut target, METADATA_OFFSET_BYTES)?)?;
        let boundary_offset = slot_offset(u64::from(count))?;
        let required = checked_offset(boundary_offset, SECTOR_BYTES as u64)?;
        if capacity < required {
            return Err(GotekError::ImageTooSmall {
                actual_bytes: capacity,
                required_bytes: required,
            });
        }

        let info = crate::inspect(&mut target)?;
        let slots = info
            .slots
            .into_iter()
            .map(|slot| SlotLayout {
                offset: slot.offset,
                bpb: slot.bpb,
                payload_bytes: slot.geometry.payload_bytes(),
            })
            .collect();
        Ok(Self {
            target,
            slots,
            boundary_offset: info.boundary_offset,
        })
    }

    /// Borrow a zero-based slot for label editing.
    pub fn slot_mut(&mut self, index: u16) -> Result<SlotMut<'_, R>, GotekError> {
        if usize::from(index) >= self.slots.len() {
            return Err(GotekError::InvalidSlot {
                index,
                count: self.slots.len() as u16,
            });
        }
        Ok(SlotMut { disk: self, index })
    }

    /// Return the owned descriptor after label editing.
    pub fn into_inner(self) -> R {
        self.target
    }
}

impl<R: Read + Write + Seek> SlotMut<'_, R> {
    /// Change the boot-sector and root-directory volume-label bytes.
    ///
    /// The final slot's boundary copy is updated too. The root entry is checked
    /// before writing. A write failure may leave the copies inconsistent;
    /// physical-device writes are not power-failure atomic.
    pub fn set_label(&mut self, label: &str) -> Result<(), GotekError> {
        let name = encode_label(label)?;
        self.set_encoded_label(&name)
    }

    fn set_encoded_label(&mut self, name: &[u8; 11]) -> Result<(), GotekError> {
        let index = usize::from(self.index);
        let layout = &self.disk.slots[index];
        let is_last = index + 1 == self.disk.slots.len();
        write_slot_label(
            &mut self.disk.target,
            layout,
            self.disk.boundary_offset,
            is_last,
            name,
        )
    }
}

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
    let opened = file
        .metadata()
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    if !opened.is_file() {
        return Err(GotekError::NotRegularFile);
    }
    let mut disk = GotekDisk::open(&mut file, opened.len())?;
    disk.slot_mut(slot)?.set_encoded_label(&name)
}

/// Change a slot's FAT12 volume label through an already-open image or device.
///
/// `capacity` is the target's accessible byte length, including the extra
/// boundary sector. Byte zero must be slot zero; the initial cursor position is
/// ignored. The caller must ensure the descriptor is authorized for writing,
/// exclusively accessed, and supplied with an accurate capacity. The same
/// label validation and changes as [`set_slot_label`] apply.
///
/// All feasible validation happens before the first write. Only the selected
/// slot's boot-sector and root-directory label bytes are written, plus the
/// boundary copy when selecting the final slot. Physical-device writes are not
/// power-failure atomic; I/O failure may leave label copies inconsistent.
///
/// ```
/// # use std::io::{Read, Write, Seek};
/// fn rename<T: Read + Write + Seek>(
///     device: &mut T,
///     capacity: u64,
/// ) -> Result<(), gotek_core::GotekError> {
///     gotek_core::set_slot_label_on(device, capacity, 5, "DATA DISK")
/// }
/// ```
pub fn set_slot_label_on<T: Read + Write + Seek + ?Sized>(
    target: &mut T,
    capacity: u64,
    slot: u16,
    label: &str,
) -> Result<(), GotekError> {
    let name = encode_label(label)?;
    let mut disk = GotekDisk::open(target, capacity)?;
    disk.slot_mut(slot)?.set_encoded_label(&name)
}

fn write_slot_label<T: Read + Write + Seek + ?Sized>(
    target: &mut T,
    selected: &SlotLayout,
    boundary_offset: u64,
    is_last: bool,
    name: &[u8; 11],
) -> Result<(), GotekError> {
    let bpb = &selected.bpb;
    let fat_sectors = u64::from(bpb.fat_count) * u64::from(bpb.sectors_per_fat);
    let root_sector = u64::from(bpb.reserved_sectors) + fat_sectors;
    let root_offset = checked_offset(selected.offset, root_sector * SECTOR_BYTES as u64)?;
    let root_bytes = u64::from(bpb.root_entries) * 32;
    let root_end = checked_offset(root_offset, root_bytes)?;
    let payload_end = checked_offset(selected.offset, selected.payload_bytes)?;
    if root_end > payload_end {
        return Err(GotekError::InvalidBpb {
            offset: selected.offset,
            reason: "root directory exceeds floppy payload",
        });
    }
    let mut root = vec![0; root_bytes as usize];
    target
        .seek(SeekFrom::Start(root_offset))
        .map_err(|source| GotekError::Io {
            offset: root_offset,
            source,
        })?;
    target
        .read_exact(&mut root)
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
    let boundary_offset = checked_offset(boundary_offset, 43)?;

    write_label(target, entry_offset, name)?;
    write_label(target, boot_offset, name)?;
    if is_last {
        write_label(target, boundary_offset, name)?;
    }
    target.flush().map_err(|source| GotekError::Io {
        offset: boot_offset,
        source,
    })
}

fn write_label<T: Write + Seek + ?Sized>(
    file: &mut T,
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
