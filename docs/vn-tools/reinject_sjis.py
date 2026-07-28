"""Reinject ES into a KiriKiri game tree as cp932 (Shift-JIS), accents stripped.
Use when the engine reads patch .ks as cp932 (UTF-16 breaks label parsing, e.g.
Taimanin's "*filetop not found"). cp932 can't hold á/ñ, so accents are
transliterated to ASCII (á→a, ñ→n, ¿¡ removed). Keeps original line structure
(labels/tags), replaces only dialogue lines.
Usage: reinject_sjis.py <game_dir> <positions.json> <en2es_db> <out_tree>
"""
import os, re, json, sqlite3, sys, unicodedata

GAME, POS_PATH, DB, OUT = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
POS = json.load(open(POS_PATH, encoding="utf-8"))

DROP = "¿¡"
MAP = {"«": '"', "»": '"', "“": '"', "”": '"', "‘": "'", "’": "'",
       "—": "-", "–": "-", "…": "...", "　": " "}
def translit(s):
    out = []
    for ch in s:
        if ch in DROP: continue
        if ch in MAP: out.append(MAP[ch]); continue
        dec = unicodedata.normalize("NFKD", ch)
        base = "".join(c for c in dec if not unicodedata.combining(c))
        out.append(base if base else ch)
    return "".join(out)

def decode(b):
    if b[:2] == b"\xff\xfe": return b.decode("utf-16")
    for enc in ("cp932", "utf-8", "utf-16"):
        try: return b.decode(enc)
        except Exception: continue
    return b.decode("cp932", "replace")

con = sqlite3.connect(f"file:{DB}?mode=ro", uri=True)
idre = re.compile(r"^(.*)\.json#(\d+)#message$")
tr = {}
for _id, t in con.execute("select id, translation from strings"):
    if not t or not t.strip(): continue
    m = idre.match(_id)
    if m: tr.setdefault(m.group(1), {})[int(m.group(2))] = t
con.close()

flat_to_rel = {}
for root, _, files in os.walk(GAME):
    for fn in files:
        rel = os.path.relpath(os.path.join(root, fn), GAME).replace("\\", "/")
        flat_to_rel[rel.replace("/", "_")] = rel

pf = pl = miss = 0
for flat, idxs in POS.items():
    rel = flat_to_rel.get(flat)
    if rel is None: miss += 1; continue
    trans = tr.get(flat, {})
    if not trans: continue
    raw = open(os.path.join(GAME, rel.replace("/", os.sep)), "rb").read()
    src_enc = "utf-16" if raw[:2] == b"\xff\xfe" else "cp932"
    lines = decode(raw).splitlines()
    changed = 0
    for i, lineno in enumerate(idxs):
        if i in trans and lineno < len(lines):
            lines[lineno] = translit(trans[i]); changed += 1
    if changed == 0: continue
    dest = os.path.join(OUT, rel.replace("/", os.sep))
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    text = "\r\n".join(lines)
    # write in the SAME encoding as the original (usually cp932, no BOM)
    with open(dest, "wb") as f:
        f.write(text.encode(src_enc, errors="replace"))
    pf += 1; pl += changed

print(f"patched files: {pf} | patched lines: {pl} | positions w/o file: {miss}")
