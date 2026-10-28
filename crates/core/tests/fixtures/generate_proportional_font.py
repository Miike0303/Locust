"""Generate Locust-owned metrics-only TTF; Python standard library, no font data."""

from pathlib import Path
import struct


def u16(*values):
    return struct.pack(">" + "H" * len(values), *values)


def u32(*values):
    return struct.pack(">" + "I" * len(values), *values)


def checksum(data):
    padded = data + bytes((-len(data)) % 4)
    return sum(struct.unpack(">" + "I" * (len(padded) // 4), padded)) & 0xFFFFFFFF


def generate():
    characters = sorted(set(range(0x20, 0x7F)) | {0xE9, 0xF1, 0x100, 0x2014})
    glyphs = {code: index + 1 for index, code in enumerate(characters)}
    segments = [(0x20, 0x7E), (0xE9, 0xE9), (0xF1, 0xF1), (0x100, 0x100), (0x2014, 0x2014), (0xFFFF, 0xFFFF)]
    count = len(segments)
    cmap4 = u16(4, 16 + count * 8, 0, count * 2, 8, 2, count * 2 - 8)
    cmap4 += u16(*(end for _, end in segments)) + u16(0)
    cmap4 += u16(*(start for start, _ in segments))
    cmap4 += u16(*((glyphs.get(start, 0) - start) & 0xFFFF for start, _ in segments))
    cmap4 += u16(*([0] * count))
    head = bytearray(54)
    head[0:4] = u32(0x10000)
    head[12:16] = u32(0x5F0F3CF5)
    head[18:20] = u16(1000)
    hhea = bytearray(36)
    hhea[0:4] = u32(0x10000)
    hhea[4:8] = struct.pack(">hh", 800, -200)
    hhea[34:36] = u16(len(characters) + 1)
    advances = {ord("i"): 200, ord("W"): 900}
    metrics = u16(500, 0) + b"".join(u16(advances.get(code, 500), 0) for code in characters)
    tables = {
        b"cmap": u16(0, 1, 3, 1) + u32(12) + cmap4,
        b"head": bytes(head),
        b"hhea": bytes(hhea),
        b"hmtx": metrics,
        b"maxp": u32(0x5000) + u16(len(characters) + 1),
        b"name": u16(0, 0, 6),
    }
    font = bytearray(u32(0x10000) + u16(len(tables), 64, 2, 32) + bytes(16 * len(tables)))
    head_offset = None
    for index, (tag, data) in enumerate(sorted(tables.items())):
        font += bytes((-len(font)) % 4)
        offset = len(font)
        font[12 + index * 16:28 + index * 16] = tag + u32(checksum(data), offset, len(data))
        font += data
        if tag == b"head":
            head_offset = offset
    font[head_offset + 8:head_offset + 12] = u32((0xB1B0AFBA - checksum(font)) & 0xFFFFFFFF)
    return bytes(font)


if __name__ == "__main__":
    output = Path(__file__).with_name("proportional-latin.ttf")
    output.write_bytes(generate())
    print(f"{output.name}: {output.stat().st_size} bytes")
