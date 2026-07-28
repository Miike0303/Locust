import os, json, sys
d = sys.argv[1]
rows = []
for fn in os.listdir(d):
    if not fn.endswith(".json"): continue
    try:
        data = json.load(open(os.path.join(d, fn), encoding="utf-8"))
    except Exception as e:
        print("ERR", fn, e); continue
    if not isinstance(data, list) or not data: continue
    lens = [len(x.get("message","")) for x in data]
    rows.append((max(lens), len(data), sum(lens), fn))
rows.sort(reverse=True)
print(f"{'maxlen':>8} {'entries':>8} {'totchars':>9}  file")
for mx, n, tot, fn in rows[:14]:
    print(f"{mx:>8} {n:>8} {tot:>9}  {fn}")
print(f"... {len(rows)} files total")
big = [r for r in rows if r[0] > 8000]
print(f"files with a message >8000 chars: {len(big)}")
