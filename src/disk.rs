use crate::{
    GotekError,
    fat12::Bpb,
    geometry::FloppyGeometry,
    inspect::{detect_geometry, read_sector},
    layout::{METADATA_OFFSET_BYTES, SECTOR_BYTES, SLOT_STRIDE_BYTES, checked_offset, slot_offset},
    metadata::parse_count,
};
use std::io::{Read, Seek};

/// An already-open Gotek image or device with a validated slot layout.
///
/// Owns `target`; byte zero must be the start of slot zero. `capacity` is the
/// accessible length in bytes. Opening validates slot zero and the declared
/// slot region; other slots are checked when accessed. The extra boundary
/// sector is required only for mutable access. Only structural layout is
/// cached, not labels or warnings.
/// The caller must prevent external changes to the target while this value is
/// in use, and must supply an accurate capacity for device descriptors.
///
/// ```no_run
/// use std::io::{Seek, SeekFrom};
/// let mut target = std::fs::File::open("disk.img")?;
/// let capacity = target.seek(SeekFrom::End(0))?;
/// let mut disk = gotek_core::GotekDisk::open(target, capacity)?;
/// let slot = disk.slot(1)?;
/// println!("slot {}: {}", slot.index(), slot.geometry());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct GotekDisk<R> {
    pub(crate) target: R,
    pub(crate) capacity: u64,
    pub(crate) slots: Vec<Option<SlotLayout>>,
    pub(crate) boundary_offset: u64,
    pub(crate) boundary_validated: bool,
    pub(crate) poisoned: bool,
}

/// Cached structural data, unaffected by volume-label changes.
pub(crate) struct SlotLayout {
    pub(crate) offset: u64,
    pub(crate) bpb: Bpb,
    pub(crate) geometry: FloppyGeometry,
}

impl<R: Read + Seek> GotekDisk<R> {
    /// Validate and take ownership of an already-open image or device.
    ///
    /// The initial cursor position is ignored. Slot zero's BPB and geometry,
    /// the slot count record, and the declared slot region are checked. Other
    /// slots and the extra boundary sector are not read here. Use [`crate::inspect`]
    /// for explicit full-container validation.
    pub fn open(mut target: R, capacity: u64) -> Result<Self, GotekError> {
        let minimum = SLOT_STRIDE_BYTES;
        if capacity < minimum {
            return Err(GotekError::ImageTooSmall {
                actual_bytes: capacity,
                required_bytes: minimum,
            });
        }
        let boot = read_sector(&mut target, 0)?;
        let bpb = Bpb::parse(&boot, 0)?;
        let geometry = detect_geometry(&bpb, 0)?;
        let count = parse_count(&read_sector(&mut target, METADATA_OFFSET_BYTES)?)?;
        let boundary_offset = slot_offset(u64::from(count))?;
        let required = boundary_offset;
        if capacity < required {
            return Err(GotekError::ImageTooSmall {
                actual_bytes: capacity,
                required_bytes: required,
            });
        }

        let mut slots = std::iter::repeat_with(|| None)
            .take(usize::from(count))
            .collect::<Vec<_>>();
        slots[0] = Some(SlotLayout {
            offset: 0,
            bpb,
            geometry,
        });
        Ok(Self {
            target,
            capacity,
            slots,
            boundary_offset,
            boundary_validated: false,
            poisoned: false,
        })
    }

    pub(crate) fn ensure_healthy(&self) -> Result<(), GotekError> {
        if self.poisoned {
            Err(GotekError::DiskNeedsReopen)
        } else {
            Ok(())
        }
    }

    pub(crate) fn validate_slot(&mut self, index: u16) -> Result<(), GotekError> {
        self.ensure_healthy()?;
        let count = self.slots.len() as u16;
        let entry = self
            .slots
            .get_mut(usize::from(index))
            .ok_or(GotekError::InvalidSlot { index, count })?;
        if entry.is_none() {
            let offset = slot_offset(u64::from(index))?;
            let bpb = Bpb::parse(&read_sector(&mut self.target, offset)?, offset)?;
            let geometry = detect_geometry(&bpb, offset)?;
            *entry = Some(SlotLayout {
                offset,
                bpb,
                geometry,
            });
        }
        Ok(())
    }

    pub(crate) fn validate_boundary(&mut self) -> Result<(), GotekError> {
        self.ensure_healthy()?;
        if self.boundary_validated {
            return Ok(());
        }
        let required = checked_offset(self.boundary_offset, SECTOR_BYTES as u64)?;
        if self.capacity < required {
            return Err(GotekError::ImageTooSmall {
                actual_bytes: self.capacity,
                required_bytes: required,
            });
        }
        let sector = read_sector(&mut self.target, self.boundary_offset)?;
        let bpb = Bpb::parse(&sector, self.boundary_offset)?;
        detect_geometry(&bpb, self.boundary_offset)?;
        self.boundary_validated = true;
        Ok(())
    }
}

impl<R> GotekDisk<R> {
    /// Return the owned descriptor after slot access or label editing.
    pub fn into_inner(self) -> R {
        self.target
    }
}
