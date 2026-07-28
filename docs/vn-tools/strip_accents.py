"""Transliterate Spanish accents to ASCII in VNTextPatch output JSONs, in place.
For engines whose text encoding (Shift-JIS) cannot hold á/é/í/ó/ú/ñ/¿/¡.
á→a ñ→n ¿¡→(removed) « »→" — keeps the text readable in pure ASCII.
Usage: strip_accents.py <output_dir>
"""
import sys, os, json, glob, unicodedata

d = sys.argv[1]
DROP = "¿¡"                     # opening marks: not in SJIS, remove
MAP = {"«": '"', "»": '"', "“": '"', "”": '"', "‘": "'", "’": "'",
       "—": "-", "–": "-", "…": "...", "　": " "}

def translit(s):
    out = []
    for ch in s:
        if ch in DROP:
            continue
        if ch in MAP:
            out.append(MAP[ch]); continue
        # decompose (á -> a + combining accent) and drop combining marks
        dec = unicodedata.normalize("NFKD", ch)
        base = "".join(c for c in dec if not unicodedata.combining(c))
        # keep if ascii-ish, else keep original (JP text untouched)
        out.append(base if base else ch)
    return "".join(out)

files = glob.glob(os.path.join(d, "*.json"))
changed_files = 0; changed_msgs = 0
for f in files:
    try:
        data = json.load(open(f, encoding="utf-8"))
    except Exception:
        continue
    if not isinstance(data, list):
        continue
    ch = 0
    for it in data:
        if isinstance(it, dict) and "message" in it:
            new = translit(it["message"])
            if new != it["message"]:
                it["message"] = new; ch += 1
    if ch:
        json.dump(data, open(f, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
        changed_files += 1; changed_msgs += ch
print(f"files: {len(files)} | changed files: {changed_files} | messages transliterated: {changed_msgs}")
