use crate::GotekError;

/// Size of the boot and metadata sectors in the supported profiles.
pub const SECTOR_BYTES: usize = 512;
/// Each slot occupies 1.5 MiB, independently of its FAT12 payload size.
pub const SLOT_STRIDE_BYTES: u64 = 0x180000;
/// Metadata lives in the final sector of each physical slot.
pub const METADATA_OFFSET_BYTES: u64 = 0x17fe00;
/// Maximum supported number of slots for this inspector.
pub const MAX_SLOTS: u16 = 100;

/// Calculate a stride boundary, including the boundary after the last slot.
///
/// This arithmetic helper does not enforce the inspector's 1–100 count limit.
pub fn slot_offset(slot_index: u64) -> Result<u64, GotekError> {
    slot_index
        .checked_mul(SLOT_STRIDE_BYTES)
        .ok_or(GotekError::ArithmeticOverflow)
}

pub(crate) fn checked_offset(base: u64, relative: u64) -> Result<u64, GotekError> {
    base.checked_add(relative)
        .ok_or(GotekError::ArithmeticOverflow)
}
