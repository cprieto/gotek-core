//! Deterministic empty-image generation for the observed ipcas profiles.
use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::Path,
};

use crate::{
    GotekError,
    fat12::Bpb,
    geometry::FloppyGeometry,
    layout::{MAX_SLOTS, METADATA_OFFSET_BYTES, SECTOR_BYTES, checked_offset, slot_offset},
    metadata::{UpdateTime, encode_update_time},
};

const BOOT_CODE_OFFSET: usize = 62;
const BOOT_SIGNATURE_OFFSET: usize = 510;
// Original real-mode diagnostic instructions, assembled from boot/non_system.S.
// The message pointer is 0x7c00 + 62 + 33 = 0x7c5f. See boot/README.md for
// instruction-by-instruction documentation and reproduction commands.
const NON_SYSTEM_BOOT_CODE: [u8; 33] = [
    0xfa, // cli
    0x31, 0xc0, // xor ax, ax
    0x8e, 0xd0, // mov ss, ax
    0xbc, 0x00, 0x7c, // mov sp, 0x7c00
    0x8e, 0xd8, // mov ds, ax
    0xfc, // cld
    0xfb, // sti
    0xbe, 0x5f, 0x7c, // mov si, 0x7c5f (message)
    0xbb, 0x07, 0x00, // mov bx, 0x0007 (page/attribute)
    0xac, // lodsb
    0x84, 0xc0, // test al, al
    0x74, 0x06, // jz stop
    0xb4, 0x0e, // mov ah, 0x0e
    0xcd, 0x10, // int 0x10 (BIOS teletype)
    0xeb, 0xf5, // jmp print_character
    0xfa, // stop: cli
    0xf4, // wait_forever: hlt
    0xeb, 0xfd, // jmp wait_forever
];
const NON_SYSTEM_MESSAGE: &[u8] = b"Non-system disk\0";
const ROOT_ENTRY_BYTES: usize = 32;
const FILL_BUFFER_BYTES: usize = 16 * 1024;

/// Explicit variable inputs for one slot. No clock, timezone, or serial algorithm
/// is assumed. Repeated serial numbers are allowed, as observed in ipcas captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotFormatOptions {
    /// Arbitrary 32-bit serial stored little-endian in the extended BPB.
    pub volume_serial: u32,
    /// Metadata timestamp, including seconds (calendar years 1–9999).
    pub update_time: UpdateTime,
    /// Date and time for the root volume-label entry (years 1980–2107).
    /// Seconds are rounded down to FAT's two-second resolution. The observed
    /// ipcas displaced creation-time bytes are preserved; see `boot/README.md`.
    pub root_label_time: UpdateTime,
}

/// Configuration for a uniform-geometry image with deterministic per-slot inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions {
    /// FAT12 payload geometry for every slot; the outer stride stays fixed.
    pub geometry: FloppyGeometry,
    /// One entry per slot, in order. The length is the slot count (1–100).
    pub slots: Vec<SlotFormatOptions>,
}

impl FormatOptions {
    /// Repeat the same variable inputs across `slot_count` slots.
    ///
    /// Labels are still numbered independently. The returned options can be edited
    /// to use distinct serials or timestamps. Invalid inputs are rejected here and
    /// revalidated before formatting.
    pub fn new(
        geometry: FloppyGeometry,
        slot_count: u16,
        slot: SlotFormatOptions,
    ) -> Result<Self, GotekError> {
        if !(1..=MAX_SLOTS).contains(&slot_count) {
            return Err(GotekError::InvalidSlotCount(slot_count));
        }
        validate_slot_options(slot)?;
        Ok(Self {
            geometry,
            slots: vec![slot; usize::from(slot_count)],
        })
    }

    /// Validate all options and return the exact output size, including the extra
    /// boundary sector. No device-capacity padding is appended.
    pub fn image_size_bytes(&self) -> Result<u64, GotekError> {
        let count = u16::try_from(self.slots.len())
            .map_err(|_| GotekError::InvalidFormatOptions("too many slots"))?;
        if !(1..=MAX_SLOTS).contains(&count) {
            return Err(GotekError::InvalidSlotCount(count));
        }
        for &slot in &self.slots {
            validate_slot_options(slot)?;
        }
        checked_offset(slot_offset(u64::from(count))?, SECTOR_BYTES as u64)
    }
}

/// Generate a complete empty FAT12 boot sector, without positioning it in
/// a Gotek container. Slot indices must be 0–99; labels are `FLPPY{slot_index}`.
///
/// BPB values follow the observed profiles. The original diagnostic stub prints
/// "Non-system disk" and waits forever; it never loads an operating system or
/// filesystem data. No captured executable code or binary template is used.
/// The result is checked with `Bpb::parse` before being returned.
pub fn generate_boot_sector(
    geometry: FloppyGeometry,
    slot_index: u16,
    volume_serial: u32,
) -> Result<[u8; SECTOR_BYTES], GotekError> {
    if slot_index >= MAX_SLOTS {
        return Err(GotekError::InvalidFormatOptions("slot index must be 0–99"));
    }
    let mut boot_sector = [0u8; SECTOR_BYTES];
    // EB 3C jumps from offset 2 to offset 62, past the BPB and extended BPB.
    boot_sector[..3].copy_from_slice(&[0xeb, 0x3c, 0x90]);
    // Preserve the observed OEM identifier as format data, not executable code.
    boot_sector[3..11].copy_from_slice(b"MSDOS5.0");
    put_word(&mut boot_sector, 11, SECTOR_BYTES as u16);
    put_word(&mut boot_sector, 14, 1);
    boot_sector[16] = 2;
    put_word(&mut boot_sector, 26, 2);
    // Hidden sectors, large total-sector count, drive number, and reserved byte
    // stay zero. These profiles use the 16-bit total-sector field below.
    boot_sector[38] = 0x29;
    boot_sector[54..62].copy_from_slice(b"FAT12   ");
    boot_sector[BOOT_SIGNATURE_OFFSET..].copy_from_slice(&[0x55, 0xaa]);
    let message_offset = BOOT_CODE_OFFSET + NON_SYSTEM_BOOT_CODE.len();
    boot_sector[BOOT_CODE_OFFSET..message_offset].copy_from_slice(&NON_SYSTEM_BOOT_CODE);
    boot_sector[message_offset..message_offset + NON_SYSTEM_MESSAGE.len()]
        .copy_from_slice(NON_SYSTEM_MESSAGE);
    let (cluster_sectors, root_entries, total_sectors, media, fat_sectors, track_sectors) =
        match geometry {
            FloppyGeometry::Kb720 => (2, 112, 1440, 0xf9, 3, 9),
            FloppyGeometry::Mb1200 => (2, 224, 2400, 0xf9, 7, 15),
            FloppyGeometry::Mb1440 => (1, 224, 2880, 0xf0, 9, 18),
        };
    boot_sector[13] = cluster_sectors;
    put_word(&mut boot_sector, 17, root_entries);
    put_word(&mut boot_sector, 19, total_sectors);
    boot_sector[21] = media;
    put_word(&mut boot_sector, 22, fat_sectors);
    put_word(&mut boot_sector, 24, track_sectors);
    boot_sector[39..43].copy_from_slice(&volume_serial.to_le_bytes());
    boot_sector[43..54].fill(b' ');
    let label = format!("FLPPY{slot_index}");
    boot_sector[43..43 + label.len()].copy_from_slice(label.as_bytes());
    Bpb::parse(&boot_sector, 0)?;
    Ok(boot_sector)
}

/// Create a new regular image file, refusing to replace any existing path.
///
/// This is the recommended disk-file entry point. `create_new` also refuses an
/// existing device path or symlink. All options are checked before file creation.
/// An I/O failure can leave a partial file, which the caller may inspect or remove.
/// The image is flushed, but no durable-storage synchronization is requested.
///
/// ```no_run
/// use gotek_core::{FormatOptions, SlotFormatOptions, create_image};
/// use gotek_core::{geometry::FloppyGeometry, metadata::UpdateTime};
///
/// let time = UpdateTime {
///     year: 2026, month: 10, day: 31, hour: 13, minute: 12, second: 19,
/// };
/// let slot = SlotFormatOptions {
///     volume_serial: 0x1234_5678,
///     update_time: time,
///     root_label_time: time,
/// };
/// let options = FormatOptions::new(FloppyGeometry::Mb1440, 50, slot)?;
/// create_image("new-image.img", &options)?;
/// # Ok::<(), gotek_core::GotekError>(())
/// ```
pub fn create_image(path: impl AsRef<Path>, options: &FormatOptions) -> Result<(), GotekError> {
    options.image_size_bytes()?;
    let mut image = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    format_image(&mut image, options)
}

/// Initialize an empty image with observed ipcas structures and an original diagnostic stub.
///
/// Use only a newly created regular image file, or an in-memory/test writer.
/// Prefer `create_image` for filesystem paths. This generic API cannot determine
/// whether a writer represents a device; it provides no raw-device support or
/// device-safety validation. The caller must uphold the destination precondition.
///
/// All configuration is validated before any write. Nonempty destinations are
/// rejected via `SeekFrom::End(0)`; this function does not truncate existing data.
/// Every byte in `[0, options.image_size_bytes())` is initialized,
/// including gaps, with bounded memory. Nothing is written after the boundary
/// sector. The destination is flushed and left at its end on success. An I/O
/// failure may leave a partial image; its cursor position is then unspecified.
pub fn format_image<W: Write + Seek + ?Sized>(
    writer: &mut W,
    options: &FormatOptions,
) -> Result<(), GotekError> {
    let image_size = options.image_size_bytes()?;
    // Prepare every slot before touching the destination, so errors in a later
    // slot's options can never leave an image that is only partly configured.
    let prepared_slots = prepare_slots(options)?;
    let length = writer
        .seek(SeekFrom::End(0))
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    if length != 0 {
        return Err(GotekError::OutputNotEmpty(length));
    }
    write_prepared_image(writer, image_size, &prepared_slots)
}

/// Reinitialize an existing regular image file without changing its total size.
///
/// The file must be opened for writing without append mode or truncation. All
/// options, the opened file type, and its capacity are checked before any writes.
/// Capacity includes the extra boundary sector: `slot_count * stride + 512`.
/// Bytes after that sector are preserved. This shares the initialization engine
/// with [`format_image`], which continues to require an empty destination.
///
/// Devices and other non-regular files are rejected. The caller must prevent
/// concurrent file changes during formatting. An I/O failure can leave a partial
/// image; successful writes are flushed but not synchronized to durable storage.
pub fn format_existing_image(image: &mut File, options: &FormatOptions) -> Result<(), GotekError> {
    let metadata = image
        .metadata()
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    if !metadata.is_file() {
        return Err(GotekError::NotRegularFile);
    }
    format_on(image, metadata.len(), options)
}

/// Reinitialize an already-open target using an explicit verified capacity.
///
/// This generic formatter does not identify devices, check mounts, or obtain
/// exclusive access. The caller must perform those checks before calling it.
/// It writes only the required prefix, without truncating or resizing the target.
/// Failed writes can leave a partial format.
pub fn format_on<W: Write + Seek + ?Sized>(
    writer: &mut W,
    capacity: u64,
    options: &FormatOptions,
) -> Result<(), GotekError> {
    let image_size = options.image_size_bytes()?;
    let prepared_slots = prepare_slots(options)?;
    if capacity < image_size {
        return Err(GotekError::ImageTooSmall {
            actual_bytes: capacity,
            required_bytes: image_size,
        });
    }
    write_prepared_image(writer, image_size, &prepared_slots)
}

fn write_prepared_image<W: Write + Seek + ?Sized>(
    writer: &mut W,
    image_size: u64,
    prepared_slots: &[PreparedSlot],
) -> Result<(), GotekError> {
    writer
        .seek(SeekFrom::Start(0))
        .map_err(|source| GotekError::Io { offset: 0, source })?;
    let mut position = 0;
    for slot in prepared_slots {
        write_bytes(writer, &mut position, image_size, &slot.boot_sector)?;
        for _ in 0..2 {
            write_bytes(
                writer,
                &mut position,
                image_size,
                &[slot.media_descriptor, 0xff, 0xff],
            )?;
            write_fill(writer, &mut position, image_size, 0, slot.fat_zero_bytes)?;
        }
        write_bytes(writer, &mut position, image_size, &slot.root_entry)?;
        write_fill(writer, &mut position, image_size, 0, slot.root_zero_bytes)?;
        write_fill(
            writer,
            &mut position,
            image_size,
            0xf6,
            slot.data_fill_bytes,
        )?;
        // The final payload sector is zero even when it belongs to a complete
        // cluster. Preserve that observation rather than deducing a FAT rule.
        write_fill(writer, &mut position, image_size, 0, SECTOR_BYTES as u64)?;
        write_fill(writer, &mut position, image_size, 0, slot.gap_bytes)?;
        write_bytes(writer, &mut position, image_size, &slot.metadata)?;
    }
    let last_slot = prepared_slots
        .last()
        .ok_or(GotekError::InvalidSlotCount(0))?;
    let mut boundary_sector = last_slot.boot_sector;
    boundary_sector[..3].fill(0);
    write_bytes(writer, &mut position, image_size, &boundary_sector)?;
    writer.flush().map_err(|source| GotekError::Io {
        offset: position,
        source,
    })
}

struct PreparedSlot {
    boot_sector: [u8; SECTOR_BYTES],
    metadata: [u8; SECTOR_BYTES],
    root_entry: [u8; ROOT_ENTRY_BYTES],
    media_descriptor: u8,
    fat_zero_bytes: u64,
    root_zero_bytes: u64,
    data_fill_bytes: u64,
    gap_bytes: u64,
}

fn prepare_slots(options: &FormatOptions) -> Result<Vec<PreparedSlot>, GotekError> {
    let mut prepared = Vec::with_capacity(options.slots.len());
    let mut encoded_updates: Vec<(UpdateTime, [u8; 31])> = Vec::new();
    for (index, slot) in options.slots.iter().enumerate() {
        let slot_index = u16::try_from(index).map_err(|_| GotekError::ArithmeticOverflow)?;
        let boot_sector = generate_boot_sector(options.geometry, slot_index, slot.volume_serial)?;
        let bpb = Bpb::parse(&boot_sector, slot_offset(u64::from(slot_index))?)?;
        let fat_bytes = u64::from(bpb.sectors_per_fat)
            .checked_mul(SECTOR_BYTES as u64)
            .ok_or(GotekError::ArithmeticOverflow)?;
        let root_bytes = u64::from(bpb.root_entries)
            .checked_mul(ROOT_ENTRY_BYTES as u64)
            .ok_or(GotekError::ArithmeticOverflow)?;
        let fat_zero_bytes = fat_bytes
            .checked_sub(3)
            .ok_or(GotekError::ArithmeticOverflow)?;
        let root_zero_bytes = root_bytes
            .checked_sub(ROOT_ENTRY_BYTES as u64)
            .ok_or(GotekError::ArithmeticOverflow)?;
        let data_start = bpb
            .first_data_sector
            .checked_mul(SECTOR_BYTES as u64)
            .ok_or(GotekError::ArithmeticOverflow)?;
        let payload_bytes = bpb.payload_bytes()?;
        let data_fill_bytes = payload_bytes
            .checked_sub(data_start)
            .and_then(|bytes| bytes.checked_sub(SECTOR_BYTES as u64))
            .ok_or(GotekError::ArithmeticOverflow)?;
        let gap_bytes = METADATA_OFFSET_BYTES
            .checked_sub(payload_bytes)
            .ok_or(GotekError::ArithmeticOverflow)?;
        let root_entry = generate_root_entry(&boot_sector, slot.root_label_time);
        let update_record = match encoded_updates
            .iter()
            .find(|(time, _)| *time == slot.update_time)
        {
            Some((_, record)) => *record,
            None => {
                let record = encode_update_time(slot.update_time)?;
                encoded_updates.push((slot.update_time, record));
                record
            }
        };
        let metadata =
            generate_metadata(update_record, (index == 0).then_some(options.slots.len()));
        prepared.push(PreparedSlot {
            boot_sector,
            metadata,
            root_entry,
            media_descriptor: bpb.media_descriptor,
            fat_zero_bytes,
            root_zero_bytes,
            data_fill_bytes,
            gap_bytes,
        });
    }
    Ok(prepared)
}

// Called only after validating every date and FAT year range in the options.
fn generate_root_entry(
    boot_sector: &[u8; SECTOR_BYTES],
    time: UpdateTime,
) -> [u8; ROOT_ENTRY_BYTES] {
    let mut root_entry = [0; ROOT_ENTRY_BYTES];
    root_entry[..11].copy_from_slice(&boot_sector[43..54]);
    root_entry[11] = 0x08;
    let fat_date = ((time.year - 1980) << 9) | (u16::from(time.month) << 5) | u16::from(time.day);
    let fat_time =
        (u16::from(time.hour) << 11) | (u16::from(time.minute) << 5) | u16::from(time.second / 2);
    // All clean captures repeat the time at 13, not the usual FAT creation
    // word at 14. Byte 15 remains zero; this deliberately preserves ipcas bytes.
    root_entry[13..15].copy_from_slice(&fat_time.to_le_bytes());
    root_entry[22..24].copy_from_slice(&fat_time.to_le_bytes());
    for offset in [16, 18, 24] {
        root_entry[offset..offset + 2].copy_from_slice(&fat_date.to_le_bytes());
    }
    root_entry
}

fn generate_metadata(update_record: [u8; 31], slot_count: Option<usize>) -> [u8; SECTOR_BYTES] {
    let mut metadata = [0; SECTOR_BYTES];
    metadata[..31].copy_from_slice(&update_record);
    if let Some(count) = slot_count {
        let count_record = format!("MAX BLOCK:{count:03}");
        metadata[0x50..0x5d].copy_from_slice(count_record.as_bytes());
    }
    metadata
}

fn validate_slot_options(slot: SlotFormatOptions) -> Result<(), GotekError> {
    encode_update_time(slot.update_time)?;
    encode_update_time(slot.root_label_time)?;
    if !(1980..=2107).contains(&slot.root_label_time.year) {
        return Err(GotekError::InvalidFormatOptions(
            "root-label year must fit FAT's 1980–2107 range",
        ));
    }
    Ok(())
}

fn put_word(sector: &mut [u8; SECTOR_BYTES], offset: usize, value: u16) {
    sector[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_bytes<W: Write + ?Sized>(
    writer: &mut W,
    position: &mut u64,
    limit: u64,
    bytes: &[u8],
) -> Result<(), GotekError> {
    let next_position = checked_offset(*position, bytes.len() as u64)?;
    if next_position > limit {
        return Err(GotekError::InvalidFormatOptions(
            "write exceeds calculated image size",
        ));
    }
    writer.write_all(bytes).map_err(|source| GotekError::Io {
        offset: *position,
        source,
    })?;
    *position = next_position;
    Ok(())
}

fn write_fill<W: Write + ?Sized>(
    writer: &mut W,
    position: &mut u64,
    limit: u64,
    byte: u8,
    mut count: u64,
) -> Result<(), GotekError> {
    let buffer = [byte; FILL_BUFFER_BYTES];
    while count != 0 {
        let chunk_size = count.min(FILL_BUFFER_BYTES as u64) as usize;
        write_bytes(writer, position, limit, &buffer[..chunk_size])?;
        count -= chunk_size as u64;
    }
    Ok(())
}
