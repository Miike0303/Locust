"""Reinject ES translations into Ochiru's .ks/.txt, output as a patch tree.
- Reads originals (paths preserved) from ochiru_game/.
- positions map + DB were built by kag_extract2 which used .split("\\n") and
  cp932-first decode -> we replicate EXACTLY so line indices line up.
- Output is written UTF-16LE + BOM (KiriKiri reads it natively) so Spanish
  accents/ñ/¡¿ survive (cp932 can't hold them).
"""
import os, re, json, sqlite3, sys

BASE = "D:/juegos/parches/locust-tests"
GAME = f"{BASE}/ochiru_game"
POS  = json.load(open(f"{BASE}/ochiru_positions.json", encoding="utf-8"))
OUT  = f"{BASE}/ochiru_patch_tree"

def decode_like_extractor(b):
    if b[:2] == b"\xff\xfe":
        return b.decode("utf-16"), "utf-16"
    for enc in ("cp932", "utf-8", "utf-16"):
        try:
            return b.decode(enc), enc
        except Exception:
            continue
    return b.decode("cp932", "replace"), "cp932"

# translations grouped by flatname, ordered by index i (id = "<flat>.json#<i>#message")
con = sqlite3.connect(f"file:{BASE}/ochiru_en2es.locust.db?mode=ro", uri=True)
tr = {}
idre = re.compile(r"^(.*)\.json#(\d+)#message$")
for _id, translation in con.execute("select id, translation from strings"):
    if not translation or not translation.strip():
        continue
    m = idre.match(_id)
    if not m:
        continue
    flat, i = m.group(1), int(m.group(2))
    tr.setdefault(flat, {})[i] = translation
con.close()

# map flatname -> real relative path by walking the game tree
flat_to_rel = {}
for root, _, files in os.walk(GAME):
    for fn in files:
        rel = os.path.relpath(os.path.join(root, fn), GAME).replace("\\", "/")
        flat_to_rel[rel.replace("/", "_")] = rel

patched_files = 0; patched_lines = 0; missing = []
for flat, idx_list in POS.items():
    rel = flat_to_rel.get(flat)
    if rel is None:
        missing.append(flat); continue
    trans = tr.get(flat, {})
    if not trans:
        continue
    b = open(os.path.join(GAME, rel.replace("/", os.sep)), "rb").read()
    text, enc = decode_like_extractor(b)
    lines = text.split("\n")
    changed = 0
    for i, lineno in enumerate(idx_list):
        if i not in trans:            # untranslated (irreducible) -> leave original
            continue
        if lineno >= len(lines):
            continue
        had_cr = lines[lineno].endswith("\r")
        lines[lineno] = trans[i] + ("\r" if had_cr else "")
        changed += 1
    if changed == 0:
        continue
    dest = os.path.join(OUT, rel.replace("/", os.sep))
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    out_text = "\n".join(lines)
    with open(dest, "wb") as f:
        f.write(b"\xff\xfe")                     # UTF-16LE BOM
        f.write(out_text.encode("utf-16-le"))
    patched_files += 1; patched_lines += changed

print(f"patched files: {patched_files} | patched lines: {patched_lines}")
if missing:
    print(f"positions with no matching game file: {len(missing)} (e.g. {missing[:3]})")
