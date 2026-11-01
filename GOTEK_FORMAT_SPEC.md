# Gotek USB Floppy Format — Findings and Implementation Plan

**Status:** Working specification, 2026-10-31  
**Reference formatter:** ipcas USB Floppy Emulator Manager / Batch Manage Tool V1.40 (Windows XP)  
**Target:** Rust library (`gotek-core`, provisional), CLI, then GTK application  
**Scope:** Original Gotek-style USB floppy layout observed in the captured ipcas images. Not a universal Gotek/HxC/FlashFloppy specification.

## 1. Goal and evidence levels

Build a tool that can **format a USB drive into numbered FAT12 floppy slots**, list/import/export files, and interoperate **both ways** with ipcas. Test the resulting media on a physical Gotek afterward.

- **Observed:** Repeatedly verified in the supplied raw USB images and forensic reports.
- **Inferred:** A reasonable implementation rule consistent with those observations, but not directly proven necessary.
- **Untested:** Needs an interoperability or hardware experiment.

**Important:** These captures establish the bytes ipcas *produces*, not which bytes Gotek firmware *requires* or how the firmware interprets them.

## 2. Container layout (observed)

The USB device is **partitionless**: slot 0's FAT12 boot sector begins at byte offset 0. Each slot has a fixed physical allocation of **1,572,864 bytes = `0x180000` = 3,072 × 512-byte sectors (1.5 MiB)**, regardless of the three tested floppy geometries.

For zero-based slot index `n`:

```text
slot_start(n)       = n * 0x180000
fat12_payload_start = slot_start(n)
fat12_payload_end   = slot_start(n) + BPB.total_sectors * 512
metadata_sector     = slot_start(n) + 0x17FE00
next_slot           = slot_start(n) + 0x180000
bank_end            = slot_count * 0x180000
```

Within every slot:

```text
+0x000000    FAT12 boot sector, FATs, root directory, data region
             (payload length depends on the BPB)
             Zero-filled gap after payload (in clean captures)
+0x17FE00    Final 512-byte metadata sector
+0x180000    Next slot
```

The metadata sector is the **last sector of the fixed stride**, not necessarily the last sector of the FAT12 volume. The BPB defines the payload size and filesystem internals; **FAT12 does not define the outer 1.5 MiB stride**.

## 3. Supported geometry profiles (observed)

All use 512-byte sectors, one reserved sector, two FAT copies, two heads, and 80 cylinders.

| Field | 720K | 1.2 MB | 1.44 MB |
|---|---:|---:|---:|
| Payload sectors | 1,440 | 2,400 | 2,880 |
| Payload bytes | `0x0B4000` | `0x12C000` | `0x168000` |
| Payload KiB | 720 | 1,200 | 1,440 |
| Sectors/cluster | 2 | 2 | 1 |
| Sectors/FAT | 3 | 7 | 9 |
| Root entries | 112 | 224 | 224 |
| Root sectors | 7 | 14 | 14 |
| Media descriptor | `0xF9` | `0xF9` | `0xF0` |
| Sectors/track | 9 | 15 | 18 |
| First data sector | 14 | 29 | 33 |
| Complete allocatable clusters | 713 | 1,185 | 2,847 |
| Allocatable capacity | 713 KiB | 1,185 KiB | 1,423.5 KiB |
| Physical slot stride | `0x180000` | `0x180000` | `0x180000` |
| Gap after FAT12 payload | 816 KiB | 336 KiB | 96 KiB |

**1.2 MB nuance:** Its data region contains 2,371 sectors, but clusters are two sectors each. Only `floor(2371 / 2) = 1185` clusters are allocatable; the final 512-byte data-region sector is outside a complete cluster. Do not confuse total data-region bytes with usable file capacity.

**Capacity labels:** The ipcas GUI showed **713.0 KB** for 720K, exactly matching its 713 KiB allocatable space. The 1.2 MB GUI capacity was **not captured**; 1185.0 KB is a prediction, not a measured GUI result.

## 4. Slot contents (observed)

- FAT12 boot sector: OEM `MSDOS5.0`, jump bytes `EB 3C 90`, extended boot signature `0x29`, filesystem type text `FAT12   `, and end signature `55 AA`.
- Boot-sector volume labels: `FLPPY0`, `FLPPY1`, …, space-padded to 11 bytes; corresponding root-directory volume-label entry (attribute `0x08`).
- FAT 1 and FAT 2 are initially identical, with reserved FAT12 entries initialized using the geometry's media descriptor; all allocatable clusters start free.
- Empty root directory contains the volume-label entry, then zeros.
- On clean full formats, the data area is mostly filled with `0xF6`, **except the final payload sector, which is zero**. For 1.2 MB, that final sector is the incomplete-cluster remainder; for the other profiles it lies inside an allocatable cluster.
- The gap from payload end up to the metadata sector is zero in all zero-baseline captures.
- Boot code is present even with the ipcas **DOS bootable** option unchecked.

**Gotek Core implementation policy:** the captured executable boot code is not
redistributed. Empty-image boot sectors are generated from scratch with the
observed BPB/extended BPB values and an original diagnostic stub that displays
`Non-system disk` and waits indefinitely. See `src/boot/README.md`. FAT12
initialization patterns remain validated against format-data fixtures and optional
private captures. Executable-region bytes `[62,510)` intentionally differ in slot
boot sectors and the boundary copy; no other established fields are excluded
from comparisons. Whether changing that code affects firmware or ipcas remains
untested.

## 5. Per-slot metadata (observed)

Each slot's final sector starts at `slot_start + 0x17FE00` (relative LBA 3071).

| Relative to slot | Content |
|---|---|
| `0x17FE00` | NUL-terminated ASCII `UPDATE TIME:YYYY-MM-DD HH-MM-SS` |
| `0x17FE50` (slot 0 only) | NUL-terminated ASCII `MAX BLOCK:NNN` |

`NNN` is **three-digit zero-padded** slot count: `050` for 50 slots and `100` for 100 slots. The count record appears **once**, in slot 0. No separate geometry flag was identified in any tested capture.

The `UPDATE TIME` value corresponds to ipcas's per-slot Renew time and changes on import. Whether these strings are essential to firmware operation, only used by the Windows GUI, or both is **untested**.

## 6. Boundary sector after the final slot (observed)

At absolute offset `slot_count * 0x180000`, ipcas writes **one extra 512-byte sector**. It is byte-for-byte identical to the **last slot's boot sector** except for its first three bytes:

```text
Last slot boot: EB 3C 90 ... 55 AA
Extra sector:   00 00 00 ... 55 AA
```

The remaining 509 bytes, including BPB, volume label, serial, boot code, and `55 AA`, match. There is **no following FAT12 filesystem**; this is not an additional usable slot.

- 50 slots: extra sector at `0x04B00000` (75 MiB).
- 100 slots: extra sector at `0x09600000` (150 MiB).

For clean zero-baseline captures, everything after this extra sector is zero. Its purpose and necessity remain **unknown**. A compatibility-first writer should reproduce it until controlled tests show it is unnecessary.

## 7. File import behavior (observed)

Comparison of `image02.img` (100 × 1.44 MB, empty) with `image03.img` (same stick after ipcas imported a five-byte file into slot 005) found **58 changed bytes**, all within slot 005:

- Both FAT copies changed to allocate cluster 2 as end-of-chain.
- Root directory gained VFAT long-filename and 8.3 directory entries; volume-label timestamps also changed.
- First five bytes of cluster 2 became `hola!`; remaining bytes retained their previous `0xF6` fill.
- Slot 005's `UPDATE TIME` changed.
- No slot boot sector, other slot, or bank-end marker changed.

The captured filename was **`hello.txt.txt`**, with short alias **`HELLOT~1.TXT`**. This is an observation of that import, not proof that ipcas automatically duplicates `.txt` (Windows XP's hidden extensions may explain the input name).

**Inferred implementation approach:** treat each slot's BPB-defined payload as a standalone FAT12 filesystem. Use a FAT12 implementation for ordinary file operations while ensuring writes cannot extend into the slot's metadata/gap or neighboring slots. Update the slot timestamp in a compatibility-conscious way.

## 8. Capture inventory

Each image is a full-device **2,013,265,920-byte (`0x78000000`)** image from the same USB stick. The clean-format captures were made after zero-filling, with full format, Quick format unchecked, and DOS bootable unchecked.

| Image | Description | SHA-256 |
|---|---|---|
| `image01.img` | 100 × 1.44 MB; earlier capture without fresh zero baseline | `50af7d775705a64d5300fce98708fca2c650ad316a10c2008baaae60adde936e` |
| `image02.img` | 100 × 1.44 MB; clean baseline | `30ab420d2ac7d0783ea1d60b408a8a5a183b991c98db47fd40af1da5eef52d71` |
| `image03.img` | `image02` after five-byte file import into slot 005 | `5288f3e2395021ec77b9a45b09103f4baca5d16fa81769bb1554480bde8ea4b3` |
| `image04.img` | 50 × 1.44 MB; clean baseline | `de4e28930bb475c67be7802997268b076caa4f211159fd9105434f56b02dc3ab` |
| `image05.img` | 100 × 720K; clean baseline | `1785eb9e563b5365fbd3878e895ae45ed386311fad29327aded2930f30def553` |
| `image06.img` | 100 × 1.2 MB; clean baseline | `65d4d03cb127d0e55f5b530ae6ac5b5b88507b839c1484fef5d702b421464c7b` |

Associated forensic reports: `GOTEK_IMAGE_ANALYSIS.md`, `GOTEK_IMAGE_COMPARISON.md`, `GOTEK_FILE_IMPORT_ANALYSIS.md`, `GOTEK_50_SLOT_ANALYSIS.md`, `GOTEK_720K_ANALYSIS.md`, `GOTEK_1200K_ANALYSIS.md`.

**Keep these reports and images as test evidence.** The SHA-256 values above identify the exact captures; timestamps and volume serials will naturally differ on newly formatted drives.

## 9. Unresolved questions / compatibility risks

1. **Firmware behavior:** Which structures does original Gotek firmware actually inspect? The captures alone cannot prove that `MAX BLOCK`, `UPDATE TIME`, or the extra sector are required.
2. **Bidirectional interoperability:** Will ipcas reopen and modify a Rust-formatted stick, and will the Rust reader handle subsequent ipcas changes?
3. **Hardware validation:** Will physical original-firmware Gotek devices recognize and use Rust-created slots in all three geometries?
4. **Quirks:** ipcas root-label creation-time bytes appear misplaced; volume serials can repeat. Neither should be treated as a firmware requirement without tests.
5. **Format options:** Quick format, DOS bootable, nonzero preexisting media, mixed slot geometries, maximum supported count, and other ipcas/firmware versions remain untested.
6. **File operations:** More cases are needed: directories, long names, overwrites, deletions, multi-cluster files, full disks, and different floppy sizes.
7. **Zeroing semantics:** Zero-baseline images prove the resulting bytes, not whether ipcas explicitly wrote every zero sector or in what order.

## 10. Implementation roadmap

### Phase 1 — `gotek-core` (Rust library)

- Define `FloppyGeometry` for 720K / 1.2 MB / 1.44 MB and a fixed-stride `SlotLayout`.
- Implement **read-only inspection first**: recognize candidate media, validate BPBs/FATs/labels, read slot count and timestamps, enumerate slots, and report errors safely.
- Implement per-slot FAT12 file listing and extraction using an existing FAT12 crate if suitable; check that it handles these exact BPBs and VFAT names.
- Implement **image-file formatting** for N slots, reproducing boot/FAT/root/data initialization, metadata, and boundary sector. Keep serial/time generation replaceable and deterministic in tests.
- Implement file import, overwrite, deletion, and timestamp updates; enforce strict payload bounds.
- Keep block-device I/O behind an abstraction so tests use regular files and raw-device writes require explicit opt-in.

### Phase 2 — CLI

Example target interface (illustrative, not implemented):

```sh
gotek inspect image.img
gotek list image.img --slot 5
gotek extract image.img --slot 5 --output ./files
gotek import image.img --slot 5 ./hello.txt
gotek format /dev/sdX --floppies 100 --size 1.44m
```

For **destructive raw-device formatting**, require an explicit device selection, clear capacity/identity display, unmounted-device checks, confirmation, and robust error handling. Do not assume `/dev/sdX` remains stable between operations.

### Phase 3 — Interoperability tests

1. **Golden-image parsing:** Validate every slot in images 02, 04, 05, and 06; extract the imported file from image03.
2. **Golden-byte comparisons:** Format temporary image files and compare stable structures byte-for-byte against ipcas references, normalizing only intentionally variable timestamps/serials.
3. **Rust → ipcas:** Format a disposable USB stick with the Rust tool, open it in ipcas, verify slot count/capacity, import a file, and recapture/diff.
4. **ipcas → Rust:** Read ipcas-formatted media, list/export files, modify a slot using Rust, then reopen it in ipcas and verify content.
5. **Physical Gotek:** Test recognition, selection, reading, and writing on the intended original-firmware hardware, across all three geometries.
6. **Negative tests:** Truncated images, corrupt BPBs, invalid count strings, missing extra sector, malformed FATs, and out-of-range slot indices must fail safely.

### Phase 4 — GTK frontend

Build the Linux GUI on top of the tested library rather than duplicating disk logic. Prioritize safe device selection, slot overview, file import/export, and explicit warnings for destructive operations.

## 11. Recommended first milestone

**Create a read-only `gotek-core` inspector with golden-image tests**, then a formatter that writes to regular image files. This proves geometry and bounds handling without risking a physical USB stick. Only move to raw-device writes after the image-based outputs are reproducible and validated.

---

**Bottom line:** For the three observed ipcas V1.40 floppy profiles, the common container rule is **fixed 1.5 MiB slots + BPB-defined FAT12 payloads + final-sector metadata + a count-dependent extra boundary sector**. The next task is implementation and interoperability testing, not more speculative layout reconstruction.
