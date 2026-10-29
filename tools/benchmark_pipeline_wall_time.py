"""Compare pipeline revisions with one test body and explicit CLI identities.

Mock example (no Locust executable is launched):
  python tools/benchmark_pipeline_wall_time.py --mock --repeats 3 --binary-mib 64
Real example (run outside the restricted sandbox):
  python tools/benchmark_pipeline_wall_time.py --base-cli PATH --branch-cli PATH
Use the same freshly built CLI path twice to isolate Python orchestration, then
use each revision's freshly built CLI with identical build settings to compare
the complete change. JSON output includes exact source and binary identities.
"""
import argparse
import builtins
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import types
import unittest
from unittest.mock import patch

import test_ja_en_es_pipeline as tests


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_TEST = "ProductionTransportTests.test_glossary_actual_cli_exact_partial_fallback_and_install"


def load_pipeline(label, source):
    module = types.ModuleType("pipeline_wall_time_" + label)
    module.__file__ = str(ROOT / "tools/ja_en_es_pipeline.py")
    sys.modules[module.__name__] = module
    exec(compile(source, module.__file__, "exec"), module.__dict__)
    return module


def mock_case(module, binary_mib):
    """Run both language stages with real Python safety and publication checks."""
    case = tests.GlossaryRules()
    case.setUp()
    try:
        with case.cli.open("wb") as stream:
            for _ in range(binary_mib):
                stream.write(b"x" * (1024 * 1024))
        policy = {"version": 1, "ja-en": [{"term": "こんにちは", "translation": "Hello"},
                                           {"term": "学者", "translation": "Scholar"}],
                  "en-es": [{"term": "Hello", "translation": "Hola"},
                            {"term": "Scholar", "translation": "Erudito"}]}
        options = case.options(glossary_json=str(case.glossary(policy)))
        cli, glossaries = tests.MockCli(case), {}
        cli.missing = False
        actual = cli.run

        def child(argv, **kwargs):
            args = argv[3:]
            result = actual(argv, **kwargs)
            if args[0] in {"extract", "pivot"}:
                glossaries[Path(args[-1] if args[0] == "extract" else args[3])] = []
            if args[:2] == ["glossary", "add"]:
                values = tuple(a.split("=", 1)[1] for a in args
                               if a.startswith(("--term=", "--translation=")))
                glossaries[Path(args[2])].append((*values, args[-1], None, 0))
                glossaries[Path(args[2])].sort()
            elif args[:2] == ["glossary", "import"]:
                payload = module.read_json_file(Path(args[args.index("--input") + 1]))
                glossaries[Path(args[2])] = sorted(
                    (e["term"], e["translation"], e["lang_pair"], e["context"], int(e["case_sensitive"]))
                    for e in payload)
            if args[0] == "translate":
                for value in cli.rows[Path(args[1])]:
                    if any(entry[0].lower() == value["source"].lower() for entry in glossaries[Path(args[1])]):
                        value["provider_used"] = "glossary"
            return result

        with patch.object(module, "read_database", side_effect=cli.read), patch.object(
                module, "read_glossary", side_effect=lambda path: list(glossaries[Path(path)])), patch.object(
                module.subprocess, "run", side_effect=child):
            with case.assertRaisesRegex(module.PipelineError, "inject-spanish"):
                module.Pipeline(options).run()
        manifest = module.read_json_file(case.root / "run/pipeline-manifest.json")
        case.assertEqual(len(glossaries), 2)
        case.assertEqual(sum(c[0] == "translate" for c in cli.calls), 2)
        return manifest["stages"]
    finally:
        case.doCleanups()


def run_once(module, cli, test_name, mocked, binary_mib):
    records, components, started = [], [], time.perf_counter()
    originals = {name: getattr(module.Pipeline, name) for name in ("command", "install_glossary", "invariants")}

    def reporter(*args, **kwargs):
        # Keep optional timing diagnostics outside tests' stdout/stderr contracts.
        if len(args) == 1 and isinstance(args[0], str):
            try:
                value = json.loads(args[0])
            except ValueError:
                value = None
            if isinstance(value, dict) and value.get("event") == "pipeline-timing":
                components.append(value)
                return
        builtins.print(*args, **kwargs)

    def measured(name):
        def call(program, *args, **kwargs):
            start = time.perf_counter()
            try:
                return originals[name](program, *args, **kwargs)
            finally:
                label = str(args[0]) if args else ""
                if name == "install_glossary":
                    label = Path(label).name
                record = {"operation": name, "stage": label,
                          "seconds": round(time.perf_counter() - start, 6)}
                if name == "command":
                    receipts = [receipt for receipt in program.manifest["stages"] if receipt["stage"] == label]
                    if receipts:
                        record["child_seconds"] = receipts[-1]["seconds"]
                records.append(record)
        return call

    with ExitStack() as stack:
        stack.enter_context(patch.object(tests, "pipeline", module))
        stack.enter_context(patch.object(tests, "CLI", cli))
        stack.enter_context(patch.dict(os.environ, {"LOCUST_PIPELINE_TIMINGS": "1"}))
        stack.enter_context(patch.object(module, "print", reporter, create=True))
        for name in originals:
            stack.enter_context(patch.object(module.Pipeline, name, measured(name)))
        if mocked:
            stages = mock_case(module, binary_mib)
        else:
            suite = unittest.defaultTestLoader.loadTestsFromName(test_name, tests)
            result = unittest.TextTestRunner(stream=sys.stderr, verbosity=2).run(suite)
            if not result.wasSuccessful():
                raise RuntimeError("Comparison test failed; retain stderr diagnostics")
            stages = []
    return {"seconds": round(time.perf_counter() - started, 6), "operations": records, "components": components,
            "child_stages": stages}


def identity(path):
    hasher, size = hashlib.sha256(), 0
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
            size += len(chunk)
    return {"path": str(path.resolve()), "bytes": size, "sha256": hasher.hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-ref", default="HEAD")
    parser.add_argument("--base-cli", type=Path)
    parser.add_argument("--branch-cli", type=Path)
    parser.add_argument("--test", default=DEFAULT_TEST)
    parser.add_argument("--mock", action="store_true")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--binary-mib", type=int, default=64,
                        help="Identical fake CLI size; real invariant hashing remains enabled")
    args = parser.parse_args()
    if args.repeats < 1 or args.binary_mib < 1:
        parser.error("repeats and binary-mib must be positive")
    if not args.mock and (args.base_cli is None or args.branch_cli is None):
        parser.error("real runs require explicit --base-cli and --branch-cli")
    tests.SCRATCH.mkdir(exist_ok=True)
    base = subprocess.run(["git", "show", args.base_ref + ":tools/ja_en_es_pipeline.py"], cwd=ROOT,
                          check=True, stdout=subprocess.PIPE).stdout
    branch = (ROOT / "tools/ja_en_es_pipeline.py").read_bytes()
    variants = [("base", load_pipeline("base", base), args.base_cli, base),
                ("branch", load_pipeline("branch", branch), args.branch_cli, branch)]
    test_source = (ROOT / "tools/test_ja_en_es_pipeline.py").read_bytes()
    print(json.dumps({"event": "comparison-inputs", "base_ref": args.base_ref, "mock": args.mock,
                      "test": "mock-glossary-two-stage" if args.mock else args.test,
                      "test_sha256": hashlib.sha256(test_source).hexdigest(),
                      "cargo_target_dir": os.environ.get("CARGO_TARGET_DIR"),
                      "mock_cli_bytes": args.binary_mib * 1024 * 1024 if args.mock else None,
                      "variants": [{"label": label, "pipeline_sha256": hashlib.sha256(source).hexdigest(),
                                    "cli": identity(cli) if cli else None}
                                   for label, _, cli, source in variants]}, sort_keys=True), flush=True)
    for repeat in range(args.repeats):
        # Alternate order to reduce warm-cache and machine-load bias.
        for label, module, cli, _ in variants[::1 if repeat % 2 == 0 else -1]:
            result = run_once(module, cli, args.test, args.mock, args.binary_mib)
            print(json.dumps({"event": "comparison-run", "label": label, "repeat": repeat + 1,
                              **result}, sort_keys=True), flush=True)


if __name__ == "__main__":
    main()
