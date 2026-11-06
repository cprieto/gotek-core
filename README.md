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
- List and read FAT12 files, including VFAT long names and nested paths; import
  single files and inspect cluster usage.
- Change slot volume labels and transfer complete floppy images.
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
Open an existing image with `GotekDisk::open(target, capacity)` and call
`disk.slot_mut(5)?.set_label("DATA DISK")` to rename a slot. The path-based
`set_slot_label()` helper retains regular-image-file checks.

### Work with a slot

Slot numbers are zero-based. This example expects `docs/readme.txt` in slot 5
of an existing image:

```rust,no_run
use gotek_core::GotekDisk;
use std::fs::{File, OpenOptions};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let image = File::open("image.img")?;
    let capacity = image.metadata()?.len();
    let mut disk = GotekDisk::open(image, capacity)?;
    let mut slot = disk.slot(5)?;
    println!("{}: {:?}", slot.label()?, slot.metadata()?.update_time);
    for entry in slot.files()? {
        println!("{}: {} bytes", entry.name, entry.size);
    }
    let bytes = slot.read_file("docs/readme.txt")?;
    println!("{}", String::from_utf8_lossy(&bytes));
    let mut output = OpenOptions::new().write(true).create_new(true).open("readme.txt")?;
    output.write_all(&bytes)?;
    println!("{} clusters used", slot.usage()?.used_clusters);
    Ok(())
}
```

`files()` lists root files and directories. `read_file()` accepts slash-separated
relative paths and returns exact bytes. The path-based `extract_file()` helper
saves a file without replacing an existing destination.

`GotekDisk::open()` checks slot zero, the slot count, and declared capacity.
Each slot is validated when accessed, so a damaged slot does not block reading
another. Read-only access works without the extra boundary sector; mutation
requires it. Use `inspect()` to check the complete container.

### Load a complete floppy image

`SlotMut::load_image()` replaces one slot with a complete FAT12 image of the
same geometry. `Slot::save_image()` exports its exact payload bytes.

```rust,no_run
use gotek_core::{GotekDisk, metadata::UpdateTime};
use std::fs::{File, OpenOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let image = OpenOptions::new().read(true).write(true).open("disk.img")?;
    let capacity = image.metadata()?.len();
    let mut disk = GotekDisk::open(image, capacity)?;
    let time = UpdateTime { year: 2026, month: 11, day: 3, hour: 9, minute: 10, second: 11 };
    disk.slot_mut(5)?.load_image(&mut File::open("boot.img")?, time)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open("saved.img")?;
    disk.slot(5)?.save_image(&mut output)?;
    Ok(())
}
```

### Import a file

```rust,no_run
use gotek_core::{GotekDisk, metadata::UpdateTime};
use std::fs::{self, OpenOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let image = OpenOptions::new().read(true).write(true).open("image.img")?;
    let capacity = image.metadata()?.len();
    let mut disk = GotekDisk::open(image, capacity)?;
    let time = UpdateTime { year: 2026, month: 11, day: 3, hour: 9, minute: 10, second: 11 };
    let bytes = fs::read("HELLO.TXT")?;
    disk.slot_mut(5)?.import_file("HELLO.TXT", &bytes, false, time)?;
    assert_eq!(disk.slot(5)?.read_file("HELLO.TXT")?, bytes);
    Ok(())
}
```

Set `overwrite` to `true` to truncate and replace an existing FAT12 file.
Back up the image before import or complete-image replacement; physical writes
are not power-failure atomic. The caller supplies an accurate capacity and
exclusive access for device descriptors.

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
