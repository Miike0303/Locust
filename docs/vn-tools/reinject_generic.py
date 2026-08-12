"""Generic KiriKiri reinjector: writes ES translations back into a decrypted game
tree, as a UTF-16LE patch tree ready to pack into patchN.xp3.
Pairs with kag_extract_recursive.py (uses splitlines() + same decode).
Usage: reinject_generic.py <game_dir> <positions.json> <en2es_db> <out_patch_tree>
DB id format: '<flatname>.json#<index>#message'; flatname = rel path with /,\ -> _.
Outputs UTF-16LE+BOM so Spanish accents survive and KiriKiri reads it natively.
"""
import os, re, json, sqlite3, sys

GAME, POS_PATH, DB, OUT = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
POS = json.load(open(POS_PATH, encoding="utf-8"))

def decode(b):
    if b[:2] == b"\xff\xfe":
        return b.decode("utf-16")
    for enc in ("cp932", "utf-8", "utf-16"):
        try:
            return b.decode(enc)
        except Exception:
            continue
    return b.decode("cp932", "replace")

con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
idre = re.compile(r"^(.*)\.json#(\d+)#message$")
tr = {}
for _id, t in con.execute("select id, translation from strings"):
    if not t or not t.strip():
        continue
    m = idre.match(_id)
    if m:
        tr.setdefault(m.group(1), {})[int(m.group(2))] = t
con.close()

flat_to_rel = {}
for root, _, files in os.walk(GAME):
    for fn in files:
        rel = os.path.relpath(os.path.join(root, fn), GAME).replace("\\", "/")
        flat_to_rel[rel.replace("/", "_")] = rel

patched_files = 0; patched_lines = 0; missing = 0
for flat, idxs in POS.items():
    rel = flat_to_rel.get(flat)
    if rel is None:
        missing += 1; continue
    trans = tr.get(flat, {})
    if not trans:
        continue
    b = open(os.path.join(GAME, rel.replace("/", os.sep)), "rb").read()
    lines = decode(b).splitlines()
    changed = 0
    for i, lineno in enumerate(idxs):
        if i in trans and lineno < len(lines):
            lines[lineno] = trans[i]; changed += 1
    if changed == 0:
        continue
    dest = os.path.join(OUT, rel.replace("/", os.sep))
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    with open(dest, "wb") as f:
        f.write(b"\xff\xfe")
        f.write("\r\n".join(lines).encode("utf-16-le"))
    patched_files += 1; patched_lines += changed

print(f"patched files: {patched_files} | patched lines: {patched_lines} | positions w/o game file: {missing}")
