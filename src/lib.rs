//! Inspect, format, and read FAT12 files in original-firmware Gotek floppy images.
//!
//! Targets the numbered-slot layout reverse engineered from **ipcas USB Floppy
//! Emulator Manager V1.40**, with 720K, 1.2 MB, and 1.44 MB FAT12 payloads inside
//! fixed 1.5 MiB slots. This is not a general HxC or FlashFloppy format library;
//! physical Gotek compatibility has not been verified.
//!
//! - [`inspect`] validates boot sectors and Gotek metadata and reports warnings.
//! - [`create_image`] creates an empty image without replacing an existing path.
//!   [`format_existing_image`] reinitializes a regular file, preserving its length.
//! - [`GotekDisk`] identifies an already-open container from slot zero and the
//!   count record. Requested slots are validated lazily. [`Slot`] provides
//!   read-only FAT12 access and complete-image export. [`SlotMut`] adds label
//!   editing, single-file import, and complete-image replacement.
//!   [`inspect`] remains the explicit full-container check.
//!
//! Readers implement [`std::io::Read`] + [`std::io::Seek`], with slot zero at byte
//! zero. No mounting or device discovery is performed. Inspection and file access
//! are not filesystem integrity checks. Formatting provides no raw-device safety
//! checks; use regular image files and keep backups before reinitializing them.
//!
//! ```no_run
//! let image = std::fs::File::open("image.img")?;
//! let capacity = image.metadata()?.len();
//! let mut disk = gotek_core::GotekDisk::open(image, capacity)?;
//! let mut slot = disk.slot(0)?;
//! println!("{} ({})", slot.label()?, slot.geometry());
//! for entry in slot.files()? {
//!     println!("{}: {} bytes", entry.name, entry.size);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The library is available without optional features. The `cli` feature adds the
//! `gotek` executable (`inspect`, `init`, `ls`, `cat`, `extract`) and its `clap` dependency.
//! See the [format specification](https://github.com/cprieto/gotek-core/blob/main/GOTEK_FORMAT_SPEC.md)
//! for capture evidence and unresolved firmware questions.

mod disk;
mod error;
mod extract;
pub mod fat12;
mod files;
mod format;
pub mod geometry;
mod import;
mod inspect;
mod label;
pub mod layout;
pub mod metadata;
mod slot;
mod slot_image;
mod usage;

pub use disk::GotekDisk;
pub use error::GotekError;
pub use extract::extract_file;
pub use files::{FileInfo, list_files, read_file};
pub use format::{
    FormatOptions, SlotFormatOptions, create_image, format_existing_image, format_image, format_on,
    generate_boot_sector,
};
pub use import::{import_file, import_into};
pub use inspect::{
    GotekImageInfo, SlotInfo, ValidationWarning, WarningKind, WarningLocation, inspect,
};
pub use label::{set_slot_label, set_slot_label_on};
pub use slot::{Slot, SlotMut};
pub use slot_image::{load_image, save_image};
pub use usage::{SlotUsage, slot_usage};
