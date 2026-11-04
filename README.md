# gotek-core

`gotek-core` is a Rust library for inspecting, creating, and reading files from
original [Gotek USB floppy emulator](https://www.gotekemulator.com/) disk images. Work with numbered floppy slots
without mounting the image.

[API documentation](https://docs.rs/gotek-core) ·
[Repository](https://github.com/cprieto/gotek-core) ·
[Technical format specification](GOTEK_FORMAT_SPEC.md)


## Features

- Inspect slot counts, floppy geometries, volume labels, and timestamps, with
  validation errors and warnings.
- Create empty images with 1–100 slots using **720K, 1.2 MB, or 1.44 MB** geometry.
- Reinitialize existing regular image files while preserving their total size.
- List FAT12 root directories, read, extract, and import files, including VFAT long names and
  nested paths, without modifying the source.
- Report FAT12 cluster allocation for a slot with `slot_usage()`.
- Change a slot's FAT12 volume label with `set_slot_label()`.
- Use files or other `Read + Seek` sources, including in-memory buffers.

## Installation

```sh
cargo add gotek-core
```

## Library usage

### Inspect an image

```rust,no_run
use gotek_core::inspect;
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut image = File::open("image.img")?;
    let info = inspect(&mut image)?;
    println!("{} slots", info.floppy_count);
    for slot in &info.slots {
        println!("{}: {} ({})", slot.index, slot.volume_label, slot.geometry);
    }
    Ok(())
}
```

### Create an image

```rust,no_run
use gotek_core::{FormatOptions, SlotFormatOptions, create_image};
use gotek_core::{geometry::FloppyGeometry, metadata::UpdateTime};

fn main() -> Result<(), gotek_core::GotekError> {
    let time = UpdateTime {
        year: 2026, month: 11, day: 1, hour: 12, minute: 0, second: 0,
    };
    let slot = SlotFormatOptions {
        volume_serial: 0x1234_5678,
        update_time: time,
        root_label_time: time,
    };
    let options = FormatOptions::new(FloppyGeometry::Mb1440, 50, slot)?;
    create_image("new-image.img", &options)?;
    Ok(())
}
```

`create_image` refuses to replace an existing path. Supply serials and timestamps
explicitly; edit `options.slots` to use different values for individual slots.
To rename an existing slot, call `set_slot_label("image.img", 5, "DATA DISK")`.
For an already-open device descriptor, use
`set_slot_label_on(&mut device, capacity, 5, "DATA DISK")`, supplying its full
capacity in bytes.

### List, read, and extract files

Slot numbers are zero-based. This example expects `docs/readme.txt` in slot 5
of an existing image:

```rust,no_run
use gotek_core::{extract_file, list_files, read_file};
use std::{fs::File, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut image = File::open(Path::new("image.img"))?;
    for entry in list_files(&mut image, 5)? {
        println!("{}: {} bytes", entry.name, entry.size);
    }
    let bytes = read_file(&mut image, 5, "docs/readme.txt")?;
    println!("{}", String::from_utf8_lossy(&bytes));
    extract_file(&mut image, 5, "docs/readme.txt", Path::new("readme.txt"))?;
    Ok(())
}
```

`list_files` lists root files and directories. `read_file` returns exact bytes
and accepts slash-separated relative paths and ASCII case-insensitive names.
`extract_file` saves exact bytes to a new file; it refuses existing destinations
and requires parent directories to exist.

### Load a complete floppy image

`load_image` replaces one slot with a complete FAT12 floppy image.
`save_image` exports a slot as a standalone image with its payload bytes unchanged. The source
must have the same supported geometry as the selected slot; back up the Gotek
image first.

```rust,no_run
use gotek_core::{load_image, save_image, metadata::UpdateTime};
let time = UpdateTime { year: 2026, month: 11, day: 3, hour: 9, minute: 10, second: 11 };
load_image("disk.img", 5, "boot.img", time)?;
save_image("disk.img", 5, "saved.img")?;
# Ok::<(), gotek_core::GotekError>(())
```

### Import a file

```rust,no_run
use gotek_core::{import_file, metadata::UpdateTime};
use std::path::Path;
let time = UpdateTime { year: 2026, month: 11, day: 3, hour: 9, minute: 10, second: 11 };
import_file(Path::new("image.img"), 5, Path::new("HELLO.TXT"), None, false, time)?;
# Ok::<(), gotek_core::GotekError>(())
```

The optional name replaces the source basename. Set the overwrite argument to
`true` to truncate and replace an existing file in that slot. Back up the image
before import; a write error can leave partial changes.

## Optional CLI

`cli` is the only optional Cargo feature and is disabled by default. It enables
the `gotek` executable; library use does not require it.

```sh
cargo install gotek-core --features cli
```

From a local checkout, use `cargo install --path . --features cli`, or run commands
with `cargo run --features cli -- <command>`.

### Commands

```sh
# Inspect an existing image.
gotek inspect image.img

# Create a new image with 50 empty 1.44 MB floppy slots.
gotek init new-image.img --floppies 50 --size 1.44m

# List slot 5 and print a file to stdout.
gotek ls image.img --slot 5
gotek cat image.img --slot 5 docs/readme.txt

# Save a file using its basename, or choose an output path.
gotek extract image.img --slot 5 docs/readme.txt
gotek extract image.img --slot 5 docs/readme.txt --output output.txt
gotek extract image.img --slot 5 docs/readme.txt --output output.txt --force

# Replace slot 5 with a complete FAT12 floppy image of the same geometry.
gotek load-image disk.img --slot 5 boot.img
gotek save-image disk.img --slot 5 saved.img

# Import a local file into slot 5; optionally rename or replace it.
gotek import image.img --slot 5 HELLO.TXT
gotek import image.img --slot 5 HELLO.TXT --name GREETING.TXT
gotek import image.img --slot 5 HELLO.TXT --force

# Reinitialize an existing image (destructive: back it up first).
gotek init new-image.img --floppies 50 --size 1.44m --force
```

`--size` accepts `720k`, `1.2m`, and `1.44m`. `cat` writes binary bytes to stdout
without adding a newline; `extract` saves them to disk. Extraction refuses to
overwrite existing files unless `--force` is specified, which applies only to
the destination. Parent directories must already exist.

CLI initialization currently uses fixed serial
`0x12345678` and timestamp `2026-11-01 12:00:00`; use the library for custom values.

Without `--force`, initialization refuses existing paths. With `--force`, it
reinitializes a regular image file large enough for the requested slots, preserving
its length and trailing data. Back up the image before reinitializing it.

`import` and `extract` transfer individual FAT12 files. `load-image` and
`save-image` transfer complete floppy images; `save-image` refuses to replace an
existing file.

## Acknowledgments

The supported disk format was reverse-engineered from images generated by
**ipcas USB Floppy Emulator Manager V1.40**. The
[usbfd project (usbfd-tools)](https://gitlab.com/rreiner/usbfd-tools) provided
inspiration.

## License

Licensed under either the [MIT license](LICENSE) or the
[Apache License, Version 2.0](LICENSE-APACHE), at your option
(`MIT OR Apache-2.0`).

## AI-Assisted Development

This project was developed with the assistance of AI tooling.

AI tools have been used for collaborative design discussions, reveerse-engineering analysis, code generation and testing support.

Direction, technical decisions, experimental validation, and final review remain under human responsibility.

AI-generated contributions are reviewed and tested before being incorporated into the project.
