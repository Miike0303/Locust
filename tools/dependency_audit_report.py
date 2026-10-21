"""Collect an audit without treating advisory findings as a build gate."""
import argparse
import json
from pathlib import Path
import subprocess
import sys


def classify(kind, stdout, code):
    if code not in (0, 1):
        raise ValueError(f"unexpected auditor exit {code}")
    data = json.loads(stdout)
    if kind == "rust":
        vulnerabilities = data["vulnerabilities"]
        count = vulnerabilities["count"]
        if (type(count) is not int or count < 0 or not isinstance(vulnerabilities["list"], list)
                or len(vulnerabilities["list"]) != count):
            raise ValueError("invalid Rust vulnerability count/list")
        warnings = data.get("warnings", {})
        if not isinstance(warnings, dict) or any(not isinstance(v, list) for v in warnings.values()):
            raise ValueError("invalid Rust warning lists")
        warning_count = sum(len(value) for value in warnings.values())
    else:
        if "error" in data:
            raise ValueError("npm reported an operational error")
        count = data["metadata"]["vulnerabilities"]["total"]
        if type(count) is not int or count < 0:
            raise ValueError("invalid npm vulnerability total")
        warning_count = 0
    if code == 1 and count == 0:
        raise ValueError("auditor failed without vulnerability findings")
    return "findings" if count else "warnings" if warning_count else "clean", count, warning_count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", required=True, choices=("rust", "npm"))
    parser.add_argument("--output", required=True, type=Path,
                        help="new evidence directory; existing evidence is never overwritten")
    parser.add_argument("command", nargs=argparse.REMAINDER,
                        help="auditor command after --; use JSON output and no fix flags")
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("an auditor command is required after --")
    try:
        args.output.mkdir(parents=True, exist_ok=False)
    except FileExistsError:
        parser.error("output directory already exists")
    result = {"kind": args.kind, "command": command, "exit_code": None,
              "status": "error", "vulnerability_count": None, "warning_count": None, "error": None}
    stdout, stderr = b"", b""
    try:
        child = subprocess.run(command, capture_output=True, timeout=300, check=False)
        stdout, stderr = child.stdout, child.stderr
        result["exit_code"] = child.returncode
        result["status"], result["vulnerability_count"], result["warning_count"] = classify(
            args.kind, stdout, child.returncode)
    except subprocess.TimeoutExpired as error:
        stdout, stderr = error.stdout or b"", error.stderr or b""
        result["error"] = "auditor timed out after 300 seconds; no retry performed"
    except (OSError, ValueError, KeyError, TypeError) as error:
        result["error"] = str(error)
    (args.output / "stdout.json").write_bytes(stdout)
    (args.output / "stderr.txt").write_bytes(stderr)
    text = json.dumps(result, indent=2, ensure_ascii=True) + "\n"
    encoded = text.encode("utf-8")
    (args.output / "result.json").write_bytes(encoded)
    sys.stdout.buffer.write(encoded)
    return 2 if result["status"] == "error" else 0


if __name__ == "__main__":
    raise SystemExit(main())
