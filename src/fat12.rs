//! Generic FAT12 boot-sector parsing, independent of Gotek slot addressing.
use crate::{GotekError, layout::SECTOR_BYTES};

/// Structural FAT12 fields and derived allocation information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bpb {
    /// Bytes per logical sector; this reader supports 512.
    pub bytes_per_sector: u16,
    /// Power-of-two sectors per allocation cluster.
    pub sectors_per_cluster: u8,
    /// Sectors preceding the first FAT.
    pub reserved_sectors: u16,
    /// Number of FAT copies.
    pub fat_count: u8,
    /// Maximum entries in the fixed root directory.
    pub root_entries: u16,
    /// Total sectors in the volume, excluding the Gotek gap.
    pub total_sectors: u32,
    /// Floppy media descriptor.
    pub media_descriptor: u8,
    /// Sectors in each FAT copy.
    pub sectors_per_fat: u16,
    /// BIOS track geometry (not used to address slots).
    pub sectors_per_track: u16,
    /// BIOS head count.
    pub heads: u16,
    /// Sectors before the volume according to the BPB.
    pub hidden_sectors: u32,
    /// First data sector, relative to the volume.
    pub first_data_sector: u64,
    /// Complete clusters only; the 1.2 MB profile has a trailing partial cluster.
    pub cluster_count: u64,
}

impl Bpb {
    /// Parse a complete boot sector and validate FAT12 allocation bounds.
    ///
    /// `offset` is used only to provide location context in errors. Jump bytes,
    /// OEM text, and extended fields are conventions, not structural FAT checks.
    pub fn parse(sector: &[u8; SECTOR_BYTES], offset: u64) -> Result<Self, GotekError> {
        let invalid = |reason| GotekError::InvalidBpb { offset, reason };
        if sector[510..512] != [0x55, 0xaa] {
            return Err(invalid("missing 55 AA boot signature"));
        }
        let bytes_per_sector = word(sector, 11);
        let sectors_per_cluster = sector[13];
        let reserved_sectors = word(sector, 14);
        let fat_count = sector[16];
        let root_entries = word(sector, 17);
        let sectors_per_fat = word(sector, 22);
        if bytes_per_sector != 512 {
            return Err(invalid("only 512-byte sectors are supported"));
        }
        if !sectors_per_cluster.is_power_of_two() || sectors_per_cluster > 128 {
            return Err(invalid("invalid sectors per cluster"));
        }
        if reserved_sectors == 0 || fat_count == 0 || root_entries == 0 || sectors_per_fat == 0 {
            return Err(invalid(
                "reserved area, FATs, and root directory must be nonempty",
            ));
        }
        let small_total = u32::from(word(sector, 19));
        let large_total = dword(sector, 32);
        if small_total != 0 && large_total != 0 {
            return Err(invalid("both total-sector fields are populated"));
        }
        let total_sectors = if small_total != 0 {
            small_total
        } else {
            large_total
        };
        // Widen bounded on-media values before arithmetic, then check derived sizes.
        let root_bytes = u64::from(root_entries)
            .checked_mul(32)
            .ok_or(GotekError::ArithmeticOverflow)?;
        let root_sectors = root_bytes.div_ceil(u64::from(bytes_per_sector));
        let fat_sectors = u64::from(fat_count)
            .checked_mul(u64::from(sectors_per_fat))
            .ok_or(GotekError::ArithmeticOverflow)?;
        let first_data_sector = u64::from(reserved_sectors)
            .checked_add(fat_sectors)
            .and_then(|value| value.checked_add(root_sectors))
            .ok_or(GotekError::ArithmeticOverflow)?;
        let data_sectors = u64::from(total_sectors)
            .checked_sub(first_data_sector)
            .ok_or_else(|| invalid("filesystem regions exceed the payload"))?;
        let cluster_count = data_sectors / u64::from(sectors_per_cluster);
        if cluster_count == 0 || cluster_count >= 4085 {
            return Err(invalid("cluster count is not FAT12"));
        }
        let fat_bytes = u64::from(sectors_per_fat)
            .checked_mul(u64::from(bytes_per_sector))
            .ok_or(GotekError::ArithmeticOverflow)?;
        let required_fat_bytes = cluster_count
            .checked_add(2)
            .and_then(|count| count.checked_mul(3))
            .ok_or(GotekError::ArithmeticOverflow)?
            .div_ceil(2);
        if fat_bytes < required_fat_bytes {
            return Err(invalid("FAT is too small for the data clusters"));
        }
        Ok(Self {
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            fat_count,
            root_entries,
            total_sectors,
            media_descriptor: sector[21],
            sectors_per_fat,
            sectors_per_track: word(sector, 24),
            heads: word(sector, 26),
            hidden_sectors: dword(sector, 28),
            first_data_sector,
            cluster_count,
        })
    }

    /// Total FAT12 payload bytes, with checked multiplication.
    pub fn payload_bytes(&self) -> Result<u64, GotekError> {
        u64::from(self.total_sectors)
            .checked_mul(u64::from(self.bytes_per_sector))
            .ok_or(GotekError::ArithmeticOverflow)
    }
}

fn word(sector: &[u8; SECTOR_BYTES], offset: usize) -> u16 {
    u16::from_le_bytes([sector[offset], sector[offset + 1]])
}
fn dword(sector: &[u8; SECTOR_BYTES], offset: usize) -> u32 {
    u32::from_le_bytes([
        sector[offset],
        sector[offset + 1],
        sector[offset + 2],
        sector[offset + 3],
    ])
}
