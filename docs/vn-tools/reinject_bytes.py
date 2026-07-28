"""Byte-level reinjection: preserve original file bytes EXACTLY for every line
except the translated dialogue lines. Avoids cp932 decode/re-encode round-trip
corruption of labels/tags (which broke KAG's *label lookup). Translated lines
are transliterated to ASCII and encoded in the file's original encoding.
Usage: reinject_bytes.py <game_dir> <positions.json> <en2es_db> <out_tree> [--keep-accents]
"""
import os, re, json, sqlite3, sys, unicodedata

GAME, POS_PATH, DB, OUT = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
KEEP = "--keep-accents" in sys.argv[5:]
POS = json.load(open(POS_PATH, encoding="utf-8"))

DROP = "¿¡"
MAP = {"«": '"', "»": '"', "“": '"', "”": '"', "‘": "'", "’": "'", "—": "-", "–": "-", "…": "..."}
def translit(s):
    if KEEP:
        return s
    out = []
    for ch in s:
        if ch in DROP: continue
        if ch in MAP: out.append(MAP[ch]); continue
        dec = unicodedata.normalize("NFKD", ch)
        base = "".join(c for c in dec if not unicodedata.combining(c))
        out.append(base if base else ch)
    return "".join(out)

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
    # encoding + newline detected from the ORIGINAL bytes
    if raw[:2] == b"\xff\xfe":
        enc, bom = "utf-16-le", b"\xff\xfe"
    else:
        enc, bom = "cp932", b""
    body = raw[len(bom):]
    # Reject only HEAVILY-garbage files (wrong scheme). A tiny replacement ratio
    # is a near-miss we sanitize per-line below.
    dec = body.decode(enc, errors="replace")
    if dec and dec.count("�") / len(dec) > 0.03:
        continue
    nl = b"\r\n" if body.count(b"\r\n") else (b"\r" if body.count(b"\r") > body.count(b"\n") else b"\n")
    if enc == "utf-16-le":
        nl = nl.decode("ascii").encode("utf-16-le")
    blines = body.split(nl)
    changed = 0; done = set()
    for i, lineno in enumerate(idxs):
        if i in trans and lineno < len(blines):
            blines[lineno] = translit(trans[i]).encode(enc, errors="replace")
            done.add(lineno); changed += 1
    if changed == 0: continue
    # sanitize untranslated lines that carry invalid bytes (near-miss garbage),
    # so the whole file is valid in `enc` (avoids the engine's ANSI->Unicode crash).
    for j in range(len(blines)):
        if j in done: continue
        try:
            blines[j].decode(enc)
        except Exception:
            blines[j] = blines[j].decode(enc, "replace").encode(enc, "replace")
    dest = os.path.join(OUT, rel.replace("/", os.sep))
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    open(dest, "wb").write(bom + nl.join(blines))
    pf += 1; pl += changed

print(f"patched files: {pf} | patched lines: {pl} | positions w/o file: {miss} | keep_accents={KEEP}")
