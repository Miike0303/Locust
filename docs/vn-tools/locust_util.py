import sqlite3, sys, datetime

cmd = sys.argv[1]

if cmd == "pending":
    db = sys.argv[2]
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    print(con.execute("select count(*) from strings where status='pending'").fetchone()[0])
    con.close()

elif cmd == "merge":
    # Add JA->EN translated entries that are missing from the en2es DB, as new
    # pending rows (source = the EN translation). Preserves existing ES work
    # because save_entries uses INSERT OR REPLACE (a full re-pivot would wipe it).
    ja, es = sys.argv[2], sys.argv[3]
    src = sqlite3.connect(f"file:{ja}?mode=ro", uri=True)
    rows = src.execute(
        "select id, translation, file_path, context, tags, metadata, char_limit "
        "from strings where status='translated' and translation is not null and trim(translation)<>''"
    ).fetchall()
    src.close()
    dst = sqlite3.connect(es)
    existing = set(r[0] for r in dst.execute("select id from strings").fetchall())
    now = datetime.datetime.now().isoformat()
    ins = 0
    for (id_, tr, fp, ctx, tags, meta, climit) in rows:
        if id_ in existing:
            continue
        dst.execute(
            "insert into strings (id, source, translation, status, file_path, context, "
            "tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at) "
            "values (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (id_, tr, None, 'pending', fp, ctx, tags, meta, climit, None, now, None, None),
        )
        ins += 1
    dst.commit()
    dst.close()
    print(f"merged {ins} new pending entries into {es}")
