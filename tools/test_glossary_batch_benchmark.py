"""Opt-in W3-01 orchestration benchmark; never launches the Locust CLI.

Set LOCUST_RUN_GLOSSARY_BENCHMARK=1 and run unittest discovery for this file.
The 1k-term fixture calls the internal installer directly, bypassing the public
200-term phase cap. Database reads, child processes and invariant scans are
mocked, so the reported time measures Python orchestration only.
For real filesystem/invariant costs and apples-to-apples revision comparisons,
use benchmark_pipeline_wall_time.py; this count benchmark cannot establish
production wall-time improvements.
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

import ja_en_es_pipeline as pipeline


def install_one_term_at_a_time(program, path):
    """Pre-W3-01 installer retained solely as a measured reference."""
    program.verify_glossary(path, [])
    before = pipeline.read_database(path)
    entries = pipeline.glossary_entries(program.settings, path.name)
    for index, entry in enumerate(entries):
        program.command("glossary-english-" + str(index), ["glossary", "add", path,
                        "--term=" + entry["term"], "--translation=" + entry["translation"], "--lang-pair", "ja-en"])
        program.verify_glossary(path, entries[:index + 1])
        pipeline.require(pipeline.read_database(path) == before, "Glossary CLI changed resource rows/metadata")
    program.verify_glossary(path)


@unittest.skipUnless(os.environ.get("LOCUST_RUN_GLOSSARY_BENCHMARK") == "1", "Opt-in synthetic benchmark")
class GlossaryBatchBenchmark(unittest.TestCase):
    def test_100k_rows_1k_terms_subprocess_counts(self):
        terms = [{"term": f"term-{index:04d}", "translation": f"target-{index:04d}"} for index in range(1000)]
        rows = [{"id": index, "source": terms[index % 1000]["term"], "translation": None, "status": "pending"}
                for index in range(100000)]
        metadata = {"game_root": "synthetic"}
        with tempfile.TemporaryDirectory(prefix="glossary-batch-benchmark-", dir=Path(__file__).resolve().parents[1]) as root:
            for label, install in (("before", install_one_term_at_a_time), ("after", pipeline.Pipeline.install_glossary)):
                case = Path(root) / label
                (case / "profile").mkdir(parents=True)
                program = pipeline.Pipeline.__new__(pipeline.Pipeline)
                program.root, program.en, program.es = case, case / "english.db", case / "spanish.db"
                program.cli = case / "mock-locust.exe"
                program.options = pipeline.Options(str(program.cli), str(case / "source"), str(case), "renpy", "google")
                program.settings = {"glossary": {"version": 1, "ja-en": terms, "en-es": []}}
                program.manifest = {"injection_started": False, "stages": []}
                program.save = lambda: None
                counts = dict(subprocesses=0, invariant_passes=0, database_reads=0, glossary_reads=0)
                glossary = []

                def invariants(**_):
                    counts["invariant_passes"] += 1

                def read_database(_):
                    counts["database_reads"] += 1
                    return list(rows), dict(metadata)

                def read_glossary(_):
                    counts["glossary_reads"] += 1
                    return list(glossary)

                def child(argv, **_):
                    counts["subprocesses"] += 1
                    args = argv[3:]
                    if args[:2] == ["glossary", "add"]:
                        entry = tuple(a.split("=", 1)[1] for a in args if a.startswith(("--term=", "--translation=")))
                        glossary.append((*entry, args[-1], None, 0))
                    else:
                        payload = pipeline.read_json_file(Path(args[args.index("--input") + 1]))
                        glossary[:] = [(e["term"], e["translation"], e["lang_pair"], e["context"], int(e["case_sensitive"]))
                                       for e in payload]
                    return subprocess.CompletedProcess(argv, 0, b"", b"")

                program.invariants = invariants
                started = time.perf_counter()
                with patch.object(pipeline, "read_database", side_effect=read_database), patch.object(
                        pipeline, "read_glossary", side_effect=read_glossary), patch.object(
                        pipeline, "child_environment", return_value={}), patch.object(pipeline.subprocess, "run", side_effect=child):
                    install(program, program.en)
                expected = (1000, 1000, 1001, 1002) if label == "before" else (1, 1, 2, 2)
                self.assertEqual(tuple(counts.values()), expected)
                self.assertEqual(len(glossary), len(terms))
                print(json.dumps({"label": label, "scope": "mocked-orchestration-only",
                                  "rows": len(rows), "terms": len(terms), **counts,
                                  "seconds": round(time.perf_counter() - started, 6)}, sort_keys=True))


if __name__ == "__main__":
    unittest.main()
