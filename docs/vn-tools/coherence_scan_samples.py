"""Scan dumped per-scheme samples by TEXT COHERENCE (not byte counts).
A correct decrypt yields dense hiragana with a LOW replacement-char ratio and
common Japanese particles. Reports schemes that look like real Japanese text."""
import sys, os, re
d = sys.argv[1]
HIRA = re.compile(r"[぀-ゟ]")
PARTICLES = ["の", "は", "を", "に", "た", "て", "が", "です", "ます", "し", "と", "な"]
results = []
for fn in sorted(os.listdir(d)):
    if not fn.endswith(".bin"):
        continue
    b = open(os.path.join(d, fn), "rb").read()
    best = None
    for enc in ("utf-16", "cp932"):
        try:
            t = b.decode(enc, errors="replace")
        except Exception:
            continue
        n = max(len(t), 1)
        hira = len(HIRA.findall(t))
        repl = t.count("�") / n
        part = sum(t.count(p) for p in PARTICLES)
        # coherence: dense hiragana, few replacement chars, many particles
        coherent = (hira / n > 0.10) and (repl < 0.01) and (part > 40)
        score = (coherent, hira, -repl, part)
        if best is None or score > best[0]:
            best = (score, enc, hira, repl, part, coherent)
    if best:
        results.append((best[5], best[2], best[3], best[4], best[1], fn))
# coherent first, then by hiragana
results.sort(key=lambda r: (r[0], r[1]), reverse=True)
print(f"{'COHER':6} {'hira':>6} {'repl':>6} {'part':>5} enc     scheme")
for coh, hira, repl, part, enc, fn in results[:15]:
    scheme = fn.split("__", 1)[-1].replace(".bin", "")
    print(f"{str(coh):6} {hira:>6} {repl:6.3f} {part:>5} {enc:7} {scheme}")
n_coh = sum(1 for r in results if r[0])
print(f"\nCOHERENT schemes found: {n_coh}")
