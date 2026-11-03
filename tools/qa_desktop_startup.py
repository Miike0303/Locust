"""Run an already-built locust-desktop --self-test; never builds anything.

Usage: python tools/qa_desktop_startup.py <path/to/locust-desktop[.exe]>
Run after the frontend build and `cargo build --locked -p locust-desktop`.
Ubuntu still needs Tauri's linked GTK/WebKit libraries, but no display or
xvfb-run: both DISPLAY and WAYLAND_DISPLAY are removed for the child process.
Each invocation has a 30-second deadline; the pair normally takes seconds.
"""

import argparse
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time


def need(condition, message):
    if not condition:
        raise AssertionError(message)


def run_case(executable, root, data_dir):
    env = os.environ.copy()
    env.update(
        LOCUST_DATA_DIR=str(data_dir),
        LOCUST_BACKUP_ROOT=str(data_dir / "backups"),
        TMP=str(root / "runtime-tmp"),
        TEMP=str(root / "runtime-tmp"),
        TMPDIR=str(root / "runtime-tmp"),
    )
    env.pop("DISPLAY", None)
    env.pop("WAYLAND_DISPLAY", None)
    completed = subprocess.run(
        [str(executable), "--self-test"],
        cwd=root,
        env=env,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="strict",
        timeout=30,
        check=False,
    )
    lines = completed.stdout.splitlines()
    need(len(lines) == 1, f"expected one JSON line: {completed!r}")
    receipt = json.loads(lines[0])
    need(isinstance(receipt, dict), f"invalid receipt: {receipt!r}")
    need(set(receipt) == {"ok", "version", "port", "checks"}, receipt)
    need(type(receipt["ok"]) is bool, receipt)
    need(
        isinstance(receipt["version"], str)
        and re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.+-]+)?", receipt["version"]),
        receipt,
    )
    need(isinstance(receipt["checks"], list) and receipt["checks"], receipt)
    for check in receipt["checks"]:
        need(isinstance(check, dict) and {"name", "ok"} <= set(check), receipt)
        need(type(check["ok"]) is bool, receipt)
    print(lines[0])
    return completed, receipt


def inventory(root):
    result = set()
    for path in root.rglob("*"):
        need(not path.is_symlink(), f"unexpected symlink: {path}")
        result.add(path.relative_to(root).as_posix() + ("/" if path.is_dir() else ""))
    return result


def verify(executable):
    started = time.monotonic()
    # A non-ASCII path also exercises startup diagnostics on Windows.
    with tempfile.TemporaryDirectory(prefix="locust-desktop-qa-") as scratch:
        root = Path(scratch).resolve()
        (root / "runtime-tmp").mkdir()
        data_dir = root / "perfil-é"
        happy, receipt = run_case(executable, root, data_dir)
        need(happy.returncode == 0, f"happy exit={happy.returncode}: {happy.stderr}")
        need(receipt["ok"] is True, receipt)
        port = receipt["port"]
        need(type(port) is int and 0 < port <= 65535, receipt)
        need(
            receipt["checks"] == [
                {"name": name, "ok": True}
                for name in ("app_state", "health", "formats", "providers", "shutdown")
            ],
            receipt,
        )
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.settimeout(1)
            need(probe.connect_ex(("127.0.0.1", port)) != 0, "backend listener survived exit")

        expected = {
            "runtime-tmp/", "perfil-é/", "perfil-é/backups/",
            "perfil-é/project.db", "perfil-é/global_memory.db",
        }
        need(inventory(root) == expected, f"unexpected startup inventory: {inventory(root)}")
        for name in ("project.db", "global_memory.db"):
            with (data_dir / name).open("rb") as database:
                need(database.read(16) == b"SQLite format 3\x00", f"invalid {name}")

        blocked = root / "blocked-é"
        sentinel = b"This data directory is a file; preserve it.\n"
        blocked.write_bytes(sentinel)
        failure, failed_receipt = run_case(executable, root, blocked)
        need(failure.returncode > 0, f"invalid-dir exit={failure.returncode}")
        need(failed_receipt["ok"] is False and failed_receipt["port"] is None, failed_receipt)
        need(failed_receipt["version"] == receipt["version"], failed_receipt)
        need(len(failed_receipt["checks"]) == 1, failed_receipt)
        check = failed_receipt["checks"][0]
        need(check["name"] == "app_state" and check["ok"] is False, failed_receipt)
        for text in (
            "Cannot create data directory", str(blocked), "LOCUST_DATA_DIR",
        ):
            need(text in check.get("error", "") and text in failure.stderr, failure.stderr)
        need("Locust could not start." in failure.stderr, failure.stderr)
        need(blocked.read_bytes() == sentinel, "failure path changed the input file")
        expected.add("blocked-é")
        need(inventory(root) == expected, f"unexpected failure inventory: {inventory(root)}")
        print(failure.stderr.strip())
    need(not root.exists(), f"scratch cleanup failed: {root}")
    print(f"desktop startup QA passed in {time.monotonic() - started:.2f}s (no display)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    args = parser.parse_args()
    executable = args.executable.resolve(strict=True)
    need(executable.is_file(), f"not an executable file: {executable}")
    verify(executable)


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"desktop startup QA failed: {error}", file=sys.stderr)
        sys.exit(1)
