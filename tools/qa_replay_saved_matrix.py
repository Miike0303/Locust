#!/usr/bin/env python3
"""Replay saved translations through production CLI; never request translation."""
from __future__ import annotations

import argparse
import concurrent.futures
from contextlib import closing
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import time
import traceback
import xml.etree.ElementTree as ET
import zipfile
from qa_replay_oracle import assert_reextracted, normalized as normalize_root

sys.dont_write_bytecode = True
HELPER = Path(__file__).resolve().parent / "qa_multilingual_matrix.py"
spec = importlib.util.spec_from_file_location("matrix_reference", HELPER)
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)
ENGINES = qa.ENGINES
TARGETS = qa.DEFAULT_TARGETS
ALLOWED = {"extract", "export", "import", "pivot", "validate", "inject", "inject-status", "patch", "apply", "patch-rollback", "patch-status"}
NS = "urn:oasis:names:tc:xliff:document:1.2"
ET.register_namespace("", NS)


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def game_hashes(root):
    """Compare game assets separately from a recognized injection journal.

    Unknown similarly named data is never silently ignored. The CLI status
    check additionally requires a closed transaction before packing.
    """
    hashes = qa.tree_hashes(root)
    store = root / ".locust-injections"
    if not store.exists():
        return hashes
    marker = json.loads((store / "store.json").read_text(encoding="utf-8"))
    require(marker.get("schema_version") == 1 and marker.get("kind") == "locust-injection-store", "unrecognized injection store")
    require(normalize_root(marker["game_root"]).lower() == normalize_root(root.resolve()).lower(), "injection store belongs to another root")
    return {name: digest for name, digest in hashes.items() if not name.startswith(".locust-injections/")}


def snapshot(path):
    # Reference snapshot helper is only ever used on an owned SQLite backup.
    result = qa.db_snapshot(path)
    require(result["error"] is None, f"invalid owned database: {result['error']}")
    return result


def backup_frozen_db(source: Path, destination: Path, stage: Path):
    """No SQLite connection touches source, including its shared-memory file.

    Copy stable main+WAL bytes to owned staging, let SQLite reconstruct its own
    WAL index there, and use SQLite backup into the independently owned DB.
    """
    require(source.is_file(), f"saved database missing: {source}")
    stage.parent.mkdir(parents=True, exist_ok=True)
    destination.parent.mkdir(parents=True, exist_ok=True)
    inputs = [source] + ([Path(str(source) + "-wal")] if Path(str(source) + "-wal").exists() else [])
    before = {str(p): qa.sha256_file(p) for p in inputs}
    shutil.copyfile(source, stage)
    if len(inputs) == 2:
        shutil.copyfile(inputs[1], Path(str(stage) + "-wal"))
    require(before == {str(p): qa.sha256_file(p) for p in inputs}, "frozen SQLite main/WAL changed while copying")
    with closing(sqlite3.connect(stage)) as src, closing(sqlite3.connect(destination)) as dest:
        require(src.execute("PRAGMA integrity_check").fetchone()[0] == "ok", "staged SQLite integrity check failed")
        src.backup(dest)
    return {"source": str(source), "source_hashes": before, "backup": str(destination), "stage": str(stage), "wal_safe": True}


def normalized(value):
    return str(value).replace("\\", "/").rstrip("/")


def relative(value, root):
    value, root = normalized(value), normalized(root)
    require(value.lower().startswith(root.lower() + "/"), f"path outside declared saved/new root: {value}")
    rel = value[len(root) + 1:]
    require(all(part not in ("", ".", "..") for part in rel.split("/")), f"unsafe relative identity: {rel}")
    return rel


def identity(entry, root):
    value, prefix = normalized(entry["id"]), normalized(root)
    if value.lower().startswith(prefix.lower() + "/"):
        value = value[len(prefix) + 1:]
    return (relative(entry["file_path"], root), value)


def indexed(entries, root):
    result = {}
    for entry in entries:
        key = identity(entry, root)
        require(key not in result, f"duplicate resource identity: {key}")
        result[key] = entry
    return result


def fill_export(template: Path, output: Path, values):
    """Edit target text only in production-exported XLIFF; never ids/source."""
    tree = ET.parse(template)
    units = tree.findall(f".//{{{NS}}}trans-unit")
    require(len(units) == len(values), "XLIFF unit count differs from project snapshot")
    seen = set()
    for unit in units:
        key = unit.attrib["id"]
        require(key in values and key not in seen, f"unexpected XLIFF id: {key}")
        seen.add(key)
        expected_source, target_text = values[key]
        source = unit.find(f"{{{NS}}}source")
        target = unit.find(f"{{{NS}}}target")
        require(source is not None and target is not None, "XLIFF source/target missing")
        require((source.text or "") == expected_source, f"export source mismatch: {key}")
        require(isinstance(target_text, str) and target_text.strip(), f"saved translation empty: {key}")
        target.text = target_text
    tree.write(output, encoding="utf-8", xml_declaration=True)


class Replay:
    def __init__(self, args):
        self.args = args
        self.output = args.output.resolve()
        self.source = args.matrix.resolve(strict=True)
        require(not self.output.exists(), "output directory must be new")
        require(not self.output.is_relative_to(self.source), "output must be outside frozen input matrix")
        self.output.mkdir(parents=True)
        self.binary = self.output / "frozen/locust.exe"
        self.binary.parent.mkdir()
        before = qa.sha256_file(args.locust)
        shutil.copyfile(args.locust, self.binary)
        require(before == qa.sha256_file(args.locust) == qa.sha256_file(self.binary), "CLI changed during freeze")
        self.configs = {}
        for language in ["ja", "en"]:
            path = self.output / f"offline-{language}.json"
            qa.json_write(path, {"providers": {}, "default_provider": None, "default_source_lang": language})
            self.configs[language] = path
        self.report = {"label": args.label, "started_at": qa.utc_now(), "matrix": str(self.source),
            "cli": {"source": str(args.locust.resolve()), "frozen": str(self.binary), "sha256": before},
            "reference_helper_sha256": qa.sha256_file(HELPER), "api_calls": 0,
            "method": "fresh CLI extract -> saved EN XLIFF import -> CLI pivot -> saved target XLIFF import",
            "engines": list(args.engines), "targets": list(args.targets), "cases": [], "input_backups": []}
        self.input_hashes = qa.tree_hashes(self.source, exclude_locust=False)
        self.saved = {}

    def command(self, case, phase, arguments, source_language="en", accepted=(0,)):
        require(arguments and arguments[0] in ALLOWED, "non-offline CLI command refused")
        require("--url" not in arguments and "--force" not in arguments and "--confirm-legacy" not in arguments, "network or override option refused")
        logs = case / "logs"
        logs.mkdir(exist_ok=True)
        command = [str(self.binary), "--config", str(self.configs[source_language]), *map(str, arguments)]
        started = time.monotonic()
        completed = subprocess.run(command, cwd=self.output, env={**os.environ, "LOCUST_BACKUP_ROOT": str(case / "backups")},
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            encoding="utf-8", errors="replace", timeout=self.args.timeout, check=False)
        out, err = logs / f"{phase}.stdout.txt", logs / f"{phase}.stderr.txt"
        out.write_text(completed.stdout, encoding="utf-8")
        err.write_text(completed.stderr, encoding="utf-8")
        record = {"command":command,"exit_code":completed.returncode,"duration_seconds":time.monotonic()-started,
            "stdout":str(out),"stderr":str(err)}
        qa.json_write(logs / f"{phase}.json", record)
        require(completed.returncode in accepted, f"{phase}: CLI exit {completed.returncode}; see {err}")
        return completed

    def prepare(self):
        saved_count = 0
        for engine in self.args.engines:
            template = self.output / "templates" / engine
            shutil.copytree(self.source / "fixtures" / engine, template)
            (template / "fixture-manifest.json").unlink(missing_ok=True)
            for lang in ["en", *self.args.targets]:
                original = self.source / "projects" / engine / ("ja-en.locust.db" if lang == "en" else f"{lang}/en-target.locust.db")
                owned = self.output / "saved" / engine / f"{lang}.db"
                stage = self.output / "staging" / engine / f"{lang}.db"
                self.report["input_backups"].append(backup_frozen_db(original, owned, stage))
                snap = snapshot(owned)
                require(snap["counts"]["total"] == snap["counts"]["translated_nonempty"], f"saved {engine}/{lang} incomplete")
                self.saved[(engine, lang)] = snap
                saved_count += snap["counts"]["translated_nonempty"]
        self.report["saved_selected_rows"] = saved_count
        self.report["expected_target_rows"] = sum(self.saved[(engine, target)]["counts"]["total"] for engine in self.args.engines for target in self.args.targets)
        # The original full matrix contains 51 JA->EN +255 EN->targets +10 direct JA->ES rows.
        comparison = self.source / "direct-comparison/ja-es.locust.db"
        if comparison.exists():
            owned = self.output / "saved/direct-ja-es.db"
            self.report["input_backups"].append(backup_frozen_db(comparison, owned, self.output / "staging/direct-ja-es.db"))
            self.report["saved_direct_comparison_rows_not_replayed"] = snapshot(owned)["counts"]["translated_nonempty"]
        self.save()

    def run_case(self, engine, target):
        case = self.output / "cases" / engine / target
        case.mkdir(parents=True)
        result = {"engine":engine,"target":target,"ok":False,"directory":str(case)}
        try:
            template = self.output / "templates" / engine
            game, consumer = case / "game", case / "consumer"
            shutil.copytree(template, game)
            shutil.copytree(template, consumer)
            before = game_hashes(game)
            require(before == game_hashes(consumer), "fresh game/consumer differ")
            result["pristine_hashes"] = before
            ja_db, target_db = case / "ja-en.db", case / "en-target.db"
            self.command(case, "extract-ja", ["extract", game, "-f", engine, "-o", ja_db], "ja")
            fresh = snapshot(ja_db)
            old_root = self.source / "work" / engine / "game"
            original = indexed(self.saved[(engine,"en")]["entries"], old_root)
            saved_target = indexed(self.saved[(engine,target)]["entries"], old_root)
            new = indexed(fresh["entries"], game)
            require(new.keys() == original.keys() == saved_target.keys(), "fresh/saved resource identity sets differ")
            for key, entry in new.items():
                require(entry["source"] == original[key]["source"], f"Japanese source changed for {key}")
                require(saved_target[key]["source"] == original[key]["translation"], f"saved pivot source not exact English translation: {key}")
                require(saved_target[key]["metadata"].get("locust_injection_source") == entry["source"], f"saved Japanese provenance missing: {key}")
            result["saved_provenance_exact"] = True
            self.command(case, "export-ja", ["export",ja_db,"-f","xliff","-l","en","-o",case/"ja-template.xlf"], "ja")
            fill_export(case/"ja-template.xlf", case/"en-saved.xlf", {entry["id"]:(entry["source"],original[key]["translation"]) for key,entry in new.items()})
            self.command(case, "import-en", ["import",ja_db,"-f","xliff","-l","en","-i",case/"en-saved.xlf"], "ja")
            english = indexed(snapshot(ja_db)["entries"], game)
            require(all(english[k]["source"] == new[k]["source"] and english[k]["translation"] == original[k]["translation"] for k in new), "English import changed source or translation")
            self.command(case, "pivot", ["pivot",ja_db,"-o",target_db])
            pivot = indexed(snapshot(target_db)["entries"], game)
            require(pivot.keys() == new.keys(), "pivot dropped resource identities")
            for key in new:
                require(pivot[key]["source"] == original[key]["translation"], f"new pivot English mismatch: {key}")
                require(pivot[key]["metadata"].get("locust_injection_source") == new[key]["source"], f"production pivot lost Japanese provenance: {key}")
            self.command(case, "export-en", ["export",target_db,"-f","xliff","-l",target,"-o",case/"en-template.xlf"])
            fill_export(case/"en-template.xlf", case/"target-saved.xlf", {entry["id"]:(entry["source"],saved_target[key]["translation"]) for key,entry in pivot.items()})
            self.command(case, "import-target", ["import",target_db,"-f","xliff","-l",target,"-i",case/"target-saved.xlf"])
            translated = snapshot(target_db)
            actual = indexed(translated["entries"], game)
            require(all(actual[k]["translation"] == saved_target[k]["translation"] for k in new), "target import not exact saved text")
            require(all(actual[k]["metadata"] == pivot[k]["metadata"] for k in new), "import rewrote extraction/pivot metadata")
            qa.json_write(case/"imported.snapshot.json",translated)
            validation = self.command(case,"validate",["validate",target_db],accepted=(0,1))
            if validation.returncode:
                kinds=[]
                for line in validation.stdout.splitlines():
                    fields=[field.strip() for field in line.split("|")[1:-1]]
                    if len(fields)==3 and fields[1] != "Kind": kinds.append(fields[1])
                require(kinds and all(kind == "IdenticalToSource" for kind in kinds), "validation has blocking issues")
                result["identity_review_warnings"] = len(kinds)
            inject = self.command(case,"inject",["inject",game,"-P",target_db,"--direct","-l",target])
            metrics = qa.parse_injection_metrics(inject.stdout,inject.stderr)
            expected = sum(actual[k]["translation"] != new[k]["source"] for k in new)
            require(metrics["strings_written"] == expected and expected > 0, f"injection coverage {metrics} != {expected}")
            state = self.command(case, "inject-status", ["inject-status", game])
            result["injection_recovery_status"] = json.loads(state.stdout)
            require(result["injection_recovery_status"]["pending"] is None, "successful inject left an unfinished transaction")
            result["retained_recovery_bytes"] = sum(p.stat().st_size for p in (game / ".locust-injections").rglob("*") if p.is_file())
            after=game_hashes(game)
            require(after != before,"inject exit0 changed no game bytes")
            recording=[row for row in snapshot(target_db)["injections"] if row["lang"] == target]
            require(recording,"injection recording missing")
            result.update({"entries":len(new),"expected_writes":expected,"injection_metrics":metrics,"injected_hashes":after,"recording":recording})
            patch=case/"translation.zip"
            self.command(case,"pack",["patch",game,"-P",target_db,"-l",target,"-o",patch,"--pristine",template])
            with zipfile.ZipFile(patch) as archive:
                manifest=json.loads(archive.read("locust-patch.json"))
                require(manifest["engine"] == engine and manifest["language"] == target,"manifest engine/language mismatch")
                require(manifest["files"],"empty manifest")
                for file in manifest["files"]:
                    path=file["path"]
                    require(file.get("original_sha256") == before.get(path) and file.get("original_sha256"),f"strict original hash mismatch: {path}")
                    require(file["patched_sha256"] == after.get(path),f"payload hash mismatch: {path}")
            result["manifest"]=manifest
            result["patch_sha256"]=qa.sha256_file(patch)
            self.command(case,"verify-dry-run",["apply",consumer,patch,"--dry-run"])
            require(game_hashes(consumer)==before,"dry-run modified consumer")
            self.command(case,"apply",["apply",consumer,patch])
            require(game_hashes(consumer)==after,"apply bytes differ from directly injected game")
            result["apply_matches_injection"]=True
            re_db=case/"reextract.db"
            self.command(case,"reextract",["extract",consumer,"-f",engine,"-o",re_db])
            re=snapshot(re_db)
            qa.json_write(case/"reextract.snapshot.json",re)
            result["resource_oracle"] = assert_reextracted(engine, list(actual.values()), game, re["entries"], consumer)
            result["missing_target_texts"]=[]
            result["reextract_all_target_values"]=True
            self.command(case,"rollback",["patch-rollback",consumer])
            require(game_hashes(consumer)==before,"rollback did not restore all pristine game hashes")
            result["rollback_exact"]=True
            result["ok"]=True
        except Exception as error:
            result["error"]=f"{type(error).__name__}: {error}"
            (case/"failure.txt").write_text(traceback.format_exc(),encoding="utf-8")
        qa.json_write(case/"result.json",result)
        print(f"{engine}/{target}: {'PASS' if result['ok'] else result['error']}",flush=True)
        return result

    def save(self):
        qa.json_write(self.output/"report.json",self.report)

    def run(self):
        self.prepare()
        with concurrent.futures.ThreadPoolExecutor(max_workers=self.args.workers) as pool:
            pending=[pool.submit(self.run_case,e,t) for e in self.args.engines for t in self.args.targets]
            for future in concurrent.futures.as_completed(pending):
                self.report["cases"].append(future.result())
                self.save()
        unchanged=self.input_hashes == qa.tree_hashes(self.source,exclude_locust=False)
        self.report.update({"finished_at":qa.utc_now(),"frozen_matrix_unchanged":unchanged,
            "cases_passed":sum(case["ok"] for case in self.report["cases"]),
            "cases_total":len(self.report["cases"]),
            "target_rows_replayed":sum(case.get("entries",0) for case in self.report["cases"] if case["ok"])})
        self.report["ok"]=unchanged and all(case["ok"] for case in self.report["cases"])
        self.save()
        lines=[f"# Saved translation replay — {self.args.label}","",f"CLI SHA256: `{self.report['cli']['sha256']}`",
            f"Cases: {self.report['cases_passed']}/{self.report['cases_total']}; target rows: {self.report['target_rows_replayed']}/{self.report['expected_target_rows']}.",
            f"Frozen matrix unchanged: {unchanged}. Provider calls: zero.","",
            "| Engine | Target | Result |","|---|---|---|"]
        lines += [f"| {r['engine']} | {r['target']} | {'PASS' if r['ok'] else r['error']} |" for r in sorted(self.report["cases"],key=lambda r:(r['engine'],r['target']))]
        lines += ["","Synthetic game fixtures validate CLI data paths and byte restoration, not runtime rendering or commercial-game startup.",
            "English/target strings are exact saved provider outputs; only production export/import documents are edited, never SQLite rows or binary capacities."]
        (self.output/"REPORT.md").write_text("\n".join(lines)+"\n",encoding="utf-8")
        return 0 if self.report["ok"] else 1


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--matrix",type=Path,required=True)
    parser.add_argument("--locust",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--label",choices=["preview","final"],default="preview")
    parser.add_argument("--engines",default=",".join(ENGINES))
    parser.add_argument("--targets",default=",".join(TARGETS))
    parser.add_argument("--workers",type=int,choices=range(1,6),default=2)
    parser.add_argument("--timeout",type=int,default=180)
    args=parser.parse_args()
    args.engines=tuple(args.engines.split(",")); args.targets=tuple(args.targets.split(","))
    require(set(args.engines)<=set(ENGINES) and len(set(args.engines))==len(args.engines),"invalid or duplicate engines")
    require(set(args.targets)<=set(TARGETS) and len(set(args.targets))==len(args.targets),"invalid or duplicate targets")
    require(30 <= args.timeout <= 600,"timeout must be 30..600 seconds")
    return Replay(args).run()


if __name__ == "__main__":
    raise SystemExit(main())
