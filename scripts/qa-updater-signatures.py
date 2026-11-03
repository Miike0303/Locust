#!/usr/bin/env python3
"""Prove Tauri signer -> .sig -> production latest.json -> updater verification.

Requires the installed desktop Tauri CLI and cached Cargo dependencies. Run from
any directory with CARGO_TARGET_DIR set to the existing shared build target.
Only GitHub/download subprocesses in the production generator are replaced;
key generation, signing, manifest writing and Rust verification are real.
All fixtures and disposable private keys live outside the repository and are
deleted, including on failure. Signer output/passwords are never printed.
"""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
from unittest import mock
from urllib.parse import quote


ROOT = Path(__file__).resolve().parents[1]
TAG = "v0.3.0"
REPO = "Miike0303/Locust"
ASSETS = {
    "windows-x86_64": "Locust_0.3.0_x64_en-US.msi",
    "windows-aarch64": "Locust_0.3.0_arm64_en-US.msi",
    "darwin-aarch64": "Locust_aarch64.app.tar.gz",
    "darwin-x86_64": "Locust_x64.app.tar.gz",
    "linux-x86_64": "locust_0.3.0_amd64.AppImage",
}


class Failed(Exception):
    """A failure safe to print without signer output or credentials."""


def signer_command():
    # Invoke the installed npx tauri entrypoint directly: no npm downloads or
    # Windows .cmd shell quoting. The owner checkout is a read-only fallback.
    node = shutil.which("node")
    candidates = [ROOT / "apps/desktop/node_modules/@tauri-apps/cli/tauri.js"]
    if os.name == "nt":
        candidates.append(Path("C:/Projects/Locust/apps/desktop/node_modules/@tauri-apps/cli/tauri.js"))
    if node:
        for entrypoint in candidates:
            if entrypoint.is_file():
                return [node, str(entrypoint)]
    raise Failed("installed desktop Tauri CLI missing; run npm ci in apps/desktop first")


def signer_run(command, scratch, env, phase):
    try:
        result = subprocess.run(command, cwd=scratch, env=env,
                                stdin=subprocess.DEVNULL, capture_output=True, timeout=120)
    except (OSError, subprocess.SubprocessError):
        raise Failed(f"Tauri {phase} could not run (signer output suppressed)") from None
    if result.returncode:
        raise Failed(f"Tauri {phase} failed, exit {result.returncode} (signer output suppressed)")


def generate_manifest(scratch, draft):
    spec = importlib.util.spec_from_file_location(
        "updater_manifest", ROOT / "scripts/generate-updater-manifest.py")
    generator = importlib.util.module_from_spec(spec)
    # Importing this production module must not leave bytecode in the repo.
    with mock.patch.object(sys, "dont_write_bytecode", True):
        spec.loader.exec_module(generator)

    base = f"https://github.com/{REPO}/releases/download/{TAG}/"
    calls = []

    def transport(command, **kwargs):
        calls.append(command)
        if command == ["gh", "release", "view", TAG, "--repo", REPO,
                       "--json", "assets,body,isDraft"]:
            # Deliberately scramble signatures to catch positional pairing.
            names = list(ASSETS.values())
            text = json.dumps({"assets": [{"name": name} for name in
                                          list(reversed([n + ".sig" for n in names])) + names],
                               "body": "Disposable signer contract", "isDraft": draft})
        else:
            for artifact in ASSETS.values():
                expected = (["gh", "release", "download", TAG, "--repo", REPO,
                             "--pattern", artifact + ".sig", "--output", "-"] if draft else
                            ["curl", "--fail", "--silent", "--show-error", "--location", "--",
                             base + quote(artifact + ".sig", safe="")])
                if command == expected:
                    text = (scratch / (artifact + ".sig")).read_text(encoding="utf-8")
                    break
            else:
                raise Failed("unexpected generator transport command; network refused")
        return subprocess.CompletedProcess(command, 0, text, "")

    previous = Path.cwd()
    out, err = io.StringIO(), io.StringIO()
    try:
        os.chdir(scratch)
        with mock.patch.dict(os.environ, {"GITHUB_REPOSITORY": REPO}), \
                mock.patch.object(generator.subprocess, "run", side_effect=transport), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = generator.main([TAG])
    finally:
        os.chdir(previous)
    if code or err.getvalue() or len(calls) != 6:
        raise Failed("production manifest generator refused the five signed artifacts")
    manifest_path = scratch / "latest.json"
    if manifest_path.read_text(encoding="utf-8") != out.getvalue():
        raise Failed("printed manifest differs from the production latest.json file")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest["version"] != TAG[1:] or set(manifest["platforms"]) != set(ASSETS):
        raise Failed("production manifest lacks the strict five-platform contract")
    for platform, artifact in ASSETS.items():
        expected = {"url": base + quote(artifact, safe=""),
                    "signature": (scratch / (artifact + ".sig")).read_text(encoding="utf-8").strip()}
        if manifest["platforms"][platform] != expected:
            raise Failed(f"production manifest paired the wrong artifact/signature: {platform}")
    return manifest


def run(scratch):
    if not os.environ.get("CARGO_TARGET_DIR"):
        raise Failed("CARGO_TARGET_DIR must name the existing shared target directory")
    signer = signer_command()
    print(f"Tauri CLI: {signer[1]}", flush=True)
    env = os.environ.copy()
    for name in ("TMP", "TEMP", "TMPDIR"):
        env[name] = str(scratch)
    # Never accidentally use an inherited release identity.
    for name in ("TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PATH",
                 "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"):
        env.pop(name, None)
    password = secrets.token_urlsafe(32)
    private_key = scratch / "disposable.key"
    wrong_key = scratch / "wrong.key"
    for path in (private_key, wrong_key):
        signer_run(signer + ["signer", "generate", "--ci", "--password", password,
                             "--write-keys", str(path)], scratch, env, "key generation")
    wrong_key.unlink()  # Only its public half is used by the negative contract.
    env["TAURI_SIGNING_PRIVATE_KEY_PASSWORD"] = password
    try:
        for platform, artifact in ASSETS.items():
            payload = scratch / artifact
            payload.write_bytes(b"Locust disposable updater payload\x00" + platform.encode("ascii") + b"\n")
            signer_run(signer + ["signer", "sign", "--private-key-path", str(private_key),
                                 str(payload)], scratch, env, f"signing {platform}")
    finally:
        private_key.unlink(missing_ok=True)
        env.pop("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", None)
    print("Signed 5 distinct synthetic artifacts; both private keys deleted.", flush=True)
    public = generate_manifest(scratch, draft=False)
    draft = generate_manifest(scratch, draft=True)
    if public["platforms"] != draft["platforms"]:
        raise Failed("public and draft manifest signatures differ")
    print("Production generator: public + draft transports, 5 exact platform pairs each.", flush=True)
    env.update({
        "CARGO_INCREMENTAL": "0",
        "LOCUST_UPDATER_PUBLIC_KEY": str(private_key.with_suffix(".key.pub")),
        "LOCUST_UPDATER_WRONG_PUBLIC_KEY": str(wrong_key.with_suffix(".key.pub")),
        "LOCUST_UPDATER_MANIFEST": str(scratch / "latest.json"),
        "LOCUST_UPDATER_PAYLOAD_DIR": str(scratch),
    })
    # Ignored by ordinary workspace tests because these need fresh signer
    # fixtures. This runner explicitly enables them, preserving thread caps.
    command = ["cargo", "test", "--offline", "--locked", "-p", "locust-desktop",
               "--test", "updater_signature_contract", "--", "--ignored", "--nocapture"]
    print("Running: " + " ".join(command), flush=True)
    try:
        result = subprocess.run(command, cwd=ROOT, env=env, stdin=subprocess.DEVNULL, timeout=1200)
    except (OSError, subprocess.SubprocessError):
        raise Failed("Cargo verifier could not complete") from None
    if result.returncode:
        raise Failed(f"Cargo signature contract failed, exit {result.returncode}")


def main():
    start = time.monotonic()
    try:
        # Windows sandbox TEMP can be virtualized into an inaccessible package
        # directory. Use the user's normal Temp, also outside Cargo's repo TMP.
        temp_root = (Path.home() / "AppData/Local/Temp" if os.name == "nt"
                     else Path(tempfile.gettempdir())).resolve()
        if temp_root.is_relative_to(ROOT):
            raise Failed("set TEMP/TMPDIR outside the repository for disposable signing keys")
        with tempfile.TemporaryDirectory(prefix="locust-updater-signatures-", dir=temp_root) as directory:
            run(Path(directory))
    except (Failed, OSError, ValueError) as error:
        reason = str(error) if isinstance(error, Failed) else "cannot prepare or clean up disposable fixtures"
        print(f"FAIL: {reason} ({time.monotonic() - start:.2f}s)", file=sys.stderr)
        return 1
    print(f"PASS: signer -> .sig -> latest.json -> cryptographic verification; "
          f"temporary directory deleted ({time.monotonic() - start:.2f}s).", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
