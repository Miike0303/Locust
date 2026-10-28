"""Reproduce W3-03 release measurements without changing the checkout or target dir.

The baseline is a generated copy of the formats crate read exclusively with
``git show HEAD:<file>``. Both implementations share one release binary and core.
Generated sources/fixtures stay in .bench-w3 (never a tmp/ directory).
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import statistics
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / ".bench-w3"
EXPECTED_TARGET = Path("C:/Users/Mike/AppData/Local/locust-shared-target/lane-a")
PRIORITY_CLASSES = {"normal": 0x20, "above-normal": 0x8000}


def git(*args: str) -> bytes:
    return subprocess.check_output(["git", *args], cwd=ROOT)


def source_manifest() -> dict:
    relevant = [
        "crates/formats/src/lib.rs",
        "crates/formats/src/renpy.rs",
        "crates/formats/src/renpy_pickle.rs",
        "crates/formats/src/rpgmaker_mv.rs",
        "crates/formats/src/discovery.rs",
        "crates/formats/src/util.rs",
    ]
    candidate = relevant + ["crates/formats/src/file_workers.rs"]
    return {
        "head": git("rev-parse", "HEAD").decode().strip(),
        "baseline_source_tree": git("rev-parse", "HEAD:crates/formats/src").decode().strip(),
        "baseline_source_sha256": {
            name: hashlib.sha256(git("show", f"HEAD:{name}")).hexdigest()
            for name in relevant
        },
        "candidate_source_sha256": {
            name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
            for name in candidate
        },
        "harness_source_sha256": {
            name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
            for name in ["scripts/bench_w3_release.py", "scripts/bench_w3_release.rs"]
        },
    }


def prepare() -> dict:
    target = os.environ.get("CARGO_TARGET_DIR")
    if not target or Path(target).resolve() != EXPECTED_TARGET.resolve():
        raise RuntimeError(f"Preserve the required CARGO_TARGET_DIR: {EXPECTED_TARGET}")
    baseline = WORK / "baseline"
    baseline.mkdir(parents=True, exist_ok=True)
    tracked = git("ls-tree", "-r", "--name-only", "HEAD", "crates/formats/src").decode().splitlines()
    for relative in tracked:
        if not relative.endswith(".rs"):
            continue
        destination = baseline / Path(relative).relative_to("crates/formats")
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(git("show", f"HEAD:{relative}"))
    manifest = git("show", "HEAD:crates/formats/Cargo.toml").decode()
    manifest = manifest.replace('name = "locust-formats"', 'name = "locust-formats-serial-baseline"', 1)
    core = (ROOT / "crates/core").as_posix()
    manifest = manifest.replace('path = "../core"', f'path = "{core}"')
    (baseline / "Cargo.toml").write_text(manifest, encoding="utf-8")
    workspace = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    workspace = re.sub(r"members = \[.*?\]", 'members = ["baseline"]', workspace, count=1, flags=re.S)
    formats = (ROOT / "crates/formats").as_posix()
    workspace += f'''
[package]
name = "w3-release"
version = "0.0.0"
edition = "2021"

[dependencies]
locust-core = {{ path = "{core}" }}
locust-formats = {{ path = "{formats}" }}
locust-formats-serial-baseline = {{ path = "baseline" }}
serde_json = "1"
blake3 = "1"
walkdir = "2"

[[bin]]
name = "w3-release"
path = "main.rs"
'''
    (WORK / "Cargo.toml").write_text(workspace, encoding="utf-8")
    (WORK / "main.rs").write_bytes((ROOT / "scripts/bench_w3_release.rs").read_bytes())
    (WORK / "Cargo.lock").write_bytes((ROOT / "Cargo.lock").read_bytes())
    (WORK / ".gitignore").write_text("*\n!.gitignore\n", encoding="utf-8")
    manifest = source_manifest()
    (WORK / "source-manifest.json").write_text(json.dumps(manifest, sort_keys=True), encoding="utf-8")
    return manifest


def metadata(sources: dict) -> dict:
    model = platform.processor()
    if sys.platform == "win32":
        import winreg

        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") as key:
            model = winreg.QueryValueEx(key, "ProcessorNameString")[0].strip()
    return {
        "kind": "metadata",
        "started_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "machine": platform.node(),
        "os": platform.platform(),
        "processor": model,
        "logical_processors": os.cpu_count(),
        "cargo_target_dir": os.environ["CARGO_TARGET_DIR"],
        **sources,
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "profile": "release (Cargo default opt-level=3)",
        "spread_definition": "100 * (maximum - minimum) / median",
    }


def bucket_stats(samples: list[dict]) -> dict:
    times = [sample["ms"] for sample in samples]
    median = statistics.median(times)
    return {
        "runs": len(times),
        "median_ms": median,
        "min_ms": min(times),
        "max_ms": max(times),
        "spread_pct": (max(times) - min(times)) / median * 100,
        "cpu_busy_pct": statistics.median(sample["cpu_busy_pct"] for sample in samples),
        "external_cpu_pct": statistics.median(sample["external_cpu_pct"] for sample in samples),
    }


def executable_path() -> Path:
    return Path(os.environ["CARGO_TARGET_DIR"]) / "release" / ("w3-release.exe" if os.name == "nt" else "w3-release")


def verify_executable(expected_sources: dict) -> None:
    result = subprocess.run([str(executable_path()), "--build-manifest"], cwd=ROOT, text=True, capture_output=True)
    try:
        compiled_sources = json.loads(result.stdout) if result.returncode == 0 else None
    except json.JSONDecodeError:
        compiled_sources = None
    if compiled_sources != expected_sources:
        raise RuntimeError("Release executable source hashes differ from current sources; rebuild the generated crate before measuring")


def run_binary(arguments: list[str], stream, priority: str) -> list[dict]:
    samples = []
    expected_priority = PRIORITY_CLASSES[priority]
    flags = expected_priority if os.name == "nt" else 0
    observed_runtime = False
    with subprocess.Popen([str(executable_path()), *arguments], cwd=ROOT, text=True, stdout=subprocess.PIPE, creationflags=flags) as process:
        for line in process.stdout:
            stream.write(line)
            stream.flush()
            sample = json.loads(line)
            if sample["kind"] == "sample":
                if not observed_runtime or sample["priority_class"] != expected_priority:
                    raise RuntimeError("Benchmark runtime priority differs from the requested priority")
                samples.append(sample)
            elif sample["kind"] == "runtime":
                if sample["priority_class"] != expected_priority:
                    raise RuntimeError("Benchmark startup priority differs from the requested priority")
                observed_runtime = True
            elif sample["kind"] == "progress":
                print(sample["message"], flush=True)
        if process.wait():
            raise RuntimeError(f"Benchmark exited with {process.returncode}")
    if not observed_runtime:
        raise RuntimeError("Benchmark did not verify its runtime priority")
    return samples


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare", action="store_true", help="write generated crate only; do not build")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--default-only", action="store_true", help="paired HEAD baseline and selected default policy only")
    parser.add_argument("--priority", choices=PRIORITY_CLASSES, default="normal", help="Windows benchmark child priority; build and other processes are unchanged")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--max-attempts", type=int, default=5)
    parser.add_argument("--output", type=Path, default=ROOT / "docs/performance/w3-03-release")
    args = parser.parse_args()
    if args.runs < 5:
        parser.error("At least five runs are required")
    if args.priority != "normal" and os.name != "nt":
        parser.error("Above-normal process priority is supported only on Windows")
    prepared_sources = prepare()
    if args.prepare:
        print(f"Prepared {WORK / 'Cargo.toml'}")
        return
    if not args.skip_build:
        subprocess.run(["cargo", "build", "--release", "--manifest-path", str(WORK / "Cargo.toml"), "--bin", "w3-release"], cwd=ROOT, check=True)
    verify_executable(prepared_sources)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    raw_path = args.output.with_suffix(".jsonl")
    info = metadata(prepared_sources)
    info["compiled_source_manifest_verified"] = True
    info["requested_priority"] = args.priority
    info["priority_class"] = PRIORITY_CLASSES[args.priority]
    info["variants"] = ["baseline", "default"] if args.default_only else ["baseline", "1", "2", "4", "8"]
    extra_arguments = ["--default-only", "true"] if args.default_only else []
    extra_arguments += ["--priority", args.priority]
    with raw_path.open("w", encoding="utf-8") as stream:
        stream.write(json.dumps(info) + "\n")
        samples = run_binary(["--root", str(WORK / "fixtures"), "--runs", str(args.runs), "--attempt", "1", *extra_arguments], stream, args.priority)
        latest_by_shape = {}
        for sample in samples:
            latest_by_shape.setdefault((sample["engine"], sample["shape"]), []).append(sample)
        for attempt in range(1, args.max_attempts + 1):
            noisy = set()
            for shape, rows in latest_by_shape.items():
                buckets = {}
                for sample in rows:
                    buckets.setdefault((sample["variant"], sample["phase"]), []).append(sample)
                if any(bucket_stats(bucket)["spread_pct"] > 15 for bucket in buckets.values()):
                    noisy.add(shape)
            if not noisy or attempt == args.max_attempts:
                break
            for engine, shape in sorted(noisy):
                print(f"Rerunning noisy shape {engine}/{shape}, attempt {attempt + 1}", flush=True)
                # Replace every variant and phase for this shape together. Never
                # combine quiet buckets from separate attempts into one result.
                latest_by_shape[(engine, shape)] = run_binary(["--root", str(WORK / "fixtures"), "--runs", str(args.runs), "--attempt", str(attempt + 1), "--engine", engine, "--shape", shape, *extra_arguments], stream, args.priority)
    buckets = {}
    for rows in latest_by_shape.values():
        for sample in rows:
            key = (sample["engine"], sample["shape"], sample["variant"], sample["phase"])
            buckets.setdefault(key, []).append(sample)
    results = [{"engine": key[0], "shape": key[1], "variant": key[2], "phase": key[3], "attempt": rows[0]["attempt"], **bucket_stats(rows)} for key, rows in sorted(buckets.items())]
    after_digests = {
        name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
        for name in info["candidate_source_sha256"]
    }
    if after_digests != info["candidate_source_sha256"]:
        raise RuntimeError("Candidate sources changed while measuring; raw attempts retained, evidence rejected")
    info["candidate_sources_unchanged_during_run"] = True
    info["priority_verified_every_sample"] = True
    args.output.with_suffix(".json").write_text(json.dumps({"metadata": info, "results": results}, indent=2) + "\n", encoding="utf-8")
    lines = ["# W3-03 release evidence", "", f"Machine: {info['processor']}; {info['logical_processors']} logical processors; {info['os']}.", f"UTC start: {info['started_utc']}. HEAD baseline: `{info['head']}`; exact baseline source tree: `{info['baseline_source_tree']}`.", f"Compiler: {info['rustc']}. {info['profile']}. Variants: {', '.join(info['variants'])}.", "", "Both implementations run in the same binary, with identical fixture paths. Each sample restores pristine files outside the timer, extracts, validates canonical entries, injects, and validates BLAKE3 file trees outside the timer. Baseline report file ordering is canonicalized. Runs rotate variant order. CPU values are aggregate machine percentages from Windows GetSystemTimes; external CPU subtracts this process via GetProcessTimes. The runner completes its build before measurements; external activity is reflected in CPU samples. JSON metadata records SHA256 of every relevant baseline/candidate module, the worker helper, and the harness sources.", "", "Spread = 100 * (max - min) / median. Each row contains at least five samples; noisy shapes are rerun in full, keeping raw attempts. The final whole-shape attempt supersedes every earlier bucket for that shape; quiet buckets are never mixed between attempts. Timings in milliseconds.", "", "| Engine | Shape | Variant | Phase | Median | Min-max | Spread | CPU busy | External CPU | Attempt |", "|---|---|---|---|---:|---:|---:|---:|---:|---:|"]
    lines.insert(6, f"Benchmark child priority: {args.priority} (Windows class 0x{info['priority_class']:x}), verified at startup and in every sample. Builds and other processes keep their existing priorities.")
    for row in results:
        lines.append(f"| {row['engine']} | {row['shape']} | {row['variant']} | {row['phase']} | {row['median_ms']:.3f} | {row['min_ms']:.3f}-{row['max_ms']:.3f} | {row['spread_pct']:.1f}% | {row['cpu_busy_pct']:.1f}% | {row['external_cpu_pct']:.1f}% | {row['attempt']} |")
    noisy_results = [row for row in results if row["spread_pct"] > 15]
    if noisy_results:
        lines += ["", f"UNRESOLVED NOISE: {len(noisy_results)} rows remain above 15% after {args.max_attempts} attempts; do not use those rows to justify a default change."]
    args.output.with_suffix(".md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"Saved {args.output.with_suffix('.md')}; {len(noisy_results)} rows exceed 15% spread")
    if noisy_results:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
