use std::io::{Read, Seek, SeekFrom};

use crate::{
    GotekError,
    fat12::Bpb,
    geometry::FloppyGeometry,
    layout::{METADATA_OFFSET_BYTES, SECTOR_BYTES, checked_offset, slot_offset},
    metadata::{SlotMetadata, parse_count, parse_metadata},
};

/// Location of a nonfatal finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningLocation {
    /// A zero-based floppy slot.
    Slot(u16),
    /// The extra sector immediately after the declared slots.
    Boundary,
}

/// Differences that do not prevent structural inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningKind {
    /// An observed ipcas convention differs; the field identifies the convention.
    IpcasConvention(&'static str),
    /// The timestamp is missing, not terminated, or not a valid calendar time.
    InvalidUpdateTime,
    /// A slot differs from slot zero. Hardware support for mixed media is untested.
    MixedGeometry,
    /// The boundary sector does not start with three zero bytes.
    BoundaryJump,
    /// Bytes 3–511 differ from the last slot's boot sector.
    BoundaryCopyMismatch,
    /// A boot label contains bytes outside printable ASCII; raw bytes are retained.
    NonAsciiLabel,
}

/// A nonfatal observation, attached to its slot or the boundary sector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationWarning {
    /// Where the observation occurred.
    pub location: WarningLocation,
    /// Machine-readable classification.
    pub kind: WarningKind,
}

/// Boot-sector and metadata information for a single floppy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotInfo {
    /// Zero-based index, independent of the editable volume label.
    pub index: u16,
    /// Absolute byte offset of this slot's boot sector.
    pub offset: u64,
    /// Geometry detected from this slot's BPB.
    pub geometry: FloppyGeometry,
    /// Parsed FAT12 allocation parameters.
    pub bpb: Bpb,
    /// Boot label with trailing spaces removed; invalid UTF-8 is replaced for display.
    /// This is not a root-directory label lookup.
    pub volume_label: String,
    /// Exact 11 boot-label bytes, without any encoding assumptions.
    pub raw_volume_label: [u8; 11],
    /// Recognized records in the final metadata sector.
    pub metadata: SlotMetadata,
    /// Findings pertaining to this slot.
    pub warnings: Vec<ValidationWarning>,
}

/// Result of inspecting a candidate ipcas image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotekImageInfo {
    /// Geometry of slot zero. Inspect individual slots for mixed geometries.
    pub geometry: FloppyGeometry,
    /// Count from slot zero's metadata, never inferred from source size.
    pub floppy_count: u16,
    /// Declared slots, in index order.
    pub slots: Vec<SlotInfo>,
    /// Absolute location of the extra boot-sector copy.
    pub boundary_offset: u64,
    /// Parsed boundary BPB; this sector is not an additional usable floppy.
    pub boundary_bpb: Bpb,
    /// Findings pertaining only to the boundary sector.
    pub warnings: Vec<ValidationWarning>,
}

/// Inspect sectors of an ipcas Gotek image without writing or mounting it.
///
/// Reads exactly one boot and one metadata sector per declared slot, plus the
/// boundary sector. It does not require a known source length or seek to EOF,
/// so the same implementation works with files, block devices, and test readers.
/// Byte zero of `reader` must be the start of slot zero, regardless of its initial
/// cursor position. The cursor is left after the boundary sector on success; its
/// position on error is unspecified. FAT contents and file integrity are not checked.
///
/// Structural BPB failures, unsupported payload sizes, bad slot counts, and I/O
/// failures are errors. Observed ipcas conventions (including timestamp and boundary
/// copy contents) produce warnings when their absence still permits inspection.
pub fn inspect<R: Read + Seek + ?Sized>(reader: &mut R) -> Result<GotekImageInfo, GotekError> {
    let first_boot = read_sector(reader, 0)?;
    let first_bpb = Bpb::parse(&first_boot, 0)?;
    let geometry = detect_geometry(&first_bpb, 0)?;
    let first_metadata = read_sector(reader, METADATA_OFFSET_BYTES)?;
    let floppy_count = parse_count(&first_metadata)?;
    let mut slots = Vec::with_capacity(usize::from(floppy_count));
    let mut last_boot = first_boot;
    for index in 0..floppy_count {
        let offset = slot_offset(u64::from(index))?;
        let (boot_sector, metadata_sector) = if index == 0 {
            (first_boot, first_metadata)
        } else {
            (
                read_sector(reader, offset)?,
                read_sector(reader, checked_offset(offset, METADATA_OFFSET_BYTES)?)?,
            )
        };
        let bpb = if index == 0 {
            first_bpb.clone()
        } else {
            Bpb::parse(&boot_sector, offset)?
        };
        let slot_geometry = detect_geometry(&bpb, offset)?;
        let location = WarningLocation::Slot(index);
        let mut warnings = convention_warnings(&boot_sector, &bpb, slot_geometry, location);
        if slot_geometry != geometry {
            warnings.push(ValidationWarning {
                location,
                kind: WarningKind::MixedGeometry,
            });
        }
        let metadata = parse_metadata(&metadata_sector, (index == 0).then_some(floppy_count));
        if metadata.update_time.is_none() {
            warnings.push(ValidationWarning {
                location,
                kind: WarningKind::InvalidUpdateTime,
            });
        }
        let mut raw_volume_label = [0; 11];
        raw_volume_label.copy_from_slice(&boot_sector[43..54]);
        if !raw_volume_label
            .iter()
            .all(|byte| (0x20..=0x7e).contains(byte))
        {
            warnings.push(ValidationWarning {
                location,
                kind: WarningKind::NonAsciiLabel,
            });
        }
        let volume_label = String::from_utf8_lossy(&raw_volume_label)
            .trim_end_matches(' ')
            .to_owned();
        if volume_label != format!("FLPPY{index}") {
            warnings.push(ValidationWarning {
                location,
                kind: WarningKind::IpcasConvention("numbered boot label"),
            });
        }
        slots.push(SlotInfo {
            index,
            offset,
            geometry: slot_geometry,
            bpb,
            volume_label,
            raw_volume_label,
            metadata,
            warnings,
        });
        last_boot = boot_sector;
    }
    let boundary_offset = slot_offset(u64::from(floppy_count))?;
    let boundary_sector = read_sector(reader, boundary_offset)?;
    let boundary_bpb = Bpb::parse(&boundary_sector, boundary_offset)?;
    detect_geometry(&boundary_bpb, boundary_offset)?;
    let location = WarningLocation::Boundary;
    let mut warnings = Vec::new();
    if boundary_sector[..3] != [0; 3] {
        warnings.push(ValidationWarning {
            location,
            kind: WarningKind::BoundaryJump,
        });
    }
    if boundary_sector[3..] != last_boot[3..] {
        warnings.push(ValidationWarning {
            location,
            kind: WarningKind::BoundaryCopyMismatch,
        });
    }
    Ok(GotekImageInfo {
        geometry,
        floppy_count,
        slots,
        boundary_offset,
        boundary_bpb,
        warnings,
    })
}

pub(crate) fn read_sector<R: Read + Seek + ?Sized>(
    reader: &mut R,
    offset: u64,
) -> Result<[u8; SECTOR_BYTES], GotekError> {
    checked_offset(offset, SECTOR_BYTES as u64)?;
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|source| GotekError::Io { offset, source })?;
    let mut sector = [0; SECTOR_BYTES];
    reader
        .read_exact(&mut sector)
        .map_err(|source| GotekError::Io { offset, source })?;
    Ok(sector)
}

pub(crate) fn detect_geometry(bpb: &Bpb, offset: u64) -> Result<FloppyGeometry, GotekError> {
    let payload_bytes = bpb.payload_bytes()?;
    FloppyGeometry::from_payload_bytes(payload_bytes).ok_or(GotekError::UnsupportedGeometry {
        offset,
        payload_bytes,
    })
}

fn convention_warnings(
    sector: &[u8; SECTOR_BYTES],
    bpb: &Bpb,
    geometry: FloppyGeometry,
    location: WarningLocation,
) -> Vec<ValidationWarning> {
    // These exact profile values were observed in captures; differing but coherent
    // FAT12 allocation parameters do not by themselves make a volume unreadable.
    let (cluster_sectors, root_entries, fat_sectors, media, track_sectors) = match geometry {
        FloppyGeometry::Kb720 => (2, 112, 3, 0xf9, 9),
        FloppyGeometry::Mb1200 => (2, 224, 7, 0xf9, 15),
        FloppyGeometry::Mb1440 => (1, 224, 9, 0xf0, 18),
    };
    let conventions = [
        (sector[..3] == [0xeb, 0x3c, 0x90], "boot jump"),
        (&sector[3..11] == b"MSDOS5.0", "OEM identifier"),
        (sector[38] == 0x29, "extended boot signature"),
        (&sector[54..62] == b"FAT12   ", "filesystem type text"),
        (
            bpb.sectors_per_cluster == cluster_sectors,
            "sectors per cluster",
        ),
        (bpb.root_entries == root_entries, "root entry count"),
        (bpb.sectors_per_fat == fat_sectors, "sectors per FAT"),
        (bpb.media_descriptor == media, "media descriptor"),
        (
            bpb.sectors_per_track == track_sectors && bpb.heads == 2,
            "BIOS geometry",
        ),
        (
            bpb.reserved_sectors == 1 && bpb.fat_count == 2,
            "reserved sectors and FAT count",
        ),
        (bpb.hidden_sectors == 0, "hidden sector count"),
        (sector[36..38] == [0, 0], "drive number and reserved byte"),
        (sector[32..36] == [0; 4], "unused large total-sector field"),
    ];
    conventions
        .into_iter()
        .filter_map(|(matches, field)| {
            (!matches).then_some(ValidationWarning {
                location,
                kind: WarningKind::IpcasConvention(field),
            })
        })
        .collect()
}
