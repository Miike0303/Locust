# Minimal reader/writer for Unreal Engine 4 pak version 11, uncompressed and
# unencrypted. Format notes in engram: locust/unreal-pak-locres-format
#
# The writer produces a PATCH pak (name it <Game>-<Platform>_P.pak) carrying only
# the files you want to override. Never rewrite the shipped multi-gigabyte pak.

import hashlib
import struct

MAGIC = 0x5A6F12E1
VERSION = 11
COMP_NAME_SLOTS = 5
COMP_NAME_LEN = 32
FOOTER_LEN = 16 + 1 + 4 + 4 + 8 + 8 + 20 + COMP_NAME_SLOTS * COMP_NAME_LEN
ENTRY_HEADER_LEN = 8 + 8 + 8 + 4 + 20 + 1 + 4


def _fstr(s):
    if s == "":
        return struct.pack('<i', 0)
    try:
        raw = s.encode('ascii') + b'\0'
        return struct.pack('<i', len(raw)) + raw
    except UnicodeEncodeError:
        raw = (s + '\0').encode('utf-16-le')
        return struct.pack('<i', -(len(s) + 1)) + raw


class _R:
    def __init__(self, b, o=0):
        self.b, self.o = b, o

    def u32(self):
        v = struct.unpack_from('<I', self.b, self.o)[0]; self.o += 4; return v

    def i32(self):
        v = struct.unpack_from('<i', self.b, self.o)[0]; self.o += 4; return v

    def u64(self):
        v = struct.unpack_from('<Q', self.b, self.o)[0]; self.o += 8; return v

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


def _decode_entry(buf, at):
    r = _R(buf, at)
    val = r.u32()
    cm = (val >> 23) & 0x3F
    offset = r.u32() if val & (1 << 31) else r.u64()
    usize = r.u32() if val & (1 << 30) else r.u64()
    if cm != 0:
        size = r.u32() if val & (1 << 29) else r.u64()
    else:
        size = usize
    return offset, size, cm, bool(val & (1 << 22))


def read_index(path):
    """Return (mount_point, {archive_path: (data_offset, size)})."""
    with open(path, 'rb') as f:
        f.seek(0, 2)
        total = f.tell()
        f.seek(total - FOOTER_LEN)
        foot = f.read(FOOTER_LEN)
        o = 16
        enc_index = foot[o]; o += 1
        magic, version = struct.unpack_from('<II', foot, o); o += 8
        if magic != MAGIC:
            raise ValueError("bad pak magic 0x{:08X}".format(magic))
        idx_off, idx_size = struct.unpack_from('<qq', foot, o)
        if enc_index:
            raise ValueError("encrypted index is not supported")
        f.seek(idx_off)
        idx = f.read(idx_size)

    r = _R(idx)
    mount = r.fstr()
    r.i32()                      # entry count
    r.u64()                      # path hash seed
    if r.i32():                  # has path hash index
        r.i64(); r.i64(); r.o += 20
    has_fdi = r.i32()
    if not has_fdi:
        raise ValueError("pak has no full directory index")
    fdi_off, fdi_size = r.i64(), r.i64()
    r.o += 20
    encoded = idx[r.o + 4:r.o + 4 + struct.unpack_from('<i', idx, r.o)[0]]

    with open(path, 'rb') as f:
        f.seek(fdi_off)
        fdi = f.read(fdi_size)
    d = _R(fdi)
    files = {}
    for _ in range(d.i32()):
        directory = d.fstr()
        for _ in range(d.i32()):
            name = d.fstr()
            at = d.u32()
            off, size, cm, enc = _decode_entry(encoded, at)
            if cm != 0 or enc:
                continue          # this reader only handles stored entries
            files[directory + name] = (off + ENTRY_HEADER_LEN, size)
    return mount, files


def extract(path, archive_path, out_path):
    _, files = read_index(path)
    off, size = files[archive_path]
    with open(path, 'rb') as f:
        f.seek(off)
        data = f.read(size)
    open(out_path, 'wb').write(data)
    return len(data)


def write_pak(out_path, entries, mount_point="../../../"):
    """entries: {archive_path: bytes}. Paths use forward slashes and are relative
    to mount_point. Everything is stored uncompressed and unencrypted."""
    blob = bytearray()
    placed = {}
    for apath, data in entries.items():
        offset = len(blob)
        blob += struct.pack('<q', 0)                 # Offset, rewritten by the engine
        blob += struct.pack('<q', len(data))         # Size
        blob += struct.pack('<q', len(data))         # UncompressedSize
        blob += struct.pack('<I', 0)                 # CompressionMethodIndex
        blob += hashlib.sha1(data).digest()          # Hash[20]
        blob += bytes([0])                           # bEncrypted
        blob += struct.pack('<I', 0)                 # CompressionBlockSize
        blob += data
        placed[apath] = (offset, len(data))

    # Encoded entries, in a stable order, remembering each one's offset.
    encoded = bytearray()
    enc_at = {}
    for apath, (offset, size) in placed.items():
        enc_at[apath] = len(encoded)
        flags = 0
        if offset <= 0xFFFFFFFF:
            flags |= 1 << 31
        if size <= 0xFFFFFFFF:
            flags |= 1 << 30
        encoded += struct.pack('<I', flags)
        encoded += struct.pack('<I', offset) if flags & (1 << 31) else struct.pack('<Q', offset)
        encoded += struct.pack('<I', size) if flags & (1 << 30) else struct.pack('<Q', size)

    # Full directory index, grouped by directory. Directory strings carry a
    # TRAILING slash and no leading one — that is how the game's own shipped pak
    # stores them, verified by reading it back.
    dirs = {}
    for apath in placed:
        d, _, name = apath.rpartition('/')
        dirs.setdefault(d + '/' if d else '', {})[name] = enc_at[apath]
    fdi = bytearray()
    fdi += struct.pack('<i', len(dirs))
    for d, names in dirs.items():
        fdi += _fstr(d)
        fdi += struct.pack('<i', len(names))
        for name, at in names.items():
            fdi += _fstr(name)
            fdi += struct.pack('<I', at)

    data_len = len(blob)
    fdi_off = data_len
    primary_off = fdi_off + len(fdi)

    primary = bytearray()
    primary += _fstr(mount_point)
    primary += struct.pack('<i', len(placed))
    primary += struct.pack('<Q', 0)                  # PathHashSeed
    primary += struct.pack('<i', 0)                  # bHasPathHashIndex = false
    primary += struct.pack('<i', 1)                  # bHasFullDirectoryIndex = true
    primary += struct.pack('<q', fdi_off)
    primary += struct.pack('<q', len(fdi))
    primary += hashlib.sha1(bytes(fdi)).digest()
    primary += struct.pack('<i', len(encoded))
    primary += bytes(encoded)
    primary += struct.pack('<i', 0)                  # NumFiles in the non-encoded list

    footer = bytearray()
    footer += bytes(16)                              # EncryptionKeyGuid
    footer += bytes([0])                             # bEncryptedIndex
    footer += struct.pack('<II', MAGIC, VERSION)
    footer += struct.pack('<q', primary_off)
    footer += struct.pack('<q', len(primary))
    footer += hashlib.sha1(bytes(primary)).digest()
    footer += bytes(COMP_NAME_SLOTS * COMP_NAME_LEN)  # no compression methods

    with open(out_path, 'wb') as f:
        f.write(bytes(blob)); f.write(bytes(fdi)); f.write(bytes(primary)); f.write(bytes(footer))
    return data_len + len(fdi) + len(primary) + len(footer)
