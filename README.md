# gotek-core

A dependency-free Rust 2024 library for **read-only inspection** of the original
Gotek slot layout observed in ipcas USB Floppy Emulator Manager / Batch Manage
Tool V1.40 captures. The format evidence is in [GOTEK_FORMAT_SPEC.md](GOTEK_FORMAT_SPEC.md).

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
No formatting or file management is implemented.

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
