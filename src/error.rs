use std::{fmt, io};

/// A failure during inspection, filesystem access, or image formatting.
#[derive(Debug)]
pub enum GotekError {
    /// A zero-based slot index is outside the image's declared count.
    InvalidSlot { index: u16, count: u16 },
    /// An I/O operation failed at this absolute byte offset.
    Io {
        /// Requested position, volume start for filesystem errors, or zero for destination queries.
        offset: u64,
        /// Underlying error (including `UnexpectedEof` for truncation).
        source: io::Error,
    },
    /// A boot sector cannot describe a structurally valid supported FAT12 volume.
    InvalidBpb {
        /// Absolute boot-sector offset.
        offset: u64,
        /// Failed structural rule.
        reason: &'static str,
    },
    /// The payload size does not match any supported floppy geometry.
    UnsupportedGeometry {
        /// Absolute boot-sector offset.
        offset: u64,
        /// Payload size reported by the BPB.
        payload_bytes: u64,
    },
    /// Slot zero's count record is absent or has invalid syntax.
    InvalidCountRecord,
    /// A syntactically valid count is outside 1–100.
    InvalidSlotCount(u16),
    /// Formatting inputs or a calculated write boundary are invalid.
    InvalidFormatOptions(&'static str),
    /// Formatting requires an empty destination; no bytes have been written.
    OutputNotEmpty(u64),
    /// Existing-image formatting only accepts regular files; no bytes were written.
    NotRegularFile,
    /// An existing image cannot hold the requested slots and boundary sector.
    ImageTooSmall {
        /// Existing file length in bytes.
        actual_bytes: u64,
        /// Minimum required length, including the boundary sector.
        required_bytes: u64,
    },
    /// An offset or size cannot be represented.
    ArithmeticOverflow,
}

impl fmt::Display for GotekError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlot { index, count } => {
                write!(formatter, "slot {index} is outside 0..{count}")
            }
            Self::Io { offset, source } => write!(formatter, "I/O at byte {offset:#x}: {source}"),
            Self::InvalidBpb { offset, reason } => {
                write!(formatter, "invalid BPB at {offset:#x}: {reason}")
            }
            Self::UnsupportedGeometry {
                offset,
                payload_bytes,
            } => write!(
                formatter,
                "unsupported payload size {payload_bytes} at {offset:#x}"
            ),
            Self::InvalidCountRecord => {
                formatter.write_str("expected NUL-terminated MAX BLOCK:NNN in slot zero")
            }
            Self::InvalidSlotCount(count) => {
                write!(formatter, "slot count {count} is outside 1–100")
            }
            Self::InvalidFormatOptions(reason) => {
                write!(formatter, "invalid format options: {reason}")
            }
            Self::OutputNotEmpty(length) => write!(
                formatter,
                "format destination must be empty, found {length} bytes"
            ),
            Self::NotRegularFile => {
                formatter.write_str("format destination must be a regular image file")
            }
            Self::ImageTooSmall {
                actual_bytes,
                required_bytes,
            } => write!(
                formatter,
                "image is too small: {actual_bytes} bytes available, {required_bytes} bytes required"
            ),
            Self::ArithmeticOverflow => formatter.write_str("offset or size arithmetic overflow"),
        }
    }
}

impl std::error::Error for GotekError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
