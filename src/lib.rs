//! Read-only inspection of the original Gotek layout observed in ipcas V1.40 captures.
//!
//! Offsets are relative to byte zero of the supplied source. No mounting or device
//! discovery is performed. This inspector checks boot sectors and metadata, not FAT
//! chains, directory contents, or file integrity. Recognition does not certify hardware
//! compatibility. See `GOTEK_FORMAT_SPEC.md` for evidence and remaining uncertainties.

mod error;
pub mod fat12;
pub mod geometry;
mod inspect;
pub mod layout;
pub mod metadata;

pub use error::GotekError;
pub use inspect::{
    GotekImageInfo, SlotInfo, ValidationWarning, WarningKind, WarningLocation, inspect,
};
