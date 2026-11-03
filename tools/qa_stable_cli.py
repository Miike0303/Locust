"""Run six synthetic Stable-format lifecycles using an explicitly prebuilt CLI.

Usage: python tools/qa_stable_cli.py --cli /path/to/locust
Alternatively set LOCUST_CLI. No builds, accounts, downloads or SQL seeding.
One JSON line per case includes candidate SHA-256, commands and inventories.
All fixtures, profiles and backups live in automatically removed temp dirs.
Known failures remain failures (exit 1); they never waive an assertion.
"""
import argparse
from contextlib import closing
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import zipfile

# Keep local module caches out of the checkout, even without python -B.
sys.dont_write_bytecode = True
from stable_cli_fixtures import CASES, check_payload, create, expected_rows, files


def require(ok, message):
    if not ok:
        raise AssertionError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def inventory(root, payload_only=False):
    """Include empty directories for immutability; exclude only root receipts
    when comparing game payload after apply/rollback, never during dry-run.
    """
    result = {}
    for path in sorted(root.rglob("*")):
        rel = path.relative_to(root)
        if payload_only and rel.parts[0] in {".locust", ".locust-injections"}:
            continue
        require(not path.is_symlink(), "unexpected link in synthetic inventory: " + str(rel))
        if path.is_dir():
            result[rel.as_posix() + "/"] = {"type": "directory"}
        else:
            data = path.read_bytes()
            result[rel.as_posix()] = {"size": len(data), "sha256": sha256(data)}
    return result


def isolated_env(root):
    env = {k: v for k, v in os.environ.items() if k.upper() in {
        "PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "SYSTEMDRIVE"}}
    for key in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
                "XDG_DATA_HOME", "XDG_CACHE_HOME", "LOCUST_DATA_DIR", "LOCUST_BACKUP_ROOT",
                "TMP", "TEMP", "TMPDIR"):
        relative = {"HOME": "profile", "USERPROFILE": "profile",
                    "APPDATA": "profile/AppData/Roaming",
                    "LOCALAPPDATA": "profile/AppData/Local"}.get(key, "environment/" + key)
        path = root / relative
        path.mkdir(parents=True, exist_ok=True)
        env[key] = str(path)
    if os.name == "nt" and os.environ.get("USERPROFILE") and os.environ.get("LOCALAPPDATA"):
        # SHGetKnownFolderPath expands USERPROFILE, then checks that the folder
        # exists. Windows sandbox tokens append Packages/<token>/AC to Local.
        # Recreate only that directory shape under our synthetic profile so
        # production GameLock files also stay inside the owned temp tree.
        try:
            suffix = Path(os.environ["LOCALAPPDATA"]).relative_to(os.environ["USERPROFILE"])
        except ValueError:
            suffix = Path("AppData/Local")
        local = Path(env["USERPROFILE"]) / suffix
        local.mkdir(parents=True, exist_ok=True)
        env["LOCALAPPDATA"] = str(local)
    env.update({"NO_COLOR": "1", "TERM": "dumb"})
    return env


class Commands:
    def __init__(self, cli, root, timeout, summary):
        self.cli, self.root, self.timeout, self.summary = cli, root, timeout, summary
        self.env = isolated_env(root)
        self.config = root / "config.json"
        self.config.write_text("{}", encoding="utf-8")

    def run(self, stage, *args):
        command = [str(self.cli), "--config", str(self.config), *map(str, args)]
        record = {"stage": stage, "argv": command[1:]}
        self.summary["commands"].append(record)
        started = time.monotonic()
        try:
            result = subprocess.run(command, cwd=self.root, env=self.env,
                                    capture_output=True, timeout=self.timeout)
        except subprocess.TimeoutExpired as error:
            record.update({"exit": None, "error": f"timeout after {self.timeout:g}s"})
            raise RuntimeError(stage + ": " + record["error"]) from error
        except OSError as error:
            record.update({"exit": None, "error": str(error)})
            raise RuntimeError(stage + ": " + str(error)) from error
        finally:
            record["seconds"] = round(time.monotonic() - started, 3)
        record["exit"] = result.returncode
        out = result.stdout.decode("utf-8", errors="replace")
        err = result.stderr.decode("utf-8", errors="replace")
        if result.returncode:
            # Keep the exact diagnostic; successful commands need only exits.
            record.update({"stdout": out, "stderr": err})
            raise RuntimeError(f"{stage}: CLI exited {result.returncode}: " + (err.strip() or out.strip()))
        return out


def database_rows(path):
    # Read-only inspection of actual public-CLI results. Never seed translations.
    with closing(sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True)) as db:
        db.row_factory = sqlite3.Row
        rows = [dict(row) for row in db.execute(
            "SELECT id,source,translation,status,provider_used FROM strings ORDER BY id")]
        memory = db.execute("SELECT COUNT(*) FROM translation_memory").fetchone()[0]
    require(memory == 0, "translation memory must remain empty with --no-memory")
    return rows


def check_patch(case, archive, source, injected, summary):
    with zipfile.ZipFile(archive) as patch:
        manifest = json.loads(patch.read("locust-patch.json"))
        require(manifest["schema_version"] == 1, "unexpected patch schema")
        entries = manifest["files"]
        require(entries and any(e.get("original_sha256") for e in entries),
                "patch is not strict tier: missing pristine hashes")
        paths = [e["path"] for e in entries]
        changed = {name for name in files(case)
                   if (source / name).read_bytes() != (injected / name).read_bytes()}
        require(len(paths) == len(set(paths)) and set(paths) == changed,
                "patch payload inventory differs from changed injected files")
        require(set(patch.namelist()) == set(paths) | {"locust-patch.json", "README.txt"},
                "unexpected patch archive entries")
        for entry in entries:
            name = entry["path"]
            require(not entry.get("added", False), "Replace fixture unexpectedly adds a payload: " + name)
            require(entry.get("original_sha256") == sha256((source / name).read_bytes()),
                    "pristine hash mismatch: " + name)
            data = (injected / name).read_bytes()
            require(entry["patched_sha256"] == sha256(data) and entry["size"] == len(data)
                    and patch.read(name) == data, "patched bytes/hash mismatch: " + name)
    summary["patch"] = {"tier": "strict", "files": entries, "sha256": sha256(archive.read_bytes())}


def lifecycle(case, root, cli, timeout, summary):
    source, install = root / "source", root / "install"
    create(case, source)
    shutil.copytree(source, install)
    baseline = inventory(source)
    pristine = files(case)
    summary["inventories"]["pristine"] = baseline
    commands = Commands(cli, root, timeout, summary)
    db, archive = root / "project.db", root / "patch.zip"
    try:
        commands.run("extract", "extract", source, "-f", case.format_id, "-o", db)
        original = database_rows(db)
        require(original, "extraction is empty")
        require({r["id"]: r["source"] for r in original} == expected_rows(case),
                "extracted slots differ from the independent fixture prediction")
        require(all(r["translation"] is None and r["status"] == "pending" for r in original),
                "extraction unexpectedly contains translations")
        summary["extracted"] = len(original)
        summary["checks"]["nonempty_extraction"] = True

        commands.run("translate", "translate", db, "-p", "mock", "-s", "ja", "-t", "en", "--no-memory")
        translated = database_rows(db)
        require({r["id"]: r["source"] for r in translated} == expected_rows(case),
                "translate changed the source catalog")
        require(all(r["status"] == "translated" and r["provider_used"] == "mock"
                    and r["translation"] for r in translated), "missing Mock provider attribution/translation")
        require({r["id"]: r["translation"] for r in translated} == expected_rows(case, True),
                "Mock translations differ from independently predicted targets")
        summary["translated"] = len(translated)
        summary["provider"] = "mock"
        summary["checks"]["mock_attribution_and_no_memory"] = True

        validation = json.loads(commands.run("validate", "validate", db, "--json"))
        require(validation == {"count": 0, "issues": []}, "validation issues: " + json.dumps(validation))
        summary["checks"]["validation"] = True

        output = root / "injected"
        injection = json.loads(commands.run("inject", "inject", source, "-P", db,
                                            "-l", "en", "-m", "replace", "-o", output, "--json"))
        injected = Path(injection["injected_roots"]["en"]).resolve()
        require(injected.is_relative_to(output.resolve()) and injected not in {source, install},
                "injection returned a root outside its owned copy")
        report = injection["reports"]["en"]
        require(report["strings_written"] == len(translated) and report["strings_skipped"] == 0,
                "injection did not write every translated row")
        check_payload(case, injected)
        summary["inventories"]["injected_payload"] = inventory(injected, payload_only=True)
        summary["checks"]["injected_payloads_and_sentinels"] = True

        commands.run("patch", "patch", injected, "-P", db, "-l", "en", "-o", archive,
                     "--pristine", source, "--no-detect-id")
        check_patch(case, archive, source, injected, summary)
        summary["checks"]["strict_patch"] = True

        before = inventory(install)
        commands.run("apply-dry-run", "apply", install, archive, "--dry-run")
        after = inventory(install)
        summary["inventories"]["dry_run_before"] = before
        summary["inventories"]["dry_run_after"] = after
        require(before == after == baseline, "apply --dry-run changed the full install inventory")
        summary["checks"]["dry_run_unchanged"] = True

        commands.run("apply", "apply", install, archive)
        check_payload(case, install)
        require(inventory(install, payload_only=True) == inventory(injected, payload_only=True),
                "applied payload inventory differs from the injected copy")
        receipt = json.loads((install / ".locust/receipt.json").read_bytes())
        require(receipt["verification"] == "strict" and receipt["baseline"] == "pristine"
                and receipt["forced"] is False, "apply did not use strict pristine verification")
        summary["checks"]["applied_payloads_and_strict_receipt"] = True

        commands.run("patch-rollback", "patch-rollback", install)
        restored = inventory(install, payload_only=True)
        summary["inventories"]["restored_payload"] = restored
        require(restored == baseline, "rollback payload paths/sizes/SHA-256 differ from pristine")
        require(all((install / name).read_bytes() == data for name, data in pristine.items()),
                "rollback payload bytes differ from pristine")
        summary["checks"]["rollback_byte_identical"] = True
    finally:
        current = inventory(source)
        summary["inventories"]["source_after"] = current
        require(current == baseline and all((source / name).read_bytes() == data
                                           for name, data in pristine.items()),
                "original source inventory/bytes changed")
        summary["checks"]["source_unchanged"] = True


def run_case(case, cli, cli_sha, timeout):
    started = time.monotonic()
    summary = {"case": case.name, "format": case.format_id, "status": "failed",
               "cli_sha256": cli_sha, "commands": [], "checks": {}, "inventories": {}}
    try:
        with tempfile.TemporaryDirectory(prefix="locust-stable-" + case.name + "-") as scratch:
            root = Path(scratch).resolve()
            summary["scratch"] = str(root)
            lifecycle(case, root, cli, timeout, summary)
        summary["status"] = "passed"
    except (AssertionError, OSError, RuntimeError, ValueError, KeyError, sqlite3.Error,
            zipfile.BadZipFile) as error:
        summary["error"] = str(error)
    finally:
        summary["seconds"] = round(time.monotonic() - started, 3)
        summary["cleanup"] = "scratch" not in summary or not Path(summary["scratch"]).exists()
        if not summary["cleanup"]:
            summary["status"] = "failed"
            summary["error"] = summary.get("error", "") + "; owned scratch cleanup failed"
    return summary


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", default=os.environ.get("LOCUST_CLI"),
                        help="Already-built CLI path (or LOCUST_CLI); no automatic build or PATH search")
    parser.add_argument("--timeout", type=float, default=30,
                        help="Per-command deadline in seconds (default: 30)")
    args = parser.parse_args(argv)
    if not args.cli or not Path(args.cli).is_file():
        parser.error("already-built CLI missing; pass --cli PATH or set LOCUST_CLI: " + repr(args.cli))
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("--timeout must be a finite positive number")
    cli = Path(args.cli).resolve()
    try:
        cli_sha = sha256(cli.read_bytes())
    except OSError as error:
        parser.error("cannot read the selected CLI: " + str(error))
    failed = False
    for case in CASES:
        result = run_case(case, cli, cli_sha, args.timeout)
        print(json.dumps(result, ensure_ascii=True, sort_keys=True), flush=True)
        failed |= result["status"] != "passed"
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
