use std::fmt;

/// Supported BPB-defined payload sizes; these are independent of the slot stride.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloppyGeometry {
    /// 737,280 bytes (720 KiB).
    Kb720,
    /// 1,228,800 bytes (1,200 KiB).
    Mb1200,
    /// 1,474,560 bytes (1,440 KiB).
    Mb1440,
}

impl FloppyGeometry {
    /// Size of the FAT12 payload, including its boot sector.
    pub const fn payload_bytes(self) -> u64 {
        match self {
            Self::Kb720 => 737_280,
            Self::Mb1200 => 1_228_800,
            Self::Mb1440 => 1_474_560,
        }
    }

    pub(crate) fn from_payload_bytes(bytes: u64) -> Option<Self> {
        match bytes {
            737_280 => Some(Self::Kb720),
            1_228_800 => Some(Self::Mb1200),
            1_474_560 => Some(Self::Mb1440),
            _ => None,
        }
    }
}

impl fmt::Display for FloppyGeometry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Kb720 => "720 KB",
            Self::Mb1200 => "1.2 MB",
            Self::Mb1440 => "1.44 MB",
        })
    }
}
