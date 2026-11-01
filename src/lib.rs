//! Inspection and empty-image formatting of the original Gotek layout observed in ipcas V1.40 captures.
//!
//! Offsets are relative to byte zero of the supplied source. No mounting or device
//! discovery is performed. This inspector checks boot sectors and metadata, not FAT
//! chains, directory contents, or file integrity. Recognition does not certify hardware
//! compatibility. Formatting targets regular image files: [`create_image`] refuses
//! existing paths; [`format_existing_image`] preserves an existing file's length.
//! No raw-device formatting is provided. [`list_files`] and [`read_file`] provide
//! bounded read-only FAT12 access through `fatfs`, independently of inspection.
//! See `GOTEK_FORMAT_SPEC.md` for evidence and remaining uncertainties.

mod error;
pub mod fat12;
mod files;
mod format;
pub mod geometry;
mod inspect;
pub mod layout;
pub mod metadata;

pub use error::GotekError;
pub use files::{FileInfo, list_files, read_file};
pub use format::{
    FormatOptions, SlotFormatOptions, create_image, format_existing_image, format_image,
    generate_boot_sector,
};
pub use inspect::{
    GotekImageInfo, SlotInfo, ValidationWarning, WarningKind, WarningLocation, inspect,
};
