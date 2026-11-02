#!/usr/bin/env python3
"""Offline release guards. Network access and publication stay in release.yml."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
import tomllib
from urllib.parse import quote

SPEC = importlib.util.spec_from_file_location(
    "updater_manifest", Path(__file__).with_name("generate-updater-manifest.py"))
updater = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(updater)
Refused = updater.Refused

CLI_ASSETS = {
    "locust-x86_64-windows.exe",
    "locust-aarch64-macos",
    "locust-x86_64-macos",
    "locust-x86_64-linux",
}


def check_versions(ref, root):
    """Require a stable version tag, including for manual dispatches."""
    if not re.fullmatch(r"refs/tags/v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", ref):
        raise Refused("a stable vX.Y.Z version tag ref is required")
    tag = ref.removeprefix("refs/tags/")
    root = Path(root)
    try:
        cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
        versions = [
            cargo["workspace"]["package"]["version"],
            json.loads((root / "apps/desktop/package.json").read_text(encoding="utf-8"))["version"],
            json.loads((root / "apps/desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))["version"],
        ]
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise Refused("cannot read application versions") from error
    if any(version != tag[1:] for version in versions):
        raise Refused("tag/application version mismatch")
    return tag


def changelog_section(text, version):
    """Return one nonempty level-two release section, including its heading."""
    lines = text.replace("\r\n", "\n").splitlines()
    heading = re.compile(r"^## \[" + re.escape(version) + r"\](?:[ \t].*)?$")
    starts = [index for index, line in enumerate(lines) if heading.fullmatch(line)]
    if len(starts) != 1:
        raise Refused("missing or duplicate CHANGELOG release section")
    start = starts[0]
    end = next((index for index in range(start + 1, len(lines))
                if lines[index].startswith("## ")), len(lines))
    if not any(line.strip() for line in lines[start + 1:end]):
        raise Refused("empty CHANGELOG release section")
    return "\n".join(lines[start:end]).rstrip() + "\n"


def check_draft(release, tag, sha):
    if not isinstance(release, dict) or release.get("isDraft") is not True:
        raise Refused("release must still be a draft")
    if release.get("isPrerelease") is not False:
        raise Refused("release must not be a prerelease")
    if release.get("tagName") != tag:
        raise Refused("release tag mismatch")
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", sha) or release.get("targetCommitish") != sha:
        raise Refused("release source SHA mismatch")


def verify_inventory(release, manifest, expected, tag, sha, repo):
    """Verify uploaded metadata and the downloaded manifest before publication.

    expected is regenerated from the exact uploaded .sig assets in this run.
    This checks signature shape/equality, not cryptographic key correspondence.
    """
    check_draft(release, tag, sha)
    pairs, notes = updater.release_pairs(json.dumps(release))
    required = CLI_ASSETS | {"latest.json"} | set(pairs.values())
    required |= {name + ".sig" for name in pairs.values()}
    assets = {asset["name"]: asset for asset in release["assets"]}
    if not required <= assets.keys():
        raise Refused("incomplete release asset inventory")
    for name in required:
        size = assets[name].get("size")
        if type(size) is not int or size <= 0:
            raise Refused("required release asset is empty or has no size")
    if not isinstance(manifest, dict) or manifest != expected:
        raise Refused("uploaded latest.json differs from generated manifest")
    if manifest.get("version") != tag.removeprefix("v") or manifest.get("notes") != notes:
        raise Refused("manifest version or release notes mismatch")
    platforms = manifest.get("platforms")
    if not isinstance(platforms, dict) or set(platforms) != set(updater.PATTERNS):
        raise Refused("incomplete manifest platform set")
    base = f"https://github.com/{repo}/releases/download/{quote(tag, safe='')}/"
    for target, artifact in pairs.items():
        entry = platforms[target]
        if not isinstance(entry, dict) or entry.get("url") != base + quote(artifact, safe=""):
            raise Refused("manifest artifact URL mismatch")
        if not isinstance(entry.get("signature"), str):
            raise Refused("invalid updater signature")
        updater.validate_signature(entry["signature"])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--ref", required=True)
    prepare.add_argument("--root", type=Path, default=Path("."))
    for command in ("draft", "verify"):
        sub = commands.add_parser(command)
        sub.add_argument("--tag", required=True)
        sub.add_argument("--sha", required=True)
        sub.add_argument("--metadata", type=Path, required=True)
        if command == "verify":
            sub.add_argument("--manifest", type=Path, required=True)
            sub.add_argument("--expected-manifest", type=Path, required=True)
            sub.add_argument("--notes-file", type=Path, required=True)
            sub.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY", "Miike0303/Locust"))
    args = parser.parse_args(argv)
    try:
        if args.command == "prepare":
            tag = check_versions(args.ref, args.root)
            notes = changelog_section((args.root / "CHANGELOG.md").read_text(encoding="utf-8"), tag[1:])
            print(notes, end="")
        else:
            release = json.loads(args.metadata.read_text(encoding="utf-8"))
            if args.command == "draft":
                check_draft(release, args.tag, args.sha)
            else:
                manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
                expected = json.loads(args.expected_manifest.read_text(encoding="utf-8"))
                notes = args.notes_file.read_text(encoding="utf-8")
                body = release.get("body") if isinstance(release, dict) else None
                if not isinstance(body, str) or body.strip() != notes.strip():
                    raise Refused("release body differs from CHANGELOG section")
                verify_inventory(release, manifest, expected, args.tag, args.sha, args.repo)
    except (Refused, OSError, ValueError, KeyError, TypeError) as error:
        reason = str(error) if isinstance(error, Refused) else "invalid release input"
        print("Release refused: " + reason, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
