#!/usr/bin/env python3
"""Bounded synthetic multilingual end-to-end QA for the Locust CLI.

This harness deliberately treats the CLI and its SQLite projects as black boxes.
It never repairs project rows or remaps source text after ``locust pivot``.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import copy
import datetime as dt
import hashlib
import json
import os
import re
import shutil
import sqlite3
import struct
import subprocess
import sys
import threading
import time
import traceback
import zipfile
from pathlib import Path
from typing import Any, Iterable


ENGINES = ("rpgmaker-mv", "html-game", "renpy", "unity", "unreal")
DEFAULT_TARGETS = ("es", "fr", "de", "pt-BR", "zh-CN")
EXPECTED_MIN = 8
EXPECTED_MAX = 12
REPORT_VERSION = 1

JA_STRINGS = (
    "星降る町",
    "新しい物語を始める",
    "冒険を続ける",
    "設定を変更する",
    "{player}さん、ようこそ！",
    "セーブデータがありません",
    "本当に終了しますか？",
    "はい、進みます",
    "いいえ、戻ります",
    "冒険を続ける",
)


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def json_write(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    temporary.replace(path)


def safe_name(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", value).strip("._") or "phase"


def tree_hashes(root: Path, *, exclude_locust: bool = True) -> dict[str, str]:
    result: dict[str, str] = {}
    if not root.exists():
        return result
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        rel = path.relative_to(root).as_posix()
        if exclude_locust and (rel == ".locust" or rel.startswith(".locust/")):
            continue
        result[rel] = sha256_file(path)
    return result


def tree_delta(before: dict[str, str], after: dict[str, str]) -> dict[str, Any]:
    shared = before.keys() & after.keys()
    return {
        "changed": sorted(key for key in shared if before[key] != after[key]),
        "added": sorted(after.keys() - before.keys()),
        "removed": sorted(before.keys() - after.keys()),
    }


def replace_tree(source: Path, destination: Path, owned_root: Path) -> None:
    owned_root = owned_root.resolve(strict=True)
    source = source.resolve(strict=True)
    destination = destination.resolve()
    if (
        not source.is_relative_to(owned_root)
        or not destination.is_relative_to(owned_root)
        or destination == owned_root
        or destination == source
        or source.is_relative_to(destination)
    ):
        raise ValueError("QA tree replacement must remain inside its owned output")
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(source, destination)


def fstring(value: str) -> bytes:
    if not value:
        return struct.pack("<i", 0)
    if value.isascii():
        payload = value.encode("ascii") + b"\0"
        return struct.pack("<i", len(payload)) + payload
    units = value.encode("utf-16-le") + b"\0\0"
    return struct.pack("<i", -(len(units) // 2)) + units


def write_unreal_locres(path: Path, strings: Iterable[str]) -> None:
    values = list(strings)
    data = bytearray(struct.pack("<i", 1))
    data.extend(fstring("LocustQA"))
    data.extend(struct.pack("<i", len(values)))
    for index, value in enumerate(values):
        data.extend(fstring(f"Entry{index:02d}"))
        data.extend(struct.pack("<I", 0))
        data.extend(fstring(value))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def aligned_string(value: str) -> bytes:
    payload = value.encode("utf-8")
    result = bytearray(struct.pack("<I", len(payload)))
    result.extend(payload)
    result.extend(b"\0" * ((-len(payload)) % 4))
    return bytes(result)


def unity_v17_textasset(name: str, script: str) -> bytes:
    """Mirror the repository's v17 TextAsset fixture writer."""
    text_payload = aligned_string(name) + aligned_string(script)
    other_payload = b"\0" * 32  # Non-text object, outside the authored Japanese corpus.

    meta = bytearray(b"2019.4.0f1\0")
    meta.extend(struct.pack("<I", 1))
    meta.append(0)  # enable_type_tree
    meta.extend(struct.pack("<i", 2))
    for class_id in (49, 1):
        meta.extend(struct.pack("<i", class_id))
        meta.append(0)
        meta.extend(struct.pack("<h", -1))
        meta.extend(b"\0" * 16)
    meta.extend(struct.pack("<i", 2))
    meta.extend(b"\0" * ((-len(meta)) % 4))
    meta.extend(struct.pack("<qIIi", 1, 0, len(text_payload), 0))
    meta.extend(b"\0" * ((-len(meta)) % 4))
    meta.extend(struct.pack("<qIIi", 2, len(text_payload), len(other_payload), 1))

    header_len = 20
    data_offset = (header_len + len(meta) + 15) & ~15
    file_size = data_offset + len(text_payload) + len(other_payload)
    header = struct.pack(">IIII", len(meta), file_size, 17, data_offset) + b"\0\0\0\0"
    return header + bytes(meta) + b"\0" * (data_offset - header_len - len(meta)) + text_payload + other_payload


def create_rpgmaker(root: Path) -> None:
    data_dir = root / "www" / "data"
    data_dir.mkdir(parents=True)
    (root / "www" / "js").mkdir(parents=True)
    (root / "www" / "js" / "rpg_core.js").write_text(
        "// Locust synthetic MV detection marker\n", encoding="utf-8"
    )
    commands: list[dict[str, Any]] = []
    for text in JA_STRINGS[1:7]:
        commands.extend(
            [
                {"code": 401, "indent": 0, "parameters": [text]},
                {"code": 0, "indent": 0, "parameters": []},
            ]
        )
    commands.extend(
        [
            {
                "code": 102,
                "indent": 0,
                "parameters": [[JA_STRINGS[7], JA_STRINGS[8]], -1, 0, 2, 0],
            },
            {"code": 401, "indent": 0, "parameters": [JA_STRINGS[9]]},
            {"code": 0, "indent": 0, "parameters": []},
        ]
    )
    game_map = {
        "displayName": JA_STRINGS[0],
        "events": [
            None,
            {
                "id": 1,
                "name": "LocustQA",
                "pages": [
                    {
                        "conditions": {},
                        "image": {},
                        "list": commands,
                        "moveRoute": {"list": [], "repeat": True, "skippable": False},
                    }
                ],
            },
        ],
        "data": [],
        "width": 1,
        "height": 1,
    }
    (data_dir / "Map001.json").write_text(
        json.dumps(game_map, ensure_ascii=False, separators=(",", ":")), encoding="utf-8"
    )
    (data_dir / "System.json").write_text(
        json.dumps({"gameTitle": ""}, ensure_ascii=False), encoding="utf-8"
    )


def create_html(root: Path) -> None:
    root.mkdir(parents=True)
    html = f"""<!doctype html>
<html lang="ja">
<head><meta charset="utf-8"><title>{JA_STRINGS[0]}</title>
<script>const untouched = "日本語のコード";</script></head>
<body>
<h1>{JA_STRINGS[1]}</h1>
<p>{JA_STRINGS[2]}</p>
<p>{JA_STRINGS[4]}</p>
<button onclick="continueGame()">{JA_STRINGS[2]}</button>
<button data-action="settings">{JA_STRINGS[3]}</button>
<input placeholder="{JA_STRINGS[5]}" aria-label="{JA_STRINGS[6]}">
<img src="star.png" alt="{JA_STRINGS[7]}">
<p>{JA_STRINGS[8]}</p>
<p>{JA_STRINGS[9]}</p>
</body></html>
"""
    (root / "index.html").write_text(html, encoding="utf-8")


def create_renpy(root: Path) -> None:
    game = root / "game"
    game.mkdir(parents=True)
    greeting = JA_STRINGS[4].replace("{player}", "[player]")
    script = f"""# Locust synthetic Ren'Py fixture
define a = Character("{JA_STRINGS[0]}")

label start:
    a "{JA_STRINGS[1]}"
    "{JA_STRINGS[2]}"
    a "{greeting}"
    a "{JA_STRINGS[2]}"
    menu:
        "{JA_STRINGS[7]}":
            pass
        "{JA_STRINGS[8]}":
            pass
    centered "{JA_STRINGS[5]}"
    $ renpy.notify("{JA_STRINGS[6]}")
    "{JA_STRINGS[9]}"
    return
"""
    (game / "script.rpy").write_text(script, encoding="utf-8")


def create_unity(root: Path) -> None:
    data = root / "LocustQA_Data"
    data.mkdir(parents=True)
    (root / "UnityPlayer.dll").write_bytes(b"Locust synthetic Unity detection marker\n")
    managed = "\n".join(
        f"QA.Entry{index:02d}: {text}" for index, text in enumerate(JA_STRINGS)
    )
    (data / "resources.assets").write_bytes(
        unity_v17_textasset("LocustQAMatrix", managed)
    )


def create_unreal(root: Path) -> None:
    locres = (
        root
        / "LocustQA"
        / "Content"
        / "Localization"
        / "Game"
        / "ja"
        / "Game.locres"
    )
    write_unreal_locres(locres, JA_STRINGS)


FIXTURE_BUILDERS = {
    "rpgmaker-mv": create_rpgmaker,
    "html-game": create_html,
    "renpy": create_renpy,
    "unity": create_unity,
    "unreal": create_unreal,
}


def encoded_length(encoding: str, text: str) -> int | None:
    if encoding == "utf8":
        return len(text.encode("utf-8"))
    if encoding == "utf16le":
        return len(text.encode("utf-16-le"))
    if encoding in {"sjis", "shift_jis", "shift-jis"}:
        try:
            return len(text.encode("shift_jis"))
        except UnicodeEncodeError:
            return None
    return None


def parse_injection_metrics(stdout: str, stderr: str) -> dict[str, Any]:
    """Parse the human CLI table conservatively; hashes remain the authority."""
    text = stdout + "\n" + stderr
    metrics: dict[str, Any] = {"skip_reasons": {}}
    labels = {
        "files_modified": r"Files modified",
        "strings_written": r"Strings written",
        "strings_skipped": r"Strings skipped",
    }
    for key, label in labels.items():
        match = re.search(label + r"\s*[|│]\s*(\d+)", text, re.IGNORECASE)
        metrics[key] = int(match.group(1)) if match else None
    for match in re.finditer(
        r"Skipped:\s*([^|│\r\n]+?)\s*[|│]\s*(\d+)", text, re.IGNORECASE
    ):
        metrics["skip_reasons"][match.group(1).strip()] = int(match.group(2))
    metrics["reported_omissions"] = metrics["strings_skipped"]
    metrics["too_long"] = metrics["skip_reasons"].get("too_long", 0)
    return metrics


def binary_oversize_against_original(
    original_entries: list[dict[str, Any]], target_entries: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    """Measure target translations against the real Japanese fixture slots."""
    originals = {item["id"]: item for item in original_entries}
    oversize = []
    for target in target_entries:
        original = originals.get(target["id"])
        translation = target.get("translation")
        if not original or not translation:
            continue
        if original.get("metadata", {}).get("extraction_method") in {"textasset_loc_line", "textasset_csv_cell"}:
            continue  # The rebuilt blob, not each individual cell, has a budget.
        encoding = original.get("metadata", {}).get("binary_slot")
        if not encoding:
            continue
        limit = encoded_length(encoding, original["source"])
        actual = encoded_length(encoding, translation)
        if limit is not None and actual is not None and actual > limit:
            oversize.append(
                {
                    "id": target["id"],
                    "encoding": encoding,
                    "limit": limit,
                    "actual": actual,
                }
            )
    return oversize


def db_snapshot(path: Path) -> dict[str, Any]:
    snapshot: dict[str, Any] = {
        "path": str(path),
        "exists": path.is_file(),
        "error": None,
        "counts": {},
        "runs": [],
        "entries": [],
        "injections": [],
        "binary_oversize": [],
    }
    if not path.is_file():
        snapshot["error"] = "database does not exist"
        return snapshot
    try:
        connection = sqlite3.connect(path)
        connection.row_factory = sqlite3.Row
        rows = connection.execute(
            "SELECT id, source, translation, status, file_path, context, tags, "
            "metadata, char_limit, provider_used FROM strings ORDER BY id"
        ).fetchall()
        by_status: dict[str, int] = {}
        translated = 0
        entries = []
        oversize = []
        for row in rows:
            item = dict(row)
            try:
                item["metadata"] = json.loads(item["metadata"] or "{}")
            except json.JSONDecodeError:
                item["metadata_parse_error"] = item["metadata"]
                item["metadata"] = {}
            try:
                item["tags"] = json.loads(item["tags"] or "[]")
            except json.JSONDecodeError:
                item["tags"] = []
            by_status[item["status"]] = by_status.get(item["status"], 0) + 1
            if item["translation"] is not None and str(item["translation"]).strip():
                translated += 1
                encoding = item["metadata"].get("binary_slot")
                if encoding and item["metadata"].get("extraction_method") not in {"textasset_loc_line", "textasset_csv_cell"}:
                    capacity = item["metadata"].get("locust_injection_capacity", {})
                    source_len = (
                        capacity.get("bytes")
                        if isinstance(capacity, dict)
                        and capacity.get("encoding") == encoding
                        and isinstance(capacity.get("bytes"), int)
                        else encoded_length(
                            encoding,
                            item["metadata"].get("locust_injection_source", item["source"]),
                        )
                    )
                    target_len = encoded_length(encoding, item["translation"])
                    if (
                        source_len is not None
                        and target_len is not None
                        and target_len > source_len
                    ):
                        oversize.append(
                            {
                                "id": item["id"],
                                "encoding": encoding,
                                "limit": source_len,
                                "actual": target_len,
                            }
                        )
            entries.append(item)
        snapshot["entries"] = entries
        snapshot["counts"] = {
            "total": len(rows),
            "translated_nonempty": translated,
            "pending": by_status.get("pending", 0),
            "by_status": by_status,
        }
        columns = {
            row[1]
            for row in connection.execute("PRAGMA table_info(translation_runs)").fetchall()
        }
        cost_column = "cost_is_complete" if "cost_is_complete" in columns else "0"
        snapshot["runs"] = [
            dict(row)
            for row in connection.execute(
                "SELECT id, started_at, duration_secs, provider, source_lang, "
                "target_lang, strings_translated, tokens_used, input_tokens, "
                f"output_tokens, cost_usd, {cost_column} AS cost_is_complete "
                "FROM translation_runs ORDER BY id"
            ).fetchall()
        ]
        if {
            row[1]
            for row in connection.execute("PRAGMA table_info(injected_files)").fetchall()
        }:
            snapshot["injections"] = [
                dict(row)
                for row in connection.execute(
                    "SELECT lang, root, rel, hash, size, recorded_at "
                    "FROM injected_files ORDER BY id"
                ).fetchall()
            ]
        snapshot["binary_oversize"] = oversize
        connection.close()
    except Exception as error:  # report malformed/locked DB instead of hiding it
        snapshot["error"] = f"{type(error).__name__}: {error}"
    return snapshot


class Harness:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.output = args.output
        self.binary = self.output / "frozen" / args.locust.name
        self.config = args.config
        self.report: dict[str, Any] = {
            "schema_version": REPORT_VERSION,
            "started_at": utc_now(),
            "finished_at": None,
            "mode": "prepare-only" if args.prepare_only else "full",
            "arguments": {
                "locust": str(args.locust),
                "config": str(args.config),
                "output": str(args.output),
                "provider": args.provider,
                "targets": list(args.targets),
                "workers": args.workers,
                "request_concurrency": args.request_concurrency,
                "aggregate_request_cap": args.workers * args.request_concurrency,
                "timeout_seconds": args.timeout,
            },
            "frozen": {},
            "engines": {},
            "direct_comparison": {},
            "failures": [],
            "warnings": [],
        }
        self.lock = threading.Lock()

    def save(self) -> None:
        with self.lock:
            # Worker threads own separate engine lanes but may finish an
            # assignment while another lane snapshots the aggregate report.
            for attempt in range(10):
                try:
                    snapshot = copy.deepcopy(self.report)
                    break
                except RuntimeError:
                    if attempt == 9:
                        raise
                    time.sleep(0.01)
            json_write(self.output / "report.json", snapshot)

    def fail(self, scope: str, message: str) -> None:
        with self.lock:
            self.report["failures"].append({"scope": scope, "message": message})
        self.save()

    def warn(self, scope: str, message: str) -> None:
        with self.lock:
            self.report["warnings"].append({"scope": scope, "message": message})
        self.save()

    def initialize(self) -> None:
        self.output.mkdir(parents=True)
        (self.output / "logs").mkdir()
        (self.output / "fixtures").mkdir()
        (self.output / "work").mkdir()
        (self.output / "projects").mkdir()
        (self.output / "patches").mkdir()
        (self.output / "consumers").mkdir()
        (self.output / "backups").mkdir()
        self.binary.parent.mkdir()
        shutil.copy2(self.args.locust, self.binary)
        self.report["frozen"] = {
            "source_executable": str(self.args.locust),
            "source_sha256": sha256_file(self.args.locust),
            "executable": str(self.binary),
            "sha256": sha256_file(self.binary),
            "config_path": str(self.config),
            "config_sha256_before": sha256_file(self.config),
            "config_copied": False,
            "note": "La configuración se usa en solo lectura y no se copia porque puede contener secretos.",
        }
        self.save()

    def command(
        self,
        scope: str,
        phase: str,
        arguments: list[str],
        *,
        timeout: int | None = None,
    ) -> dict[str, Any]:
        command = [
            str(self.binary),
            "--config",
            str(self.config),
            *map(str, arguments),
        ]
        started_at = utc_now()
        started = time.monotonic()
        stdout = ""
        stderr = ""
        exit_code: int | None = None
        timed_out = False
        error_text = None
        try:
            process = subprocess.run(
                command,
                cwd=self.output,
                env={
                    **os.environ,
                    "LOCUST_BACKUP_ROOT": str(self.output / "backups"),
                },
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=timeout or self.args.timeout,
                check=False,
            )
            stdout = process.stdout
            stderr = process.stderr
            exit_code = process.returncode
        except subprocess.TimeoutExpired as error:
            timed_out = True
            stdout = error.stdout or ""
            stderr = error.stderr or ""
            if isinstance(stdout, bytes):
                stdout = stdout.decode("utf-8", "replace")
            if isinstance(stderr, bytes):
                stderr = stderr.decode("utf-8", "replace")
            error_text = f"timeout after {timeout or self.args.timeout}s"
        except Exception as error:
            error_text = f"{type(error).__name__}: {error}"
        duration = time.monotonic() - started
        stem = safe_name(f"{scope}__{phase}")
        stdout_path = self.output / "logs" / f"{stem}.stdout.txt"
        stderr_path = self.output / "logs" / f"{stem}.stderr.txt"
        record_path = self.output / "logs" / f"{stem}.json"
        stdout_path.write_text(stdout, encoding="utf-8")
        stderr_path.write_text(stderr, encoding="utf-8")
        record = {
            "scope": scope,
            "phase": phase,
            "command": command,
            "started_at": started_at,
            "duration_seconds": duration,
            "exit_code": exit_code,
            "timed_out": timed_out,
            "error": error_text,
            "stdout": str(stdout_path),
            "stderr": str(stderr_path),
            "ok": exit_code == 0 and not timed_out and error_text is None,
        }
        json_write(record_path, record)
        return record

    def create_fixtures(self) -> None:
        for engine, builder in FIXTURE_BUILDERS.items():
            root = self.output / "fixtures" / engine
            builder(root)
            hashes = tree_hashes(root)
            manifest = {
                "engine": engine,
                "synthetic": True,
                "runtime_verified": False,
                "expected_entries": f"{EXPECTED_MIN}-{EXPECTED_MAX}",
                "files": hashes,
                "created_at": utc_now(),
            }
            json_write(root / "fixture-manifest.json", manifest)
            # The manifest is QA metadata, not part of the game fixture.
            self.report["engines"][engine] = {
                "fixture": str(root),
                "fixture_hashes": tree_hashes(root),
                "phases": {},
                "targets": {},
                "status": "preparing",
            }
        self.save()

    def extract_engine(self, engine: str) -> bool:
        state = self.report["engines"][engine]
        template = self.output / "fixtures" / engine
        game = self.output / "work" / engine / "game"
        replace_tree(template, game, self.output)
        # QA metadata must never be packed as a game artifact.
        (game / "fixture-manifest.json").unlink(missing_ok=True)
        database = self.output / "projects" / engine / "ja-en.locust.db"
        database.parent.mkdir(parents=True, exist_ok=True)
        command = self.command(
            engine,
            "extract-ja",
            ["extract", str(game), "-o", str(database)],
        )
        snapshot = db_snapshot(database)
        json_write(database.with_suffix(".snapshot.json"), snapshot)
        count = snapshot.get("counts", {}).get("total", 0)
        ok = command["ok"] and snapshot["error"] is None and EXPECTED_MIN <= count <= EXPECTED_MAX
        state["game_path"] = str(game)
        state["database"] = str(database)
        state["phases"]["extract"] = {
            "command": command,
            "database": snapshot,
            "entry_count_in_expected_range": EXPECTED_MIN <= count <= EXPECTED_MAX,
            "ok": ok,
        }
        state["status"] = "prepared" if ok else "failed"
        if not ok:
            self.fail(
                engine,
                f"extracción falló o produjo {count} entradas; se esperaban {EXPECTED_MIN}-{EXPECTED_MAX}",
            )
        self.save()
        return ok

    def translate(
        self,
        scope: str,
        phase: str,
        database: Path,
        source: str,
        target: str,
        engine: str,
    ) -> tuple[dict[str, Any], dict[str, Any], bool]:
        context = (
            f"Locust synthetic {engine} QA. Translate only from {source} to {target}; "
            "preserve placeholders, line structure, names, and UI intent."
        )
        command = self.command(
            scope,
            phase,
            [
                "translate",
                str(database),
                "-p",
                self.args.provider,
                "-s",
                source,
                "-t",
                target,
                "--batch-size",
                "4",
                "--concurrency",
                str(self.args.request_concurrency),
                "--no-memory",
                "--context",
                context,
            ],
        )
        snapshot = db_snapshot(database)
        json_write(
            database.with_name(database.name + f".{safe_name(target)}.snapshot.json"),
            snapshot,
        )
        counts = snapshot.get("counts", {})
        runs = snapshot.get("runs", [])
        matching_runs = [
            run
            for run in runs
            if run["source_lang"].casefold() == source.casefold()
            and run["target_lang"].casefold() == target.casefold()
        ]
        complete = (
            counts.get("total", 0) > 0
            and counts.get("translated_nonempty") == counts.get("total")
            and counts.get("pending", 0) == 0
        )
        ok = command["ok"] and snapshot["error"] is None and complete and bool(matching_runs)
        if matching_runs:
            last = matching_runs[-1]
            if not last.get("cost_is_complete"):
                self.warn(scope, f"{phase}: el proveedor no informó costo completo")
            if (
                last.get("tokens_used", 0) == 0
                and last.get("input_tokens", 0) == 0
                and last.get("output_tokens", 0) == 0
            ):
                self.warn(scope, f"{phase}: el proveedor no informó tokens")
        return command, snapshot, ok

    def validate(
        self, scope: str, phase: str, database: Path
    ) -> tuple[dict[str, Any], bool]:
        command = self.command(scope, phase, ["validate", str(database)])
        if command["exit_code"] == 1:
            output = Path(command["stdout"]).read_text(encoding="utf-8")
            kinds = []
            for line in output.splitlines():
                fields = [field.strip() for field in line.split("|")[1:-1]]
                if len(fields) == 3 and fields[1] != "Kind":
                    kinds.append(fields[1])
            if kinds and all(kind == "IdenticalToSource" for kind in kinds):
                command["identity_warnings"] = len(kinds)
                self.warn(scope, f"{phase}: {len(kinds)} textos idénticos; revisión lingüística pendiente")
                return command, True
        return command, bool(command["ok"])

    def pivot_target(self, engine: str, target: str, ja_en_db: Path) -> dict[str, Any]:
        scope = f"{engine}-{target}"
        result: dict[str, Any] = {"status": "running", "phases": {}, "ok": False}
        target_dir = self.output / "projects" / engine / safe_name(target)
        target_dir.mkdir(parents=True, exist_ok=True)
        database = target_dir / "en-target.locust.db"

        pivot = self.command(
            scope, "pivot-en", ["pivot", str(ja_en_db), "-o", str(database)]
        )
        pivot_snapshot = db_snapshot(database)
        result["phases"]["pivot"] = {"command": pivot, "database": pivot_snapshot}
        if not pivot["ok"] or pivot_snapshot["error"]:
            result["status"] = "blocked:pivot"
            self.fail(scope, "pivot EN falló; traducción/inyección bloqueadas")
            return result
        source_snapshot = db_snapshot(ja_en_db)
        en_translations = {
            item["id"]: item["translation"]
            for item in source_snapshot["entries"]
            if item["translation"] is not None
        }
        pivot_sources = {item["id"]: item["source"] for item in pivot_snapshot["entries"]}
        pivot_exact = pivot_sources == en_translations
        result["phases"]["pivot"]["sources_equal_ja_en_translations"] = pivot_exact
        original_sources = {item["id"]: item["source"] for item in source_snapshot["entries"]}
        result["phases"]["pivot"]["japanese_source_provenance_present"] = all(
            item["metadata"].get("locust_injection_source") == original_sources[item["id"]]
            for item in pivot_snapshot["entries"]
        )
        if not result["phases"]["pivot"]["japanese_source_provenance_present"]:
            self.warn(
                scope,
                "pivot no conserva procedencia japonesa explícita; no se remapea por SQL",
            )
        if not pivot_exact:
            result["status"] = "blocked:pivot-integrity"
            self.fail(scope, "las fuentes pivotadas no coinciden con las traducciones JA→EN")
            return result

        command, translated, translated_ok = self.translate(
            scope, f"translate-en-{target}", database, "en", target, engine
        )
        actual_fixture_oversize = binary_oversize_against_original(
            source_snapshot["entries"], translated["entries"]
        )
        if translated["binary_oversize"] and not actual_fixture_oversize:
            self.warn(
                scope,
                "el validador marca exceso contra la fuente inglesa pivotada, "
                "pero las traducciones caben en los slots japoneses reales",
            )
        result["phases"]["translate"] = {
            "command": command,
            "database": translated,
            "actual_fixture_binary_oversize": actual_fixture_oversize,
            "ok": translated_ok,
        }
        if not translated_ok:
            result["status"] = "blocked:translate"
            self.fail(scope, "traducción EN→destino incompleta; validación/inyección bloqueadas")
            return result

        validation, validation_ok = self.validate(
            scope, f"validate-{target}", database
        )
        result["phases"]["validate"] = {
            "command": validation,
            "ok": validation_ok,
            "pivot_source_binary_oversize": translated["binary_oversize"],
            "actual_fixture_binary_oversize": actual_fixture_oversize,
        }
        if not validation_ok:
            result["status"] = "blocked:validate"
            self.fail(scope, "validación falló; inyección bloqueada")
            return result

        template = self.output / "fixtures" / engine
        game = self.output / "work" / engine / "game"
        replace_tree(template, game, self.output)
        (game / "fixture-manifest.json").unlink(missing_ok=True)
        before = tree_hashes(game)
        inject = self.command(
            scope,
            f"inject-{target}",
            ["inject", str(game), "-P", str(database), "--direct", "-l", target],
        )
        after = tree_hashes(game)
        delta = tree_delta(before, after)
        inject_metrics = parse_injection_metrics(
            Path(inject["stdout"]).read_text(encoding="utf-8"),
            Path(inject["stderr"]).read_text(encoding="utf-8"),
        )
        injection_rows = [
            row for row in db_snapshot(database)["injections"] if row["lang"] == target
        ]
        expected_writes = sum(
            item["translation"] != original_sources[item["id"]]
            for item in translated["entries"]
        )
        coverage_ok = (
            inject["ok"]
            and (inject_metrics["strings_written"] or 0) > 0
            and inject_metrics["strings_written"] == expected_writes
            and bool(delta["changed"] or delta["added"] or delta["removed"])
            and bool(injection_rows)
        )
        result["phases"]["inject"] = {
            "command": inject,
            "hashes_before": before,
            "hashes_after": after,
            "delta": delta,
            "reported_metrics": inject_metrics,
            "recorded_files": injection_rows,
            "translated_entries": translated["counts"].get("translated_nonempty", 0),
            "pivot_source_binary_oversize": translated["binary_oversize"],
            "actual_fixture_binary_oversize": actual_fixture_oversize,
            "omissions": {
                "reported_total": inject_metrics["reported_omissions"],
                "too_long": inject_metrics["too_long"],
                "binary_oversize_against_pivot_source": len(
                    translated["binary_oversize"]
                ),
                "binary_oversize_against_actual_fixture": len(
                    actual_fixture_oversize
                ),
                "other_skip_reasons": {
                    key: value
                    for key, value in inject_metrics["skip_reasons"].items()
                    if key != "too_long"
                },
            },
            "expected_writes": expected_writes,
            "coverage_ok": coverage_ok,
        }
        if not coverage_ok:
            result["status"] = "blocked:inject-coverage"
            self.fail(
                scope,
                "inyección sin cobertura comprobable (exit 0 no basta); pack/apply bloqueados",
            )
            return result

        patch = self.output / "patches" / engine / f"{safe_name(target)}.zip"
        patch.parent.mkdir(parents=True, exist_ok=True)
        pack = self.command(
            scope,
            f"pack-{target}",
            [
                "patch",
                str(game),
                "-P",
                str(database),
                "-l",
                target,
                "-o",
                str(patch),
                "--pristine",
                str(template),
            ],
        )
        strict = self.inspect_patch(patch)
        pack_ok = pack["ok"] and strict.get("strict", False)
        result["phases"]["pack"] = {
            "command": pack,
            "zip": str(patch),
            "inspection": strict,
            "ok": pack_ok,
        }
        if not pack_ok:
            result["status"] = "blocked:pack"
            self.fail(scope, "ZIP estricto no se produjo; apply bloqueado")
            return result

        consumer = self.output / "consumers" / engine / safe_name(target)
        replace_tree(template, consumer, self.output)
        (consumer / "fixture-manifest.json").unlink(missing_ok=True)
        pristine = tree_hashes(consumer)
        apply = self.command(
            scope, f"apply-{target}", ["apply", str(consumer), str(patch)]
        )
        applied = tree_hashes(consumer)
        apply_delta = tree_delta(pristine, applied)
        payload_matches = all(
            applied.get(rel) == after.get(rel)
            for rel in strict.get("payload_files", [])
        )
        apply_ok = (
            apply["ok"]
            and bool(apply_delta["changed"] or apply_delta["added"])
            and payload_matches
        )
        result["phases"]["apply"] = {
            "command": apply,
            "hashes_before": pristine,
            "hashes_after": applied,
            "delta": apply_delta,
            "payload_matches_injected_hashes": payload_matches,
            "ok": apply_ok,
        }
        if not apply_ok:
            result["status"] = "blocked:apply"
            self.fail(scope, "apply no reprodujo los bytes inyectados; rollback bloqueado")
            return result

        rollback = self.command(
            scope, f"rollback-{target}", ["patch-rollback", str(consumer)]
        )
        rolled_back = tree_hashes(consumer)
        rollback_ok = rollback["ok"] and rolled_back == pristine
        result["phases"]["rollback"] = {
            "command": rollback,
            "hashes_after": rolled_back,
            "pristine_hashes_restored": rolled_back == pristine,
            "ok": rollback_ok,
        }
        if not rollback_ok:
            result["status"] = "failed:rollback"
            self.fail(scope, "rollback no restauró hashes prístinos")
            return result

        reextract_db = target_dir / "reextract.locust.db"
        reextract = self.command(
            scope,
            f"reextract-{target}",
            ["extract", str(game), "-o", str(reextract_db)],
        )
        reextracted = db_snapshot(reextract_db)
        target_values = {
            item["translation"]
            for item in translated["entries"]
            if item["translation"] is not None
        }
        re_sources = {item["source"] for item in reextracted.get("entries", [])}
        # Unity intentionally preserves fixed-slot padding. RPG Maker reflows
        # message blocks at word boundaries; these are representation changes,
        # not lost words. Retain raw snapshots and match exact text first.
        if engine == "unity":
            re_sources.update(item["source"].rstrip(" ") for item in reextracted.get("entries", []))
        elif engine == "rpgmaker-mv":
            re_sources.update(item["source"].replace("\n", " ") for item in reextracted.get("entries", []))
        matches = sorted(value for value in target_values if value in re_sources)
        missing_values = sorted(target_values - re_sources)
        reextract_ok = reextract["ok"] and bool(matches) and not missing_values
        result["phases"]["reextract"] = {
            "command": reextract,
            "database": reextracted,
            "target_translation_matches": matches,
            "missing_target_values": missing_values,
            "proves_translated_bytes": reextract_ok,
            "ok": reextract_ok,
        }
        if not reextract_ok:
            self.fail(scope, "re-extracción no recuperó todas las traducciones destino exactas")
            result["status"] = "failed:reextract"
            return result
        result["status"] = "passed"
        result["ok"] = True
        return result

    def inspect_patch(self, patch: Path) -> dict[str, Any]:
        result: dict[str, Any] = {
            "exists": patch.is_file(),
            "strict": False,
            "payload_files": [],
            "error": None,
        }
        if not patch.is_file():
            result["error"] = "zip missing"
            return result
        try:
            with zipfile.ZipFile(patch) as archive:
                names = archive.namelist()
                manifest = json.loads(archive.read("locust-patch.json"))
                files = manifest.get("files", [])
                payload = [item.get("path") for item in files if item.get("path")]
                result.update(
                    {
                        "sha256": sha256_file(patch),
                        "entries": names,
                        "manifest": manifest,
                        "payload_files": payload,
                        "strict": bool(files)
                        and all(item.get("original_sha256") for item in files),
                    }
                )
        except Exception as error:
            result["error"] = f"{type(error).__name__}: {error}"
        return result

    def engine_pipeline(self, engine: str) -> None:
        state = self.report["engines"][engine]
        if not state["phases"]["extract"]["ok"]:
            return
        database = Path(state["database"])
        command, snapshot, translated_ok = self.translate(
            engine, "translate-ja-en", database, "ja", "en", engine
        )
        state["phases"]["translate_ja_en"] = {
            "command": command,
            "database": snapshot,
            "ok": translated_ok,
        }
        if not translated_ok:
            state["status"] = "blocked:translate-ja-en"
            self.fail(engine, "traducción JA→EN incompleta; fases dependientes bloqueadas")
            return
        validation, validation_ok = self.validate(engine, "validate-en", database)
        state["phases"]["validate_en"] = {"command": validation, "ok": validation_ok}
        if not validation_ok:
            state["status"] = "blocked:validate-en"
            self.fail(engine, "validación inglesa falló; targets bloqueados")
            return
        for target in self.args.targets:
            try:
                state["targets"][target] = self.pivot_target(engine, target, database)
            except Exception as error:
                state["targets"][target] = {
                    "status": "internal-error",
                    "ok": False,
                    "error": f"{type(error).__name__}: {error}",
                    "traceback": traceback.format_exc(),
                }
                self.fail(f"{engine}-{target}", f"error interno: {error}")
            self.save()
        state["status"] = (
            "passed"
            if state["targets"]
            and all(item.get("ok") for item in state["targets"].values())
            else "failed"
        )
        self.save()

    def direct_comparison(self) -> None:
        scope = "comparison-rpgmaker-mv-ja-es"
        root = self.output / "direct-comparison"
        template = self.output / "fixtures" / "rpgmaker-mv"
        game = root / "game"
        replace_tree(template, game, self.output)
        (game / "fixture-manifest.json").unlink(missing_ok=True)
        database = root / "ja-es.locust.db"
        result: dict[str, Any] = {"phases": {}, "status": "running", "ok": False}
        extract = self.command(
            scope, "extract-ja", ["extract", str(game), "-o", str(database)]
        )
        extracted = db_snapshot(database)
        result["phases"]["extract"] = {"command": extract, "database": extracted}
        if not extract["ok"]:
            result["status"] = "blocked:extract"
            self.fail(scope, "extracción de comparación falló")
            self.report["direct_comparison"] = result
            return
        command, translated, ok = self.translate(
            scope, "translate-ja-es", database, "ja", "es", "rpgmaker-mv"
        )
        result["phases"]["translate"] = {
            "command": command,
            "database": translated,
            "ok": ok,
        }
        if not ok:
            result["status"] = "blocked:translate"
            self.fail(scope, "traducción directa JA→ES falló")
            self.report["direct_comparison"] = result
            return
        validation, valid = self.validate(scope, "validate-es", database)
        result["phases"]["validate"] = {"command": validation, "ok": valid}
        if not valid:
            result["status"] = "blocked:validate"
            self.fail(scope, "validación directa JA→ES falló")
            self.report["direct_comparison"] = result
            return
        before = tree_hashes(game)
        inject = self.command(
            scope,
            "inject-es",
            ["inject", str(game), "-P", str(database), "--direct", "-l", "es"],
        )
        after = tree_hashes(game)
        delta = tree_delta(before, after)
        metrics = parse_injection_metrics(
            Path(inject["stdout"]).read_text(encoding="utf-8"),
            Path(inject["stderr"]).read_text(encoding="utf-8"),
        )
        coverage = (
            inject["ok"]
            and (metrics["strings_written"] or 0) > 0
            and bool(delta["changed"] or delta["added"])
        )
        result["phases"]["inject"] = {
            "command": inject,
            "delta": delta,
            "reported_metrics": metrics,
            "coverage_ok": coverage,
        }
        result["status"] = "passed" if coverage else "failed:inject-coverage"
        result["ok"] = coverage
        if not coverage:
            self.fail(scope, "comparación directa no modificó archivos")
        self.report["direct_comparison"] = result
        self.save()

    def write_spanish_report(self) -> None:
        lines = [
            "# Matriz QA multilingüe de Locust",
            "",
            f"- Inicio: `{self.report['started_at']}`",
            f"- Fin: `{self.report.get('finished_at')}`",
            f"- Modo: `{self.report['mode']}`",
            f"- Proveedor: `{self.args.provider}`",
            f"- Destinos: `{', '.join(self.args.targets)}`",
            f"- Límite agregado: `{self.args.workers * self.args.request_concurrency}` solicitudes simultáneas",
            f"- Fallos: `{len(self.report['failures'])}`",
            f"- Advertencias: `{len(self.report['warnings'])}`",
            "",
            "## Resultado por motor",
            "",
            "| Motor | Estado | Entradas | Destinos aprobados |",
            "|---|---|---:|---:|",
        ]
        for engine in ENGINES:
            item = self.report["engines"].get(engine, {})
            count = (
                item.get("phases", {})
                .get("extract", {})
                .get("database", {})
                .get("counts", {})
                .get("total", 0)
            )
            targets = item.get("targets", {})
            passed = sum(1 for value in targets.values() if value.get("ok"))
            lines.append(
                f"| {engine} | {item.get('status', 'sin ejecutar')} | {count} | {passed}/{len(self.args.targets)} |"
            )
        lines.extend(
            [
                "",
                "## Fallos",
                "",
            ]
        )
        if self.report["failures"]:
            lines.extend(
                f"- **{item['scope']}**: {item['message']}"
                for item in self.report["failures"]
            )
        else:
            lines.append("- Ninguno.")
        lines.extend(
            [
                "",
                "## Advertencias",
                "",
            ]
        )
        if self.report["warnings"]:
            lines.extend(
                f"- **{item['scope']}**: {item['message']}"
                for item in self.report["warnings"]
            )
        else:
            lines.append("- Ninguna.")
        lines.extend(
            [
                "",
                "## Limitaciones",
                "",
                "- Los artefactos son sintéticos y no verifican que un juego comercial arranque ni renderice correctamente.",
                "- La matriz no corrige SQLite ni restaura procedencia japonesa perdida por `pivot`; esos fallos se reportan como fallos de producción.",
                "- `--prepare-only` y `--dry-run` generan fixtures y ejecutan extracción local, pero nunca llaman a un proveedor.",
                "- Los stdout, stderr, códigos de salida, snapshots SQLite, hashes y manifiestos ZIP están en este directorio.",
                "- La configuración indicada no se copia porque puede contener secretos. El CLI puede renovar su token OAuth según su comportamiento normal; la matriz comprueba que el archivo `--config` no cambie.",
                "",
            ]
        )
        (self.output / "REPORT.md").write_text("\n".join(lines), encoding="utf-8")

    def finish(self) -> int:
        config_after = sha256_file(self.config)
        self.report["frozen"]["config_sha256_after"] = config_after
        self.report["frozen"]["config_unchanged"] = (
            config_after == self.report["frozen"]["config_sha256_before"]
        )
        self.report["frozen"]["frozen_executable_unchanged"] = (
            sha256_file(self.binary) == self.report["frozen"]["sha256"]
        )
        if not self.report["frozen"]["config_unchanged"]:
            self.fail("config", "el archivo de configuración cambió durante la ejecución")
        if not self.report["frozen"]["frozen_executable_unchanged"]:
            self.fail("binary", "la copia congelada del ejecutable cambió")
        self.report["finished_at"] = utc_now()
        self.save()
        self.write_spanish_report()
        return 1 if self.report["failures"] else 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Matriz QA sintética JA->EN->múltiples idiomas para Locust."
    )
    parser.add_argument("--locust", required=True, type=Path, help="Ejecutable locust")
    parser.add_argument("--config", required=True, type=Path, help="Config JSON de Locust")
    parser.add_argument("--output", required=True, type=Path, help="Directorio nuevo de salida")
    parser.add_argument("--provider", default="grok-sub")
    parser.add_argument("--targets", default=",".join(DEFAULT_TARGETS))
    parser.add_argument("--workers", type=int, default=5)
    parser.add_argument("--request-concurrency", type=int, default=2)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument(
        "--prepare-only",
        action="store_true",
        help="Solo fixtures y extracción; cero llamadas a proveedores",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Alias de --prepare-only",
    )
    args = parser.parse_args(argv)
    args.locust = args.locust.expanduser().resolve()
    args.config = args.config.expanduser().resolve()
    args.output = args.output.expanduser().resolve()
    args.prepare_only = args.prepare_only or args.dry_run
    args.targets = tuple(
        item.strip() for item in args.targets.split(",") if item.strip()
    )
    if not args.locust.is_file():
        parser.error(f"--locust no es un archivo: {args.locust}")
    if not args.config.is_file():
        parser.error(f"--config no es un archivo: {args.config}")
    if args.output.exists():
        parser.error("--output debe ser un directorio nuevo; no se sobrescribe ni reanuda")
    if not args.targets:
        parser.error("--targets requiere al menos un idioma")
    if len(set(map(str.casefold, args.targets))) != len(args.targets):
        parser.error("--targets contiene idiomas duplicados")
    if args.workers < 1 or args.request_concurrency < 1:
        parser.error("--workers y --request-concurrency deben ser positivos")
    if args.workers * args.request_concurrency > 10:
        parser.error("--workers × --request-concurrency no puede exceder 10")
    if not 300 <= args.timeout <= 900:
        parser.error("--timeout debe estar entre 300 y 900 segundos")
    return args


def main(argv: list[str] | None = None) -> int:
    # argparse otherwise inherits legacy cp1252 on some Windows terminals and
    # can crash while printing Spanish help.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    args = parse_args(argv if argv is not None else sys.argv[1:])
    harness = Harness(args)
    try:
        harness.initialize()
        harness.create_fixtures()
        with concurrent.futures.ThreadPoolExecutor(
            max_workers=min(args.workers, len(ENGINES))
        ) as executor:
            extracted = {
                executor.submit(harness.extract_engine, engine): engine
                for engine in ENGINES
            }
            for future, engine in extracted.items():
                try:
                    future.result()
                except Exception as error:
                    harness.report["engines"][engine]["status"] = "internal-error"
                    harness.fail(engine, f"error interno de extracción: {error}")
        if not args.prepare_only:
            with concurrent.futures.ThreadPoolExecutor(
                max_workers=min(args.workers, len(ENGINES))
            ) as executor:
                futures = {
                    executor.submit(harness.engine_pipeline, engine): engine
                    for engine in ENGINES
                }
                for future, engine in futures.items():
                    try:
                        future.result()
                    except Exception as error:
                        harness.report["engines"][engine]["status"] = "internal-error"
                        harness.fail(
                            engine,
                            f"error interno de pipeline: {error}\n{traceback.format_exc()}",
                        )
            harness.direct_comparison()
    except KeyboardInterrupt:
        harness.fail("harness", "interrumpido por el usuario")
    except Exception as error:
        if harness.output.exists():
            harness.fail("harness", f"error fatal: {type(error).__name__}: {error}")
            harness.report["fatal_traceback"] = traceback.format_exc()
        else:
            print(f"fatal: {error}", file=sys.stderr)
            return 2
    return harness.finish()


if __name__ == "__main__":
    raise SystemExit(main())
