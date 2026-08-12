# Hxv4 XP3 format — reverse-engineering notes (Motto Haramase, KiriKiriZ)

Running log of the reverse-engineering of the **Hxv4** custom XP3 variant used by
`scn.xp3` in *Motto Haramase Honoo no Oppai Isekai Oppai Bunny Gakuen*
(engine: **KiriKiriZ**, scripts: `.scn` = PSB). Goal: extract the `.scn` statically
so we can translate the whole game without a full playthrough.

## Context / where Hxv4 shows up
- Engine: KiriKiriZ. Exe `hbunny.exe` (x86, "KIRIKIRI core" copyright), imports
  `version.dll` + `winmm.dll` (so our proxy DLL attaches — see VN_RPG_TRANSLATION.md §3).
- Archives: `data.xp3`, `image.xp3`, `voice*.xp3` are normal; **`scn.xp3`** (the scripts)
  uses the Hxv4 variant. Standard tools (GARbro, krkr-xp3, our `make_xp3`/`xp3read.py`)
  read **0 entries** from it.
- The engine at runtime registers a `psb` storage media that reads `.scn` from `scn.xp3`
  on demand. Our proxy's `extract-unencrypted.txt` hook dumps those decrypted `.scn`
  (clean uncompressed `PSB\0`) as they're played — but only scenes actually reached.

## Header layout (confirmed)
```
off 0   : "XP3\r\n \n\x1a\x8bg\x01"        (11-byte XP3 magic)
off 11  : QWORD = 0x17                       (v2 marker: "index-info" is at an extended header)
off 19  : DWORD = 1                           (?)
off 23  : QWORD = 0x80                         (index-info size? / flags)
off 31  : BYTE  = 0
off 32  : QWORD = real index offset            (e.g. 0x9212d76, near EOF)
```
Read the real index at `QWORD@32`. Index framing is standard: `BYTE flag` then, if
`flag&1`, `QWORD compSize, QWORD origSize, zlib data`.

## Index structure (confirmed)
Decompressed index (~27 KB for 257 entries) is a sequence of `tag(4) + QWORD size + body`:
```
@0    "Hxv4"  size=14   body = a8 ff 20 09 00 00 00 00 ce 2d 00 00 00 00   <-- header/KEY (see below)
@26   "File"  size=94   } standard File chunks, 106 bytes apart
@132  "File"  size=94   }
...                       257 File chunks total
```
Each `File` body has standard sub-chunks (`tag(4)+QWORD size+data`):
- `adlr` (4B): adler32 of the DECRYPTED file content (integrity hash / lookup key).
- `segm` (28B): `flags(4), offset(8), origSize(8), arcSize(8)`. flags=0 seen (=uncompressed
  framing), but the bytes at `offset` are still **CxDec-encrypted** (see below).
- `info` (26B): `enc(4), origSize(8), arcSize(8), nameLen(2), name(utf16)`.
  **Names are STRIPPED**: `nameLen=1`, name = one junk char (e.g. 0x5000). Classic
  KiriKiri name-stripping protection — the engine looks files up by `adlr` hash, not name.
  `enc` field reads 0 (misleading — the DATA is still encrypted).

## The wall (confirmed)
Extracting each entry by its `segm` offset/size yields **random magics** (`c1 f8 71 ab`,
`d3 3b 9c 65`, …), NOT `PSB\0`. So the file bodies are **CxDec-encrypted**; only the
game's engine (with the key) decrypts them → the runtime dump is clean, static is not.
The `Hxv4` header body `a8 ff 20 09 00 00 00 00 ce 2d 00 00 00 00` is the prime suspect
for the key/scheme selector.

## Known-plaintext result — encryption structure CRACKED (one file)
We have one clean plaintext (`sc_0_pr00.txt.scn`, dumped via the psb media, 2,119,705 B).
`adler32(plaintext) = 0xaae0444f` **matched entry #5's `adlr` exactly** → so `adlr` IS
`adler32` of the decrypted content. That gives us a **verification oracle**: any candidate
decryption is correct iff its adler32 equals the stored `adlr`. No guessing needed to know
when we're right.

`keystream = cipher XOR plaintext` for entry #5 has a clean 3-region structure:
```
[0   : 16 ]  16-byte per-file header key = aa 6c 0c 16 aa b2 85 7c 8d 2b 26 a3 85 d5 51 e2
[16  : 324]  constant 0x55   (308 bytes)
[324 : EOF]  constant 0xe2   (rest of file)
```
So the cipher is a **two-constant XOR** (region A = 0x55, region B = 0xe2) plus a 16-byte
header key, with a split at 324. That is the classic CxDec "simple" shape.

## The remaining wall: per-file key derivation
Applying entry #5's constants (0x55 / 0xe2, split 324) to OTHER entries and checking
adler32 does NOT validate → **the two constants, the split, and the header key are all
per-file, derived from the file's hash** (real CxDec, hash-seeded). Cracking the whole
archive statically therefore needs the derivation function `f(hash) -> (keyA, keyB, split,
headerKey)`. Two ways to get it:
1. **Empirical**: dump N more scenes (play ~10 min), giving N (adlr, keyA, keyB, split,
   header) tuples; reverse how they relate to the hash. Feasible with the adler oracle.
2. **Static RE**: disassemble `hbunny.exe`'s CxDec routine (IDA/Ghidra) — deepest, surest.

## Status
- [x] Parse Hxv4 index (v2 offset@32, Hxv4 header chunk, standard File chunks).
- [x] Confirm names stripped; `adlr` = adler32(plaintext) → verification oracle.
- [x] Crack the cipher STRUCTURE from one known-plaintext pair (2 XOR constants + header).
- [ ] Derive the per-file key function (needs more pairs or exe RE) — NOT done; deep.
- Decision: the per-file key derivation is a large binary-RE task with low marginal value
  because the runtime dump already yields clean PSB. Left here for future reference.

## Practical fallback (works today)
Runtime dump via the proxy DLL + `extract-unencrypted.txt`: launch, play through the
scenes you want translated, they dump as clean PSB → `VNTextPatch extractlocal` →
translate → `VNTextPatch insertlocal` → serve via `unencrypted/`. Complete but needs
playing each scene.
