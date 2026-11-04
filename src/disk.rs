use crate::{
    GotekError,
    fat12::Bpb,
    geometry::FloppyGeometry,
    inspect::read_sector,
    layout::{METADATA_OFFSET_BYTES, SECTOR_BYTES, checked_offset, slot_offset},
    metadata::parse_count,
};
use std::io::{Read, Seek};

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
/// let target = std::fs::File::open("disk.img")?;
/// let capacity = target.metadata()?.len();
/// let mut disk = gotek_core::GotekDisk::open(target, capacity)?;
/// let slot = disk.slot(1)?;
/// println!("slot {}: {}", slot.index(), slot.geometry());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct GotekDisk<R> {
    pub(crate) target: R,
    pub(crate) slots: Vec<SlotLayout>,
    pub(crate) boundary_offset: u64,
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
                geometry: slot.geometry,
            })
            .collect();
        Ok(Self {
            target,
            slots,
            boundary_offset: info.boundary_offset,
        })
    }
}

impl<R> GotekDisk<R> {
    /// Return the owned descriptor after slot access or label editing.
    pub fn into_inner(self) -> R {
        self.target
    }
}
