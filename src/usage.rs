use crate::{GotekError, fat12::fat12_entry, layout::SECTOR_BYTES};
use std::io::{Read, Seek, SeekFrom};

/// Counts of data-cluster allocation states in one slot's first FAT.
/// FAT entries 0 and 1, boot/FAT/root sectors, and Gotek metadata are excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotUsage {
    pub total_clusters: u32,
    pub used_clusters: u32,
    pub free_clusters: u32,
    pub bad_clusters: u32,
    /// Reserved markers, including the invalid `0x001` marker.
    pub reserved_clusters: u32,
}

/// Count FAT12 allocation states for a zero-based Gotek slot, without writing.
///
/// Uses the first FAT copy. If another FAT copy differs, returns an error because
/// this API cannot return inspection warnings. Counts reflect allocation, not the
/// sum of file sizes. The reader's final position is unspecified.
///
/// ```no_run
/// let mut image = std::fs::File::open("image.img")?;
/// let usage = gotek_core::slot_usage(&mut image, 0)?;
/// println!("{} of {} clusters used", usage.used_clusters, usage.total_clusters);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn slot_usage<R: Read + Seek>(reader: &mut R, slot: u16) -> Result<SlotUsage, GotekError> {
    let info = crate::inspect(reader)?;
    let selected = info
        .slots
        .get(usize::from(slot))
        .ok_or(GotekError::InvalidSlot {
            index: slot,
            count: info.floppy_count,
        })?;
    let bpb = &selected.bpb;
    let start = selected.offset;
    let fat_len = usize::from(bpb.sectors_per_fat) * SECTOR_BYTES;
    let fat_start = start
        .checked_add(u64::from(bpb.reserved_sectors) * SECTOR_BYTES as u64)
        .ok_or(GotekError::ArithmeticOverflow)?;
    let fat_end = fat_start
        .checked_add(fat_len as u64)
        .ok_or(GotekError::ArithmeticOverflow)?;
    let payload_end = start
        .checked_add(bpb.payload_bytes()?)
        .ok_or(GotekError::ArithmeticOverflow)?;
    if fat_end > payload_end {
        return Err(GotekError::InvalidBpb {
            offset: start,
            reason: "FAT exceeds payload",
        });
    }
    let mut fat = vec![0; fat_len];
    reader
        .seek(SeekFrom::Start(fat_start))
        .map_err(|source| GotekError::Io {
            offset: fat_start,
            source,
        })?;
    reader
        .read_exact(&mut fat)
        .map_err(|source| GotekError::Io {
            offset: fat_start,
            source,
        })?;
    for copy in 1..bpb.fat_count {
        let copy_start = fat_start
            .checked_add(u64::from(copy) * fat_len as u64)
            .ok_or(GotekError::ArithmeticOverflow)?;
        let copy_end = copy_start
            .checked_add(fat_len as u64)
            .ok_or(GotekError::ArithmeticOverflow)?;
        if copy_end > payload_end {
            return Err(GotekError::InvalidBpb {
                offset: start,
                reason: "FAT copy exceeds payload",
            });
        }
        let mut other = vec![0; fat_len];
        reader
            .seek(SeekFrom::Start(copy_start))
            .map_err(|source| GotekError::Io {
                offset: copy_start,
                source,
            })?;
        reader
            .read_exact(&mut other)
            .map_err(|source| GotekError::Io {
                offset: copy_start,
                source,
            })?;
        if other != fat {
            return Err(GotekError::InvalidBpb {
                offset: start,
                reason: "FAT copies differ",
            });
        }
    }
    let total_clusters =
        u32::try_from(bpb.cluster_count).map_err(|_| GotekError::ArithmeticOverflow)?;
    let mut usage = SlotUsage {
        total_clusters,
        used_clusters: 0,
        free_clusters: 0,
        bad_clusters: 0,
        reserved_clusters: 0,
    };
    for cluster in
        2..usize::try_from(bpb.cluster_count + 2).map_err(|_| GotekError::ArithmeticOverflow)?
    {
        let entry = fat12_entry(&fat, cluster).ok_or(GotekError::InvalidBpb {
            offset: start,
            reason: "FAT entry outside table",
        })?;
        match entry {
            0 => usage.free_clusters += 1,
            1 | 0xff0..=0xff6 => usage.reserved_clusters += 1,
            0xff7 => usage.bad_clusters += 1,
            _ => usage.used_clusters += 1,
        }
    }
    Ok(usage)
}
