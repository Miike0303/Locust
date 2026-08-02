# Build an RPA-3.0 archive whose index uses the SAME pickle opcodes a real Ren'Py
# archive emits, then prove it round-trips through Python's own pickle.
#
# Why this exists: the Rust fixture hand-builds an index from SHORT_BINUNICODE +
# TUPLE2 + SETITEM with key 0 — a shape no shipped game produces — so the parser
# passed its tests while failing on every real archive. See engram
# locust/renpy-rpa-index-opcodes.
#
# Real shape (measured on Area69's scripts.rpa, 271 members):
#   PROTO 2 · EMPTY_DICT · BINPUT · MARK
#   per entry: BINUNICODE(name) · BINPUT · EMPTY_LIST · BINPUT
#              LONG1(offset^key) · BININT(len^key) · SHORT_BINSTRING('') · TUPLE3
#              APPEND
#   SETITEMS · STOP

import pickle
import struct
import zlib

KEY = 0x42424242  # the key real Ren'Py archives ship with


def _binunicode(s: str) -> bytes:
    raw = s.encode("utf-8")
    return b"X" + struct.pack("<I", len(raw)) + raw


def _long1(v: int) -> bytes:
    # LONG1: 1-byte length then little-endian two's-complement magnitude.
    if v == 0:
        return b"\x8a\x00"
    n = (v.bit_length() // 8) + 1
    return b"\x8a" + bytes([n]) + v.to_bytes(n, "little", signed=True)


def _binint(v: int) -> bytes:
    return b"J" + struct.pack("<i", v)


def _short_binstring(b: bytes) -> bytes:
    return b"U" + bytes([len(b)]) + b


def _put(idx: int) -> bytes:
    """BINPUT below 256, LONG_BINPUT above — exactly what a real index does once it
    has more than ~85 members, which is why a two-entry fixture never emits it."""
    if idx < 256:
        return b"q" + bytes([idx])
    return b"r" + struct.pack("<I", idx)


def _get(idx: int) -> bytes:
    if idx < 256:
        return b"h" + bytes([idx])
    return b"j" + struct.pack("<i", idx)


def build_index(entries, key=KEY) -> bytes:
    """entries: [(archive_name, offset, length)] — real offsets/lengths, XORed here.

    The empty prefix string is written ONCE and then fetched from the memo, which
    is how a real archive spends 270 of its BINGETs. Getting this wrong is how a
    fixture ends up exercising a code path no shipped game takes."""
    p = bytearray()
    p += b"\x80\x02"          # PROTO 2
    p += b"}"                 # EMPTY_DICT
    p += _put(0)
    p += b"("                 # MARK
    memo = 1
    prefix_memo = None
    for name, off, ln in entries:
        p += _binunicode(name)
        p += _put(memo); memo += 1
        p += b"]"             # EMPTY_LIST
        p += _put(memo); memo += 1
        p += _long1(off ^ key)
        p += _binint(ln ^ key)
        if prefix_memo is None:
            p += _short_binstring(b"")
            prefix_memo = memo
            p += _put(memo); memo += 1
        else:
            p += _get(prefix_memo)
        p += b"\x87"          # TUPLE3
        p += b"a"             # APPEND
    p += b"u"                 # SETITEMS
    p += b"."                 # STOP
    return bytes(p)


def build_rpa(files: dict, key=KEY) -> bytes:
    """files: {archive_name: content_bytes}"""
    header_len = len("RPA-3.0 %016x %08x\n" % (0, 0))
    body = bytearray()
    index_entries = []
    pos = header_len
    for name, content in files.items():
        index_entries.append((name, pos, len(content)))
        body += content
        pos += len(content)
    index = zlib.compress(build_index(index_entries, key))
    header = ("RPA-3.0 %016x %08x\n" % (pos, key)).encode("ascii")
    assert len(header) == header_len
    return bytes(header) + bytes(body) + index


if __name__ == "__main__":
    # 152 members: enough for the memo index to pass 255 so LONG_BINPUT appears,
    # which a small fixture never reaches.
    files = {
        "script.rpyc": b"RENPY RPC2" + b"\x00" * 40,
        "tl/english/common.rpyc": b"RENPY RPC2" + b"\x11" * 24,
    }
    for i in range(150):
        files["kNPCs/npc_%02d.rpyc" % i] = b"RENPY RPC2" + bytes([i]) * 16
    blob = build_rpa(files)
    out = "real_shape.rpa"
    open(out, "wb").write(blob)
    print("wrote %s (%d bytes)" % (out, len(blob)))

    # Prove the index is a genuine pickle by reading it with Python's own loader,
    # the same way the real archive was read.
    header = blob.split(b"\n", 1)[0]
    idx_off = int(header.split()[1], 16)
    key = int(header.split()[2], 16)
    idx = pickle.loads(zlib.decompress(blob[idx_off:]), encoding="latin1")
    print("index parsed by pickle: %d entries" % len(idx))
    for name, meta in idx.items():
        off, ln, prefix = meta[0]
        off ^= key
        ln ^= key
        got = blob[off:off + ln]
        assert got == files[name], "content mismatch for %s" % name
        print("  %-24s offset %-6d len %-4d prefix=%r  OK" % (name, off, ln, prefix))
    print("\nround trip verified — this is the shape real Ren'Py archives use")
