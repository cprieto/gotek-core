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
//! - [`list_files`] lists a slot's root; [`read_file`] reads relative FAT paths.
//!   [`extract_file`] saves a file to a new destination. These provide bounded, read-only access through `fatfs`.
//!
//! Readers implement [`std::io::Read`] + [`std::io::Seek`], with slot zero at byte
//! zero. No mounting or device discovery is performed. Inspection and file access
//! are not filesystem integrity checks. Formatting provides no raw-device safety
//! checks; use regular image files and keep backups before reinitializing them.
//!
//! ```no_run
//! let mut image = std::fs::File::open("image.img")?;
//! let info = gotek_core::inspect(&mut image)?;
//! println!("{} slots ({})", info.floppy_count, info.geometry);
//! for entry in gotek_core::list_files(&mut image, 0)? {
//!     println!("{}: {} bytes", entry.name, entry.size);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The library is available without optional features. The `cli` feature adds the
//! `gotek` executable (`inspect`, `init`, `ls`, `cat`, `extract`) and its `clap` dependency.
//! See the [format specification](https://github.com/cprieto/gotek-core/blob/main/GOTEK_FORMAT_SPEC.md)
//! for capture evidence and unresolved firmware questions.

mod error;
mod extract;
pub mod fat12;
mod files;
mod format;
pub mod geometry;
mod import;
mod inspect;
pub mod layout;
pub mod metadata;
mod slot_image;
mod usage;

pub use error::GotekError;
pub use extract::extract_file;
pub use files::{FileInfo, list_files, read_file};
pub use format::{
    FormatOptions, SlotFormatOptions, create_image, format_existing_image, format_image,
    generate_boot_sector,
};
pub use import::{import_file, import_into};
pub use inspect::{
    GotekImageInfo, SlotInfo, ValidationWarning, WarningKind, WarningLocation, inspect,
};
pub use slot_image::{load_image, save_image};
pub use usage::{SlotUsage, slot_usage};
