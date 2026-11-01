# Original non-system boot stub

Gotek Core generates the entire FAT12 boot sector from zeroed memory. The header
and BPB follow the documented geometry profiles; OEM text `MSDOS5.0` is retained
as a format identifier only. No Microsoft DOS or ipcas executable code is used.

`non_system.S` is original MIT-licensed 16-bit x86 assembly. Its instructions are
also listed as a small, commented Rust byte array in `src/format.rs`, so building
or using the library requires neither an assembler nor any binary template.

The BIOS loads the boot sector at physical address `0x7c00`. The sector starts
with `EB 3C 90`: its short jump lands at offset 62, after the FAT12 extended BPB.
The 33-byte stub starts there and its NUL-terminated message starts at offset 95
(physical `0x7c5f`). A zero data segment makes the message address independent of
whether the BIOS entered at `0000:7c00` or `07c0:0000`. Interrupts are disabled
while setting up a zero-segment stack below the boot sector, then reenabled.
The direction flag is cleared before iterating over the message.

The stub uses BIOS interrupt `10h`, service `0Eh`, to display `Non-system disk`
on video page zero. It then disables interrupts and waits in an `HLT` loop.
It never invokes disk services, reads FAT data, or loads an operating system.
This is an empty, non-system floppy, not bootable DOS media. The unused bytes
through offset 509 remain zero; the final two bytes are `55 AA`.

To reproduce the stub bytes with GNU binutils, from the repository root:

```sh
as --32 -o /tmp/gotek-non-system.o src/boot/non_system.S
objcopy -O binary -j .text /tmp/gotek-non-system.o /tmp/gotek-non-system.bin
od -An -tx1 -v /tmp/gotek-non-system.bin
objdump -D -b binary -m i8086 --adjust-vma=0x7c3e /tmp/gotek-non-system.bin
```

The flat output has 49 bytes: 33 instruction bytes and the 16-byte terminated
message. Compare its first 33 bytes with `NON_SYSTEM_BOOT_CODE` and its final
16 with `NON_SYSTEM_MESSAGE` in `src/format.rs`. The assembler computes the
message address and branch displacements. If the instructions or their placement
change, regenerate those values together. The assembly object has no relocations;
`objdump -r /tmp/gotek-non-system.o` can verify that before extracting its bytes.

The observed root-label timestamp quirk is still preserved separately: the time
word is written at bytes 13–14 and 22–23; byte 15 remains zero. Creation, access,
and last-write dates occupy 16–17, 18–19, and 24–25. These observations describe
format data and are not executable boot code or claims about firmware needs.

Original Gotek hardware and ipcas interoperability remain unverified. In
particular, it is not established whether either checks boot-code bytes.
