#!/usr/bin/env python3
"""Generate the existing Tauri latest.json contract from GitHub release assets."""
import base64
import binascii
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from urllib.parse import quote

# Each target accepts one installer and only that installer's exact .sig asset.
PATTERNS = {
    "windows-x86_64": r"_x64_en-US\.msi$",
    "windows-aarch64": r"_(?:arm64|aarch64)_en-US\.msi$",
    "darwin-aarch64": r"_aarch64\.app\.tar\.gz$",
    "darwin-x86_64": r"_x64\.app\.tar\.gz$",
    "linux-x86_64": r"_amd64\.AppImage$",
}


class Refused(Exception):
    """A sanitized failure safe to print in release automation."""


def run(command, reason):
    try:
        return subprocess.run(command, check=True, capture_output=True,
                              text=True, encoding="utf-8", timeout=30).stdout
    except (OSError, subprocess.SubprocessError, UnicodeError) as error:
        raise Refused(reason) from error


def validate_signature(text):
    """Check Tauri's base64-encoded minisign shape, not key correspondence."""
    text = text.strip()
    try:
        lines = base64.b64decode(text, validate=True).decode("ascii").splitlines()
        if len(lines) != 4 or not lines[0].startswith("untrusted comment: ") or not lines[2].startswith("trusted comment: "):
            raise ValueError()
        packet = base64.b64decode(lines[1], validate=True)
        global_signature = base64.b64decode(lines[3], validate=True)
        if len(packet) != 74 or packet[:2] not in (b"Ed", b"ED") or len(global_signature) != 64:
            raise ValueError()
    except (ValueError, UnicodeError, binascii.Error) as error:
        raise Refused("invalid updater signature") from error
    return text


def release_pairs(raw):
    try:
        release = json.loads(raw)
        assets, notes = release["assets"], release.get("body")
        if notes is None:
            notes = ""
        if not isinstance(assets, list) or not isinstance(notes, str):
            raise ValueError()
        names = [asset["name"] for asset in assets]
        if any(not isinstance(name, str) or not name or "/" in name or "\\" in name for name in names):
            raise ValueError()
    except (ValueError, TypeError, KeyError) as error:
        raise Refused("invalid release metadata") from error
    if len(names) != len(set(names)):
        raise Refused("duplicate or ambiguous release assets")
    pairs = {}
    for target, pattern in PATTERNS.items():
        candidates = [name for name in names if re.search(pattern, name)]
        if len(candidates) > 1:
            raise Refused("duplicate or ambiguous release assets")
        if candidates and candidates[0] + ".sig" in names:
            pairs[target] = candidates[0]
    if not pairs:
        raise Refused("no complete updater artifact/signature pairs")
    return pairs, notes


def generate(tag, repo):
    raw = run(["gh", "release", "view", tag, "--repo", repo, "--json", "assets,body"],
              "release lookup failed")
    pairs, notes = release_pairs(raw)
    base = f"https://github.com/{repo}/releases/download/{quote(tag, safe='')}/"
    platforms = {}
    for target, artifact in pairs.items():
        text = run(["curl", "--fail", "--silent", "--show-error", "--location", "--",
                    base + quote(artifact + ".sig", safe="")], "signature download failed")
        platforms[target] = {"signature": validate_signature(text),
                             "url": base + quote(artifact, safe="")}
    return {"version": tag.removeprefix("v"), "notes": notes,
            "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "platforms": platforms}


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 1 or not args[0]:
        print("Usage: generate-updater-manifest.sh <tag>", file=sys.stderr)
        return 1
    repo = os.environ.get("GITHUB_REPOSITORY") or "Miike0303/Locust"
    temporary = None
    try:
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo) or not re.fullmatch(r"v?\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?(?:\+[A-Za-z0-9.-]+)?", args[0]):
            raise Refused("invalid repository or version tag")
        output = json.dumps(generate(args[0], repo), indent=2, ensure_ascii=False) + "\n"
        # Validation and every download finish before touching an existing manifest.
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", newline="\n",
                                         dir=".", prefix=".latest-", suffix=".json", delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(output)
        os.replace(temporary, "latest.json")
        temporary = None
    except (Refused, OSError) as error:
        reason = str(error) if isinstance(error, Refused) else "manifest write failed"
        print("Updater manifest refused: " + reason, file=sys.stderr)
        return 1
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    print(output, end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
