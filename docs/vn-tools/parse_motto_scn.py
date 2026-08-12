"""Parse a KiriKiri XP3 v2 (extended header, QWORD@11==0x17) archive that GARbro/
krkr-xp3/arc_unpacker choke on, and extract its entries. Handles zlib-compressed
index and segments. Reports whether extracted .scn are PSB (magic) for FreeMote."""
import sys, struct, zlib, os

path = sys.argv[1]
outdir = sys.argv[2]
os.makedirs(outdir, exist_ok=True)
b = open(path, "rb").read()

q11 = struct.unpack_from("<Q", b, 11)[0]
if q11 == 0x17:
    index_offset = struct.unpack_from("<Q", b, 32)[0]   # v2: real index offset
else:
    index_offset = q11
print(f"v2={q11==0x17} index_offset={index_offset} (0x{index_offset:x}) filesize={len(b)}")

flag = b[index_offset]
if flag == 1:
    comp = struct.unpack_from("<Q", b, index_offset + 1)[0]
    orig = struct.unpack_from("<Q", b, index_offset + 9)[0]
    raw = zlib.decompress(b[index_offset + 17: index_offset + 17 + comp])
else:
    size = struct.unpack_from("<Q", b, index_offset + 1)[0]
    raw = b[index_offset + 9: index_offset + 9 + size]
print(f"index flag={flag} raw_index={len(raw)} bytes")

def sub_chunks(body):
    q = 0
    while q + 12 <= len(body):
        tag = body[q:q+4]
        sz = struct.unpack_from("<Q", body, q+4)[0]
        yield tag, body[q+12:q+12+sz]
        q += 12 + sz

p = 0; entries = []
while p + 12 <= len(raw):
    tag = raw[p:p+4]
    sz = struct.unpack_from("<Q", raw, p+4)[0]
    body = raw[p+12:p+12+sz]
    p += 12 + sz
    if tag != b"File":
        continue
    info = None; segms = []; enc = 0
    for st, sb in sub_chunks(body):
        if st == b"info":
            enc = struct.unpack_from("<I", sb, 0)[0]
            nlen = struct.unpack_from("<H", sb, 20)[0]
            name = sb[22:22+nlen*2].decode("utf-16-le", "replace")
            info = name
        elif st == b"segm":
            k = 0
            while k + 28 <= len(sb):
                sflags, off, osz, csz = struct.unpack_from("<IQQQ", sb, k)
                segms.append((sflags, off, osz, csz)); k += 28
    if info is not None:
        entries.append((info, enc, segms))

print(f"entries: {len(entries)}")
enc_count = sum(1 for _, e, _ in entries if e != 0)
print(f"entries with encryption flag != 0: {enc_count}")
exts = {}
for name, _, _ in entries:
    e = name.rsplit(".", 1)[-1].lower() if "." in name else "?"
    exts[e] = exts.get(e, 0) + 1
print("extensions:", exts)

# extract entries, report first .scn magic
def extract(segms):
    out = bytearray()
    for sflags, off, osz, csz in segms:
        chunk = b[off:off+csz]
        if sflags & 1:
            chunk = zlib.decompress(chunk)
        out += chunk
    return bytes(out)

first_scn = None; saved = 0
for name, enc, segms in entries:
    if not name.lower().endswith((".scn", ".ks", ".txt", ".tjs")):
        continue
    try:
        data = extract(segms)
    except Exception as ex:
        continue
    flat = name.replace("/", "_").replace("\\", "_")
    open(os.path.join(outdir, flat), "wb").write(data)
    saved += 1
    if first_scn is None and name.lower().endswith(".scn"):
        first_scn = (name, data[:16])
print(f"saved text/scn entries: {saved}")
if first_scn:
    name, head = first_scn
    print(f"first .scn: {name} head={head.hex(' ')}")
    print(f"  PSB? {head[:3]==b'PSB'}  mdf(compressed PSB)? {head[:3]==b'mdf'}")
