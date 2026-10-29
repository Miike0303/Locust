#!/usr/bin/env python3
"""Measure whole-game I/O; no installed game, credentials, or network providers.

The default mocks child work only. Filesystem copies, content hashes, invariant
checks, backup-shaped copying, pack, apply and rollback checks run on real files.
--mode real uses the actual pipeline/CLI and a scripted loopback provider.
"""
import argparse
import json
import os
from pathlib import Path
import random
import shutil
import statistics
import subprocess
import sys
import tempfile
from unittest.mock import patch
import zipfile

import ja_en_es_pipeline as pipeline

MIB = 1024 * 1024
LIMIT = 2 * 1024 * MIB


def generate(root, mib, small_files, large_files):
    # Reuse the production suite's fixture and scripted provider contract.
    from test_ja_en_es_pipeline import fixture
    answers, _, _ = fixture(root, "renpy")
    budget = mib * MIB
    existing = sum(path.stat().st_size for path in root.rglob("*") if path.is_file())
    small_size = min(4096, (budget - existing) // (small_files * 2))
    pipeline.require(small_size > 0, "Fixture budget too small")
    block = random.Random(304).randbytes(MIB)
    for index in range(small_files):
        path = root / "assets" / f"small-{index % 32:02}" / f"asset-{index:06}.bin"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(block[:small_size])
    remaining = budget - existing - small_files * small_size
    for index in range(large_files):
        size = remaining // (large_files - index)
        with (root / "assets" / f"large-{index:02}.bin").open("xb") as stream:
            left = size
            while left:
                chunk = block[:min(left, len(block))]
                stream.write(chunk)
                left -= len(chunk)
        remaining -= size
    return answers


def mocked(options):
    """Exercise production guards and I/O, with explicit stand-ins for CLI work.

    This is a storage benchmark, not a claim to test Rust transaction semantics.
    The real mode runs those semantics, including immutable backup proofs.
    """
    program = pipeline.Pipeline(options)
    metrics = program.measurements
    program.root.mkdir()
    for name in ("profile", "backups"):
        (program.root / name).mkdir()
    (program.root / "profile/config.json").write_bytes(pipeline.encoded(pipeline.permitted_config(options)))
    program.manifest = {"status": "benchmark-mocked", "phase": "benchmark", "stages": [],
                        "injection_started": False, "active_command": None}

    def child(argv, **_):
        return subprocess.CompletedProcess(argv, 0, b"", b"")

    with metrics.activate(), metrics.stage("workflow"), patch.object(pipeline.subprocess, "run", side_effect=child):
        with metrics.stage("prepare-copies"):
            for destination in (program.pristine, program.work, program.install):
                pipeline.copy_tree(program.game, destination, program.original, readonly=destination == program.pristine)
        for stage, args in (("extract-japanese", ["extract"]), ("ja-en", ["translate"]),
                            ("validate-english", ["validate"]), ("strict-pivot", ["pivot"]),
                            ("en-es", ["translate"]), ("validate-spanish", ["validate"])):
            program.command(stage, args)
        program.invariants(before_injection=True)
        program.manifest["injection_started"] = True
        with metrics.stage("mock:inject-and-backup"):
            program.command("inject-spanish", ["inject"])
            pipeline.copy_tree(program.work, program.root / "backups/payload", program.original)
            script = program.work / "game/script.rpy"
            script.write_bytes(script.read_bytes().replace("こんにちは".encode(), b"Hola"))
            patched = pipeline.scan(program.work)
        with metrics.stage("mock:work-proof"):
            program.copy_proof(program.root / "proof-game", patched)
            program.command("reextract-work", ["extract"])
        with metrics.stage("mock:pack-and-proof"):
            program.command("pack-spanish", ["patch"])
            with zipfile.ZipFile(program.archive, "x", compression=zipfile.ZIP_DEFLATED) as archive:
                archive.write(script, "game/script.rpy")
            pipeline.file_hash(program.archive)
            with zipfile.ZipFile(program.archive) as archive:
                pipeline.require(archive.read("game/script.rpy") == script.read_bytes(), "ZIP proof failed")
        with metrics.stage("mock:apply-rollback-reapply"):
            for stage in ("strict-apply-dry-run", "strict-apply", "reextract-install", "rollback-proof", "final-reapply", "reextract-final-install"):
                program.command(stage, ["apply"])
                if stage in {"strict-apply", "final-reapply", "rollback-proof"}:
                    source = program.pristine if stage == "rollback-proof" else program.work
                    destination = program.install / "game/script.rpy"
                    destination.unlink()
                    pipeline.copy_bytes(source / "game/script.rpy", destination)
                expected = program.original if stage in {"strict-apply-dry-run", "rollback-proof"} else patched
                pipeline.require(pipeline.scan(program.install) == expected, "Install/rollback proof failed")
            program.invariants()
    program.save()
    return metrics.report()


def real(options, answers):
    from test_ja_en_es_pipeline import ScriptedEndpoint
    endpoint = ScriptedEndpoint(answers)
    try:
        options.test_endpoint = endpoint.url
        program = pipeline.Pipeline(options)
        program.run()
        pipeline.require(not endpoint.errors, "Scripted endpoint failed")
        return program.measurements.report()
    finally:
        endpoint.close()


def summarize(report):
    rows = {stage["stage"]: stage for stage in report["stages"]}
    wanted = ("preflight", "prepare-copies", "command:extract-japanese", "command:ja-en", "translate:ja-en",
              "command:validate-english", "command:strict-pivot", "command:en-es", "translate:en-es", "command:validate-spanish",
              "mock:inject-and-backup", "inject-and-proof", "command:inject-spanish", "copy-proof", "mock:pack-and-proof",
              "pack-and-proof", "command:pack-spanish", "mock:apply-rollback-reapply", "workflow")
    return {**{name: rows[name] for name in wanted if name in rows},
            "overall (includes preflight)": {"seconds": report["seconds"], **report["totals"]}}


def table(runs, workers):
    print("\nStage (nested rows overlap) | workers | wall s | hash MiB | copy MiB | invariant s | vs 1 worker")
    print("--- | ---: | ---: | ---: | ---: | ---: | ---:")
    summaries = {count: [summarize(run["report"]) for run in runs if run["workers"] == count] for count in workers}
    names = list(summaries[workers[0]][0])
    for name in names:
        baseline = statistics.median(row[name]["seconds"] for row in summaries[workers[0]])
        for count in workers:
            values = summaries[count]
            median = lambda key: statistics.median(row[name][key] for row in values)
            wall = median("seconds")
            print(f"{name} | {count} | {wall:.3f} | {median('hash_bytes') / MIB:.1f} | "
                  f"{median('copy_bytes') / MIB:.1f} | {median('invariant_seconds'):.3f} | {baseline / wall:.2f}x")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("mocked", "real"), default="mocked")
    parser.add_argument("--cli", type=Path, help="Required for real mode; never built by this script")
    parser.add_argument("--mib", type=int, default=192, help="Source size; all simultaneously retained trees are budgeted below 2 GiB")
    parser.add_argument("--small-files", type=int, default=2048)
    parser.add_argument("--large-files", type=int, default=3)
    parser.add_argument("--workers", default="1,2,4")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--scratch", type=Path, default=Path(__file__).resolve().parents[1] / ".test-tmp")
    parser.add_argument("--storage-label", default="unspecified", help="For separate SSD/HDD recordings; no device assumptions")
    parser.add_argument("--json", type=Path, help="Optional full measurement output, outside generated scratch")
    args = parser.parse_args(argv)
    workers = [int(value) for value in args.workers.split(",")]
    pipeline.require(workers and workers[0] == 1 and len(set(workers)) == len(workers)
                     and all(1 <= value <= 4 for value in workers), "Workers must start with 1 and stay within 1..4")
    pipeline.require(1 <= args.mib and args.mib * MIB * 8 + 64 * MIB <= LIMIT,
                     "Eight source-sized trees plus 64 MiB overhead must fit within 2 GiB")
    pipeline.require(1 <= args.small_files <= 20000 and 1 <= args.large_files <= 8 and 1 <= args.repeats <= 10,
                     "Invalid file distribution or repeat count")
    pipeline.require(args.mode == "mocked" or (args.cli is not None and args.cli.is_file()), "Real mode requires --cli")
    scratch = pipeline.plain(args.scratch, missing=True)
    pipeline.require(all(part.lower() != "tmp" for part in scratch.parts), "A tmp directory cannot be used")
    scratch.mkdir(parents=True, exist_ok=True)
    runs = []
    result = {"mode": args.mode, "storage": args.storage_label, "source_mib": args.mib,
              "small_files": args.small_files, "large_files": args.large_files, "runs": runs}
    root = Path(tempfile.mkdtemp(prefix="workflow-benchmark-", dir=scratch)).absolute()
    try:
        answers = generate(root / "source", args.mib, args.small_files, args.large_files)
        cli = args.cli.absolute() if args.cli else root / "mock-locust.exe"
        if args.mode == "mocked" and args.cli is None:
            cli.write_bytes(b"benchmark mock; never executed")
            cli.chmod(0o700)
        print(f"mode={args.mode}; storage={args.storage_label}; source={args.mib} MiB; "
              f"small={args.small_files}; large={args.large_files}; repeats={args.repeats}; warm OS cache; rotated worker order", flush=True)
        for repeat in range(args.repeats):
            order = workers[repeat % len(workers):] + workers[:repeat % len(workers)]
            for count in order:
                output = root / "run"
                options = pipeline.Options(str(cli), str(root / "source"), str(output), "renpy", "google", timeout=900)
                with patch.dict(os.environ, {"LOCUST_PIPELINE_IO_WORKERS": str(count)}):
                    report = mocked(options) if args.mode == "mocked" else real(options, answers)
                runs.append({"repeat": repeat + 1, "workers": count, "report": report})
                if args.json:
                    # Keep finished samples available even if a later sample fails.
                    args.json.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
                print(f"finished repeat={repeat + 1} workers={count} wall={report['seconds']:.3f}s", flush=True)
                # Only this generated, known absolute child is ever removed.
                pipeline.require(output.parent == root and output.name == "run", "Unexpected cleanup path")
                pipeline.plain(output)
                # Real pristine copies have read-only directories on POSIX and
                # read-only files on Windows. Make only generated members writable.
                for directory_path, _, files in os.walk(output, followlinks=False):
                    os.chmod(directory_path, 0o700)
                    for name in files:
                        os.chmod(Path(directory_path) / name, 0o600)
                shutil.rmtree(output)
    except BaseException:
        # A real child timeout may leave descendants alive. Retain owned evidence
        # and locks on every failure instead of a TemporaryDirectory auto-delete.
        print(f"Benchmark failed; generated evidence retained at {root}", file=sys.stderr, flush=True)
        raise
    pipeline.require(root.parent == scratch and root.name.startswith("workflow-benchmark-"), "Unexpected cleanup root")
    pipeline.plain(root)
    shutil.rmtree(root)
    table(runs, workers)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
