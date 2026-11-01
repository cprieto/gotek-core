# Format-data-only ipcas fixtures

These small fixtures contain only observed filesystem format data from slot zero
of the clean captures identified in `GOTEK_FORMAT_SPEC.md`. No captured executable
boot-code region is retained or distributed.

| Prefix | Private capture | BPB/header range | Root-label range |
| --- | --- | --- | --- |
| `ipcas-1440` | image02.img | 0..62 | 0x2600..0x2620 |
| `ipcas-1440-50` | image04.img | 0..62 | 0x2600..0x2620 |
| `ipcas-720` | image05.img | 0..62 | 0x0e00..0x0e20 |
| `ipcas-1200` | image06.img | 0..62 | 0x1e00..0x1e20 |

Each `*-bpb.bin` is exactly 62 bytes: the permitted three-byte jump pattern,
eight-byte OEM identifier, BPB, and extended BPB. It stops before the boot-code
region. Each `*-root-label.bin` is exactly 32 bytes of directory-entry data.
No fixture is used to generate boot sectors. Tests compare every header byte,
root-label byte, and the independently specified `55 AA` signature.
The root-label timestamp quirk is described in `src/boot/README.md`.

SHA-256:

```text
1e6bbe23009a86054d81a0d49c9d9ccc2ac6e286cf1e4cbc3baf1e4902981e46  ipcas-1200-bpb.bin
7a27f445bc96bee21f9c6a1d1239bbcb293a8a347ca0e8fa0d8887172e86d470  ipcas-1200-root-label.bin
ad65929df16ab99586bb2f59d7beae1bf560d0d8052436a01fdb2c67cd25c339  ipcas-1440-50-bpb.bin
14d390c325645eefd5b745240b588f62362a2b108805f5396961867c0d7a5118  ipcas-1440-50-root-label.bin
927699a549b6f7c7325596b5d7ff38ba9434ca50d742344d721ab7d7b13cec25  ipcas-1440-bpb.bin
9617a45f4ced2c3342f3d44f2b9e4c232aa8d11284fb8ab25c6843d5e94e6905  ipcas-1440-root-label.bin
2f72fcc3acd6f790f154ce8a0f818ef16a2d9ae311a60525b417090c6946517e  ipcas-720-bpb.bin
a0d8eee8f6a3f97b9d3ec4746d40161df16943a837a98beb98ecab98e64d4727  ipcas-720-root-label.bin
```

Optional full-capture tests regenerate the formatted region through the boundary
sector with the capture's explicit serial/time values. They compare every byte
except `[62,510)` of each stride-aligned slot boot sector and the boundary sector.
Only executable boot code and its embedded data/padding differ intentionally.
Headers, signatures, FATs, root directories, data fill, gaps, and metadata remain
strict comparisons. Device-capacity padding after the boundary remains outside
the minimal new image. Missing private captures are reported on stderr; run
`cargo test -- --nocapture` to see skip diagnostics on successful test runs.

The captures and research reports with executable hex dumps are private local
evidence, ignored by Git and excluded by the Cargo package allowlist. The MIT
license covers Gotek Core's original code; it does not relicense third-party
material or grant rights to separately held research captures.
