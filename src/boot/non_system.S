# SPDX-License-Identifier: MIT
# Original Gotek Core diagnostic stub. No filesystem reads or OS loader.
# Assemble with GNU as --32, then objcopy -O binary -j .text (see README.md).
.intel_syntax noprefix
.code16
.section .text
.global _start
.set BOOT_LOAD_ADDRESS, 0x7c00
.set BOOT_CODE_OFFSET, 0x3e
_start:
    cli
    xor ax, ax
    mov ss, ax
    mov sp, BOOT_LOAD_ADDRESS
    mov ds, ax
    cld
    sti
    # DS is zero, so use a physical address irrespective of the BIOS's CS:IP.
    mov si, offset BOOT_LOAD_ADDRESS + BOOT_CODE_OFFSET + (message - _start)
    mov bx, 0x0007               # BIOS video page zero, text attribute seven.
print_character:
    lodsb
    test al, al
    jz stop
    mov ah, 0x0e                 # BIOS teletype output only; no disk services.
    int 0x10
    jmp print_character
stop:
    cli
wait_forever:
    hlt
    jmp wait_forever
message:
    .asciz "Non-system disk"
