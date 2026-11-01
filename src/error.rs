use std::{fmt, io};

/// A failure during inspection or empty-image formatting.
#[derive(Debug)]
pub enum GotekError {
    /// An I/O operation failed at this absolute byte offset.
    Io {
        /// Requested read/write position, or zero when querying the destination.
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
    /// An offset or size cannot be represented.
    ArithmeticOverflow,
}

impl fmt::Display for GotekError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
