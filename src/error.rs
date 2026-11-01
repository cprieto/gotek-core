use std::{fmt, io};

/// A failure that prevents reliable inspection. Convention differences are warnings.
#[derive(Debug)]
pub enum GotekError {
    /// A seek or exact sector read failed at this absolute byte offset.
    Io {
        /// Requested sector offset.
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
