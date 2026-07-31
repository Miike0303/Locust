# Reader and writer for Unreal Engine .locres localization files (version 3,
# Optimized_CityHash64_UTF16). Format notes in engram: locust/unreal-pak-locres-format
#
# The writer exists so an existing translation can be corrected and shipped back.
# Round-trip it against the untouched original before trusting it: a wrong writer
# does not fail loudly, it ships a game that will not boot.

import struct

MAGIC = bytes([0x0E, 0x14, 0x74, 0x75, 0x67, 0x4A, 0x03, 0xFC,
               0x4A, 0x15, 0x90, 0x9D, 0xC3, 0x37, 0x7F, 0x1B])


# ── CityHash64 (UE4 uses it for the key/namespace hashes in v3) ──────────────
# Only needed to WRITE new keys. Existing keys keep the hash we read.
K0 = 0xc3a5c85c97cb3127
K1 = 0xb492b66fbe98f273
K2 = 0x9ae16a3b2f90404f
M64 = (1 << 64) - 1


def _rot(v, s):
    return ((v >> s) | (v << (64 - s))) & M64 if s else v


def _shiftmix(v):
    return (v ^ (v >> 47)) & M64


def _hash128to64(lo, hi):
    mul = 0x9ddfea08eb382d69
    a = ((lo ^ hi) * mul) & M64
    a ^= a >> 47
    b = ((hi ^ a) * mul) & M64
    b ^= b >> 47
    return (b * mul) & M64


def _fetch64(b, i):
    return struct.unpack_from('<Q', b, i)[0]


def _fetch32(b, i):
    return struct.unpack_from('<I', b, i)[0]


def city_hash64(b):
    n = len(b)
    if n <= 32:
        if n <= 16:
            if n >= 8:
                mul = (K2 + n * 2) & M64
                a = (_fetch64(b, 0) + K2) & M64
                c = (_rot(_fetch64(b, n - 8), 37) * mul) & M64
                d = (_rot(a, 25) + _fetch64(b, n - 8)) & M64
                return _hash128to64((c + d) & M64, (c ^ d) & M64) if False else _hash_len_9_16(b, n)
            if n >= 4:
                mul = (K2 + n * 2) & M64
                a = _fetch32(b, 0)
                return _hash128to64((n + (a << 3)) & M64, _fetch32(b, n - 4))
            if n > 0:
                a, y, z = b[0], b[n >> 1], b[n - 1]
                y32 = (a + (y << 8)) & M64
                z32 = (n + (z << 2)) & M64
                return (_shiftmix((y32 * K2) ^ (z32 * K0)) * K2) & M64
            return K2
        return _hash_len_17_32(b, n)
    raise NotImplementedError("only strings up to 32 bytes are hashed here")


def _hash_len_9_16(b, n):
    mul = (K2 + n * 2) & M64
    a = (_fetch64(b, 0) + K2) & M64
    c = (_rot(_fetch64(b, n - 8), 37) * mul) & M64
    d = (_rot(a, 25) + _fetch64(b, n - 8)) & M64
    return _hash128to64((a + _rot(d, 20)) & M64, (c + d) & M64)


def _hash_len_17_32(b, n):
    mul = (K2 + n * 2) & M64
    a = (_fetch64(b, 0) * K1) & M64
    bb = _fetch64(b, 8)
    c = (_fetch64(b, n - 8) * mul) & M64
    d = (_fetch64(b, n - 16) * K2) & M64
    return _hash128to64(
        (_rot((a + bb) & M64, 43) + _rot(c, 30) + d) & M64,
        (a + _rot((bb + K2) & M64, 18) + c) & M64,
    )


# ── FString ─────────────────────────────────────────────────────────────────
# Length INCLUDES the null terminator. Positive = UTF-8, negative = UTF-16LE
# with the count in code units.

class Reader:
    def __init__(self, b):
        self.b, self.o = b, 0

    def u8(self):
        v = self.b[self.o]; self.o += 1; return v

    def u32(self):
        v = struct.unpack_from('<I', self.b, self.o)[0]; self.o += 4; return v

    def i32(self):
        v = struct.unpack_from('<i', self.b, self.o)[0]; self.o += 4; return v

    def i64(self):
        v = struct.unpack_from('<q', self.b, self.o)[0]; self.o += 8; return v

    def fstr(self):
        n = self.i32()
        if n == 0:
            return ""
        if n > 0:
            raw = self.b[self.o:self.o + n]; self.o += n
            return raw.split(b'\0')[0].decode('utf-8', 'replace')
        n = -n
        raw = self.b[self.o:self.o + n * 2]; self.o += n * 2
        return raw.decode('utf-16-le', 'replace').split('\0')[0]


def w_fstr(out, s):
    if s == "":
        out += struct.pack('<i', 0)
        return out
    try:
        raw = s.encode('ascii') + b'\0'
        out += struct.pack('<i', len(raw)) + raw
    except UnicodeEncodeError:
        raw = (s + '\0').encode('utf-16-le')
        out += struct.pack('<i', -(len(s) + 1)) + raw
    return out


class LocRes:
    """namespaces: list of (ns_hash, ns_name, [(key_hash, key, src_hash, text)])"""

    def __init__(self, version, namespaces):
        self.version = version
        self.namespaces = namespaces

    def entries(self):
        for _, ns, keys in self.namespaces:
            for kh, k, sh, text in keys:
                yield ns, k, kh, sh, text

    def count(self):
        return sum(len(k) for _, _, k in self.namespaces)


def load(path):
    b = open(path, 'rb').read()
    assert b[:16] == MAGIC, "not a .locres file"
    r = Reader(b); r.o = 16
    version = r.u8()
    assert version >= 3, f"only v3+ supported here, got v{version}"
    arr_off = r.i64()
    r.u32()  # declared entry count, recomputed on write
    namespaces = []
    for _ in range(r.u32()):
        nh = r.u32(); ns = r.fstr()
        keys = []
        for _ in range(r.u32()):
            kh = r.u32(); k = r.fstr(); sh = r.u32(); keys.append([kh, k, sh, r.i32()])
        namespaces.append((nh, ns, keys))
    r.o = arr_off
    strings = []
    for _ in range(r.u32()):
        s = r.fstr(); r.i32(); strings.append(s)
    # resolve indices to text
    for _, _, keys in namespaces:
        for e in keys:
            e[3] = strings[e[3]] if 0 <= e[3] < len(strings) else ""
    return LocRes(version, namespaces)


def save(lr, path):
    # Deduplicate text into the string array, counting references, exactly as the
    # format expects — the game reads text through these indices.
    order, refs = [], {}
    for _, _, _, _, text in lr.entries():
        if text not in refs:
            refs[text] = 0
            order.append(text)
        refs[text] += 1
    index_of = {s: i for i, s in enumerate(order)}

    body = bytearray()
    body += struct.pack('<I', len(lr.namespaces))
    for nh, ns, keys in lr.namespaces:
        body += struct.pack('<I', nh)
        body = w_fstr(body, ns)
        body += struct.pack('<I', len(keys))
        for kh, k, sh, text in keys:
            body += struct.pack('<I', kh)
            body = w_fstr(body, k)
            body += struct.pack('<I', sh)
            body += struct.pack('<i', index_of[text])

    arr = bytearray()
    arr += struct.pack('<I', len(order))
    for s in order:
        arr = w_fstr(arr, s)
        arr += struct.pack('<i', refs[s])

    head_len = 16 + 1 + 8 + 4          # magic, version, array offset, entry count
    out = bytearray()
    out += MAGIC
    out += bytes([lr.version])
    out += struct.pack('<q', head_len + len(body))
    out += struct.pack('<I', lr.count())
    out += body
    out += arr
    open(path, 'wb').write(bytes(out))
    return len(out)
