"""Pack a directory tree into an UNENCRYPTED XP3 archive (KiriKiri v1 layout).
KiriKiri loads patchN.xp3 over data.xp3, and reads unencrypted XP3 fine, so a
translation patch needs no re-encryption. Uncompressed segments for simplicity.
Usage: make_xp3.py <tree_dir> <out.xp3>
"""
import os, sys, struct, zlib, io

tree, out = sys.argv[1], sys.argv[2]
# KirikiriTools' version.dll reads a patch whose adler32 hashes are all ZERO
# as an unencrypted-override marker, bypassing the game's custom/encrypted
# archive format. Pass --zero-hash to produce that marker patch.
ZERO_HASH = "--zero-hash" in sys.argv[3:]

files = []
for root, _, names in os.walk(tree):
    for n in names:
        full = os.path.join(root, n)
        rel = os.path.relpath(full, tree).replace("\\", "/")
        files.append((rel, full))
files.sort()

MAGIC = bytes([0x58,0x50,0x33,0x0D,0x0A,0x20,0x0A,0x1A,0x8B,0x67,0x01])

body = io.BytesIO()
body.write(MAGIC)
body.write(b"\x00" * 8)          # placeholder for index offset

file_records = []
for rel, full in files:
    data = open(full, "rb").read()
    offset = body.tell()
    body.write(data)
    adler = 0 if ZERO_HASH else (zlib.adler32(data) & 0xffffffff)
    file_records.append((rel, offset, len(data), adler))

index_offset = body.tell()

# build raw index
idx = io.BytesIO()
for rel, offset, size, adler in file_records:
    name = rel.encode("utf-16-le")
    name_len = len(rel)               # chars
    info = struct.pack("<IQQ", 0, size, size) + struct.pack("<H", name_len) + name
    info_chunk = b"info" + struct.pack("<Q", len(info)) + info
    segm = struct.pack("<IQQQ", 0, offset, size, size)   # flags=0 (uncompressed)
    segm_chunk = b"segm" + struct.pack("<Q", len(segm)) + segm
    adlr = struct.pack("<I", adler)
    adlr_chunk = b"adlr" + struct.pack("<Q", len(adlr)) + adlr
    filebody = info_chunk + segm_chunk + adlr_chunk
    idx.write(b"File" + struct.pack("<Q", len(filebody)) + filebody)

idx_raw = idx.getvalue()
idx_comp = zlib.compress(idx_raw, 9)                  # KiriKiri stores index zlib-compressed
body.write(b"\x01")                                   # index compressed
body.write(struct.pack("<Q", len(idx_comp)))          # compressed size
body.write(struct.pack("<Q", len(idx_raw)))           # uncompressed size
body.write(idx_comp)

# patch index offset
buf = bytearray(body.getvalue())
struct.pack_into("<Q", buf, 11, index_offset)
open(out, "wb").write(buf)
print(f"wrote {out}: {len(file_records)} files, {len(buf)} bytes, index@{index_offset}")
