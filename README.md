# gotek-core

A Rust 2024 library for inspection and empty-image generation of the original
Gotek slot layout observed in ipcas USB Floppy Emulator Manager / Batch Manage
Tool V1.40 captures. The library implementation uses only the standard library;
the existing `tester` inspection binary uses `clap`. The format evidence is in [GOTEK_FORMAT_SPEC.md](GOTEK_FORMAT_SPEC.md).

```rust,no_run
use std::fs::File;
use gotek_core::inspect;

let mut image = File::open("image.img")?;
let info = inspect(&mut image)?;
println!("{} slots; slot zero is {}", info.floppy_count, info.geometry);
for slot in &info.slots {
    println!("{}: {}", slot.index, slot.volume_label);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

Run the educational example with:

```sh
cargo run --example inspect_image -- image02.img
```

The same `Read + Seek` API accepts files, caller-opened block devices, and
`Cursor` data. Byte zero must be slot zero; inspection seeks there even if the
reader initially points elsewhere. The API never requests `Write`, never mounts
anything, and never discovers or opens devices itself. It reads 512-byte boot
and metadata sectors for each slot plus one boundary sector (102,912 bytes for
100 slots). It never queries the total device length. Reader position is left
after the boundary sector on success and is unspecified after failure.

`GotekImageInfo` contains slot zero's geometry, the declared slot count,
individual `SlotInfo` records, the boundary BPB, and boundary warnings. Each slot
has its own geometry, BPB, boot label, metadata, and warnings. Raw label and
update-record bytes are retained when text cannot be represented faithfully.
Timestamps are calendar-validated local wall-clock values with no assumed timezone.

`fat12` parses structural FAT12 allocation parameters. `geometry` identifies the
three supported payload sizes. `layout` owns the independent fixed-stride
addressing rules. `metadata` parses Gotek records. The inspection workflow joins
these parsers to sector I/O, and `GotekError` preserves I/O errors and offsets.

Validation policy:

- Invalid structural BPBs, unsupported payload sizes, unreadable sectors, invalid
  `MAX BLOCK:NNN` syntax, and counts outside 1–100 are errors. FAT bounds and
  FAT12 cluster counts are derived, including the 1.2 MB partial-cluster remainder.
- OEM/jump/type strings, exact allocation profile values, numbered labels, and
  malformed or absent update times produce warnings. Edited labels are allowed.
- The boundary sector must have a structurally valid supported BPB and boot
  signature. Nonzero jump bytes or a mismatch against bytes 3–511 of the last
  boot sector produce boundary warnings, since firmware necessity is unknown.
- Each slot is parsed independently. Mixed geometry produces a warning;
  hardware compatibility is untested. Geometry is identified by payload length,
  with deviations from captured allocation/BIOS profiles reported as conventions.

This is a boot-sector and metadata inspector, **not a filesystem integrity
checker**: it does not read FAT contents, directory labels, files, gap fill, or
bytes after the boundary sector. It does not certify hardware compatibility.
Empty-image formatting is described below. File management is not implemented.

The observations establish 50- and 100-slot count encodings. Accepting other
counts from 1–100, including `001`, generalizes the specified three-digit rule;
these counts were not verified against the supplied real captures. Which
metadata and boundary bytes firmware needs, mixed-geometry support, and other
formatter options remain unresolved. Timestamp/calendar validation and the
error-versus-warning policy above are implementation choices, not claims about
firmware behavior.

Tests use `Cursor` and a sparse reader supporting short reads without exposing
its length. They cover all geometries with 1, 50, and 100 slots, malformed input,
boundary differences, and I/O failures. If present in the repository root,
`image01.img` through `image06.img` are also opened read-only and checked against
the documented counts, geometries, labels, and metadata. Missing local captures
are skipped with a diagnostic (`cargo test -- --nocapture` shows it). The large
captures are ignored by Git; keep them locally as reference evidence.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Creating an empty image

```rust,no_run
use gotek_core::{FormatOptions, SlotFormatOptions, create_image};
use gotek_core::{geometry::FloppyGeometry, metadata::UpdateTime};

let time = UpdateTime {
    year: 2026, month: 10, day: 31, hour: 13, minute: 12, second: 19,
};
let slot = SlotFormatOptions {
    volume_serial: 0x1234_5678,
    update_time: time,
    root_label_time: time,
};
let options = FormatOptions::new(FloppyGeometry::Mb1440, 100, slot)?;
// This refuses any existing path, including existing device paths and symlinks.
create_image("new-image.img", &options)?;
# Ok::<(), gotek_core::GotekError>(())
```

`FormatOptions::slots` is the ordered list of slot inputs; its length is the
configured count, so there is no second count to keep in sync. The constructor
repeats the supplied inputs, and callers may edit individual slots before
formatting. Serials, root-label times, and metadata times are explicit; no clock,
timezone, or undocumented ipcas serial-generation algorithm is assumed.

`format_image(&mut writer, &options)` is also available for newly created regular
files and memory/test writers implementing `Write + Seek`. It rejects nonempty
outputs, validates every slot before writing, writes with a 16 KiB fill buffer,
and flushes at the end. A generic writer cannot identify its underlying device:
callers must provide a new image file or test sink, never a raw device. For paths,
prefer `create_image`, which uses atomic `create_new` and never replaces a path.
I/O failure may leave a partial image. Neither entry point promises disk durability
via `sync_all`, and neither automatically deletes partially written files.

The exact output size is `count * 0x180000 + 512`, including the extra boundary
sector. Unlike the full-device captures, a new image has no device-capacity
padding after that sector. Every slot contains a generated FAT12 boot sector with an original diagnostic stub,
two initialized FATs, the label-only root, `0xF6` data fill, the zero final payload
sector and gap, and the two documented metadata records (count in slot zero only).
`generate_boot_sector` separately returns a complete sector without positioning it.
`format` prepares these structures and streams them to the destination; existing
inspection logic is unchanged apart from sharing its timestamp parser internally.

The format-data-only BPB and root-label golden fixtures include provenance and
hashes in `tests/fixtures/README.md`. The original boot stub is documented in
`src/boot/README.md`. The root-label timestamp
word is deliberately copied to bytes 13–14 as well as 22–23, reproducing the
observed ipcas quirk. FAT seconds are rounded down to two-second resolution;
root-label years must fit 1980–2107. Metadata retains full seconds and supports
calendar years 1–9999. This is a byte-reproduction policy, not a firmware rule.

Formatter tests cover all three geometries at 1, 50, and 100 slots, the BPB round
trip, every initialized region, variable fields, boundary and metadata bytes,
nonempty destinations, invalid dates/counts, short writes, and I/O failures.
The small golden-fixture tests always run. Optional tests for clean captures
`image02`, `image04`, `image05`, and `image06` use their explicit serial/time inputs
and compare **every byte through the boundary sector except boot-code bytes
62–509 of each slot boot sector and the boundary copy**. BPB headers, signatures,
FATs, root entries, data fill, gaps, and metadata are compared without exclusions.
Run `cargo test -- --nocapture` to see per-capture comparison or skip messages.

No independent ipcas reopening or physical Gotek test has been performed. Matching
captured bytes does not establish bidirectional or hardware compatibility. Counts
other than 50/100, firmware use of metadata and the boundary, quick format, DOS
boot options, and the original serial/time generation remain unverified.

## Licensing and non-system images

Gotek Core is MIT licensed; see `LICENSE`. Its FAT12 boot sectors are generated
entirely by Gotek Core, including an original, independently authored nonbootable
x86 stub that displays `Non-system disk` and waits indefinitely. No Microsoft DOS
or ipcas executable boot code is distributed. The literal `MSDOS5.0` OEM string
is retained as the observed header identifier, not as an OS loader.

The original assembly and byte-reproduction instructions are in
`src/boot/non_system.S` and `src/boot/README.md`; normal builds need no assembler.
Captured executable templates have been removed, including test copies. Remaining
fixtures contain only BPB/header and directory-entry format data. A Cargo package
allowlist excludes private full-device captures and local forensic reports that
contain executable hex dumps. Those research reports are also ignored by Git.
The MIT license does not grant rights to third-party material kept separately
for research.

Future complete floppy-image import will let users supply their own bootable
boot sector and filesystem; that feature is not implemented here. Original Gotek
firmware and ipcas compatibility, including their handling of this changed boot
code, remain unverified until independently tested.

## Reinitializing an existing image with the CLI

```sh
cargo run --features cli -- init gotek-test01.img --floppies 50 --size 1.44m --force
```

Without `--force`, `init` continues to refuse every existing path. With `--force`,
it can overwrite an existing regular image file; missing paths still create a new
image. Directories, symlinks (including dangling links), block devices, and other
special files are rejected. The opened file handle is also checked before writing.

Existing files must already contain at least `floppies * 0x180000 + 512` bytes,
including the boundary sector. The fixed stride makes this required length the
same for all three supported geometries. All formatting inputs and capacity are
validated before any writes. The file is never truncated or extended, and bytes
after the new boundary sector are preserved, including any old trailing slot data.
An I/O failure can leave a partially reinitialized prefix. Avoid concurrent writes
or resizing while formatting.

The core's `format_existing_image(&mut File, &FormatOptions)` implements the
regular-file and capacity checks and shares the initialization engine with
`format_image`. `format_image` still requires an empty destination, and
`create_image` still refuses existing paths. Clap remains optional and confined
to the CLI. Run `cargo test --features cli` to include the command-level tests.
