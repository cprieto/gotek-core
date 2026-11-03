# gotek-core

A Rust library for inspecting, formatting, and reading FAT12 files in disk images
for the **original Gotek firmware family** using numbered floppy slots. The layout
was reverse engineered from **ipcas USB Floppy Emulator Manager / Batch Manage
Tool V1.40** captures. It is not a general HxC or FlashFloppy format library.
Physical Gotek hardware compatibility has not been verified.

Supported payload geometries are **720K, 1.2 MB, and 1.44 MB** (720, 1,200, and
1,440 KiB). Each occupies a fixed 1.5 MiB slot, including a gap and Gotek metadata.
Images support 1–100 slots and include an extra 512-byte boundary sector:
`slot_count * 0x180000 + 512` bytes, without device-capacity padding.

See the [format specification](GOTEK_FORMAT_SPEC.md),
[API documentation](https://docs.rs/gotek-core), and
[source repository](https://github.com/cprieto/gotek-core) for details.

## Installation

Add the library to a Rust project:

```sh
cargo add gotek-core
```

The library does not enable the CLI by default. The optional `gotek` executable
uses `clap` and exposes `inspect`, `init`, `ls`, and `cat`:

```sh
cargo install gotek-core --features cli
```

Before the first crates.io release, use a local checkout: `cargo install --path .
--features cli`. To run without installing, use `cargo run --features cli --
<command>`. The crate uses Rust edition 2024.

## Inspect an image

```rust,no_run
use gotek_core::inspect;
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut image = File::open("image.img")?;
    let info = inspect(&mut image)?;
    println!("{} slots; slot zero is {}", info.floppy_count, info.geometry);
    for slot in &info.slots {
        println!("{}: {} ({})", slot.index, slot.volume_label, slot.geometry);
    }
    Ok(())
}
```

```sh
gotek inspect image.img
cargo run --example inspect_image -- image.img
```

Inspection reads each slot's boot and metadata sectors plus the boundary sector.
Invalid structural BPBs, unsupported payload sizes, invalid counts, and I/O
failures are errors; deviations from observed conventions produce warnings.
Labels, raw metadata, local timestamps (without a timezone), and per-slot warnings
are retained. Mixed geometries are inspected independently and produce a warning.
Readers implement `Read + Seek`; byte zero must be slot zero. Inspection does not
need the total source length, mount filesystems, or discover devices.

## Create an empty image

```rust,no_run
use gotek_core::{FormatOptions, SlotFormatOptions, create_image};
use gotek_core::{geometry::FloppyGeometry, metadata::UpdateTime};

fn main() -> Result<(), gotek_core::GotekError> {
    let time = UpdateTime {
        year: 2026, month: 10, day: 31, hour: 13, minute: 12, second: 19,
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

```sh
gotek init new-image.img --floppies 50 --size 1.44m
```

CLI sizes are `720k`, `1.2m`, and `1.44m`. It currently uses fixed serial
`0x12345678` and timestamp `2026-11-01 12:00:00`, rather than the current time.
The library accepts explicit per-slot serials and timestamps through
`FormatOptions::slots`. Root-label dates must fit 1980–2107 and seconds round
down to FAT's two-second resolution; metadata retains full seconds.

Formatting initializes two FATs, numbered volume labels, data fill, gaps,
metadata, and the boundary copy. Generated boot sectors contain an original
nonbootable diagnostic stub; no Microsoft DOS or ipcas executable code is
included. See the [boot stub documentation](src/boot/README.md) and
[format-data fixture provenance](tests/fixtures/README.md).

## Read files without modifying the image

```rust,no_run
use gotek_core::{list_files, read_file};
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut image = File::open("image.img")?;
    for entry in list_files(&mut image, 5)? {
        println!("{}: {} bytes", entry.name, entry.size);
    }
    // For an image containing this file in slot 5:
    let bytes = read_file(&mut image, 5, "hello.txt.txt")?;
    println!("Read {} bytes", bytes.len());
    Ok(())
}
```

```sh
gotek ls image.img --slot 5
gotek cat image.img --slot 5 hello.txt.txt > extracted.txt
```

Slots are zero-based and checked against slot zero's declared count. `list_files`
returns root files and directories, omitting volume labels. `read_file` supports
slash-separated relative paths, VFAT long names, and ASCII case-insensitive long
or short names. `cat` outputs exact bytes without adding a newline. Only the
selected slot's BPB is validated; unrelated slots and the boundary need not be
readable. The `fatfs` adapter bounds access to the payload and rejects all writes.

## Limitations and safety

- Inspection checks boot sectors and metadata, not FAT chains, directories, gap
  fill, or trailing data. File access is also not a filesystem integrity check:
  FAT copies are not compared and cyclic chains can repeat data. Non-ASCII case
  folding is not supported; malformed VFAT names may fall back to short names.
- File import, modification, deletion, repair, complete floppy-image import, quick
  format, and DOS bootable formatting are not implemented.
- Prefer `create_image`, which atomically refuses existing paths, including
  devices and symlinks. `format_image` requires an empty `Write + Seek` destination;
  callers must supply a new regular image file or memory sink, never a raw device.
- `gotek init ... --force` reinitializes an existing regular image file and rejects
  devices, directories, and symlinks. `format_existing_image` checks the opened
  file type and capacity. Both preserve file length and bytes after the new
  boundary, including old trailing slot data. Back up images first and prevent
  concurrent writes or resizing.
- I/O failures can leave partial images. Writes are flushed, without `sync_all`
  durability guarantees or automatic cleanup. No raw-device formatting workflow
  or device-safety validation is provided.

## Compatibility evidence and tests

Confirmed capture-based checks cover ipcas V1.40 images with 50/100 slots across
all three geometries, including reading the imported `hello.txt.txt` (`hola!`)
from slot 5 of `image03.img`. Formatter comparisons against clean captures check
all bytes through the boundary except boot-code bytes 62–509 in slot boot sectors
and the boundary copy, where Gotek Core uses its own stub.

These checks establish agreement with captured ipcas data, not a Rust → ipcas
reopening/modification round trip. Such a round trip and physical Gotek tests are
not documented as completed. Counts other than 50/100, mixed-geometry hardware
support, other firmware/tool versions, and firmware requirements for metadata
and the boundary remain unverified.

Small format-data fixtures and generated tests run without private captures.
Optional tests read local `image01.img` through `image06.img` when present;
missing captures are skipped with diagnostics (`cargo test -- --nocapture`).
Full captures and local forensic reports are excluded from the Cargo package.
The format specification includes capture hashes and the research roadmap;
its planned interfaces are not the current API.

```sh
cargo fmt --check
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps --all-features
cargo publish --dry-run
```

## License

Licensed under either the [MIT license](LICENSE) or the
[Apache License, Version 2.0](LICENSE-APACHE), at your option
(`MIT OR Apache-2.0`). This does not grant rights to third-party research material
kept separately from the package.
