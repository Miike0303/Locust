"""Recursive KAG dialogue extractor for an ENGLISH-translated game tree.
Like kag_extract_recursive.py but keeps lines whose tag-stripped residual has a
run of >=2 Latin letters (English dialogue/narration), skipping @/*/; command,
label and comment lines. Emits VNTextPatch {message} JSON + positions map.
"""
import os, re, json, sys, io
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")

SRC, JSON_OUT, MAP_OUT = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(JSON_OUT, exist_ok=True)

LAT = re.compile(r"[A-Za-z]{2,}")
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
            residual = TAG.sub("", s)
            if LAT.search(residual):
                msgs.append({"message": line})
                idxs.append(i)
        if msgs:
            json.dump(msgs, open(os.path.join(JSON_OUT, flat + ".json"), "w", encoding="utf-8"),
                      ensure_ascii=False, indent=2)
            positions[flat] = idxs
            total += len(msgs); files_ct += 1

json.dump(positions, open(MAP_OUT, "w", encoding="utf-8"))
print(f"files with English text: {files_ct} | translatable lines: {total}")
