import os, re, sys
d = sys.argv[1]
HIRA = re.compile(r"[぀-ゟ]")
def best_hira(b):
    best = 0
    for enc in ("utf-16", "cp932"):
        try:
            t = b.decode(enc, errors="replace")
            best = max(best, len(HIRA.findall(t)))
        except Exception:
            pass
    return best
ok = 0; bad = 0; okfiles = []
allfiles = []
for root, _, files in os.walk(d):
    for fn in files:
        allfiles.append(os.path.join(root, fn))
for full in allfiles:
    fn = os.path.relpath(full, d)
    b = open(full, "rb").read()
    h = best_hira(b)
    if h >= 50:
        ok += 1; okfiles.append((h, len(b), fn))
    else:
        bad += 1
print(f"files decrypted OK (>=50 hiragana): {ok}")
print(f"files still garbage (<50 hiragana): {bad}")
okfiles.sort(reverse=True)
print("top OK files:")
for h, sz, fn in okfiles[:8]:
    print(f"   hira={h} size={sz} {fn}")
print("sample garbage (first 6):")
n=0
for fn in sorted(os.listdir(d)):
    b=open(os.path.join(d,fn),"rb").read()
    if best_hira(b) < 50:
        print(f"   {fn} size={len(b)} head={b[:4].hex()}"); n+=1
        if n>=6: break
