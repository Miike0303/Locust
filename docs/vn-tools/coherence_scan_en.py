"""Scan dumped per-scheme samples for ENGLISH coherence (for localized releases).
Correct scheme -> valid text, low replacement ratio, many common English words."""
import sys, os, re
d = sys.argv[1]
WORDS = [" the ", " and ", " to ", " a ", " of ", " that ", " you ", " is ", " it ", " in ", " I ", "'s ", "'t "]
LATWORD = re.compile(r"[A-Za-z]{2,}")
results = []
for fn in sorted(os.listdir(d)):
    if not fn.endswith(".bin"): continue
    b = open(os.path.join(d, fn), "rb").read()
    best = None
    for enc in ("cp932", "utf-16", "utf-8"):
        try:
            t = b.decode(enc, errors="replace")
        except Exception:
            continue
        n = max(len(t), 1)
        repl = t.count("�") / n
        words = len(LATWORD.findall(t))
        common = sum(t.lower().count(w) for w in WORDS)
        coherent = (repl < 0.02) and (common > 15) and (words > 100)
        score = (coherent, common, words, -repl)
        if best is None or score > best[0]:
            best = (score, enc, common, words, repl, coherent)
    if best:
        results.append((best[5], best[2], best[3], best[4], best[1], fn))
results.sort(key=lambda r: (r[0], r[1]), reverse=True)
print(f"{'COHER':6} {'commonW':>8} {'words':>6} {'repl':>6} enc     scheme")
for coh, common, words, repl, enc, fn in results[:12]:
    sch = fn.split("__", 1)[-1].replace(".bin", "")
    print(f"{str(coh):6} {common:>8} {words:>6} {repl:6.3f} {enc:7} {sch}")
print("\nCOHERENT(EN):", sum(1 for r in results if r[0]))
