"""Recursive KAG dialogue extractor for a DECRYPTED game tree (paths preserved).
Walks SRC, decodes each .ks/.txt/.scn (BOM utf-16, else cp932, else utf-16),
splits on universal newlines, keeps lines whose visible text (after stripping
[tags]) contains Japanese and that aren't @/*/; commands. Writes one VNTextPatch
{message} JSON per file, named by FLATTENED relative path (/,\ -> _), plus a
positions map {flatname: [line_index,...]} for reinjection.
"""
import os, re, json, sys, io
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")

SRC, JSON_OUT, MAP_OUT = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(JSON_OUT, exist_ok=True)

JP = re.compile(r"[぀-ヿ一-鿿ｦ-ﾟ]")
TAG = re.compile(r"\[[^\]]*\]")

def decode(b):
    if b[:2] == b"\xff\xfe":
        return b.decode("utf-16")
    for enc in ("cp932", "utf-8", "utf-16"):
        try:
            return b.decode(enc)
        except Exception:
            continue
    return b.decode("cp932", "replace")

positions = {}
total = 0; files_ct = 0
for root, _, files in os.walk(SRC):
    for fn in files:
        if not fn.lower().endswith((".ks", ".txt", ".scn")):
            continue
        rel = os.path.relpath(os.path.join(root, fn), SRC).replace("\\", "/")
        flat = rel.replace("/", "_")
        try:
            lines = decode(open(os.path.join(root, fn), "rb").read()).splitlines()
        except Exception:
            continue
        msgs = []; idxs = []
        for i, line in enumerate(lines):
            s = line.strip()
            if not s or s[0] in "@*;":
                continue
            if JP.search(TAG.sub("", s)):
                msgs.append({"message": line})
                idxs.append(i)
        if msgs:
            json.dump(msgs, open(os.path.join(JSON_OUT, flat + ".json"), "w", encoding="utf-8"),
                      ensure_ascii=False, indent=2)
            positions[flat] = idxs
            total += len(msgs); files_ct += 1

json.dump(positions, open(MAP_OUT, "w", encoding="utf-8"))
print(f"files with dialogue: {files_ct} | translatable lines: {total}")
