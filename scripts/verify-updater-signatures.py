#!/usr/bin/env python3
"""Cryptographically verify every updater artifact against the shipped public key.

Usage: verify-updater-signatures.py <tauri.conf.json> <artifact-dir>

The release publish job downloads the five updater artifacts and their `.sig`
files into <artifact-dir> before latest.json is generated. Each Tauri (minisign)
signature must verify with the `plugins.updater.pubkey` in tauri.conf.json:
Ed25519 over BLAKE2b-512 of the file for prehashed ("ED") signatures, over the
file itself for legacy ("Ed") ones, plus the global signature over the trusted
comment. Pure standard library: no package install on the release runner.

Deliberately not checked: the trusted comment's `file:` name (GitHub renames
assets on upload, for example spaces become dots, so it would refuse genuine
releases) and legacy "Ed" signatures stay accepted, as the Tauri client does.
"""
import base64
import binascii
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent
_SPEC = importlib.util.spec_from_file_location("updater_manifest", ROOT / "generate-updater-manifest.py")
_manifest = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_manifest)
PATTERNS = _manifest.PATTERNS


class Refused(Exception):
    """A failure safe to print in release automation (no key or file contents)."""


# ─── Ed25519 verification (RFC 8032, section 6 reference algorithm) ─────────

_P = 2**255 - 19
_L = 2**252 + 27742317777372353535851937790883648493
_D = -121665 * pow(121666, _P - 2, _P) % _P
_SQRT_M1 = pow(2, (_P - 1) // 4, _P)


def _recover_x(y, sign):
    if y >= _P:
        return None
    x2 = (y * y - 1) * pow(_D * y * y + 1, _P - 2, _P) % _P
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (_P + 3) // 8, _P)
    if (x * x - x2) % _P:
        x = x * _SQRT_M1 % _P
    if (x * x - x2) % _P:
        return None
    if (x & 1) != sign:
        x = _P - x
    return x


def _decompress(encoded):
    y = int.from_bytes(encoded, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _recover_x(y, sign)
    return None if x is None else (x, y, 1, x * y % _P)


def _add(p, q):
    a = (p[1] - p[0]) * (q[1] - q[0]) % _P
    b = (p[1] + p[0]) * (q[1] + q[0]) % _P
    c = 2 * p[3] * q[3] * _D % _P
    d = 2 * p[2] * q[2] % _P
    e, f, g, h = b - a, d - c, d + c, b + a
    return (e * f % _P, g * h % _P, f * g % _P, e * h % _P)


def _mul(scalar, point):
    result = (0, 1, 1, 0)
    while scalar:
        if scalar & 1:
            result = _add(result, point)
        point = _add(point, point)
        scalar >>= 1
    return result


def _equal(p, q):
    return (p[0] * q[2] - q[0] * p[2]) % _P == 0 and (p[1] * q[2] - q[1] * p[2]) % _P == 0


_BASE_Y = 4 * pow(5, _P - 2, _P) % _P
_BASE = (_recover_x(_BASE_Y, 0), _BASE_Y, 1, _recover_x(_BASE_Y, 0) * _BASE_Y % _P)


def ed25519_verify(public_key, message, signature):
    """True when `signature` is a valid Ed25519 signature of `message`."""
    if len(public_key) != 32 or len(signature) != 64:
        return False
    a = _decompress(public_key)
    r = _decompress(signature[:32])
    if a is None or r is None:
        return False
    s = int.from_bytes(signature[32:], "little")
    if s >= _L:
        return False
    h = int.from_bytes(hashlib.sha512(signature[:32] + public_key + message).digest(), "little") % _L
    return _equal(_mul(s, _BASE), _add(r, _mul(h, a)))


# ─── Minisign / Tauri wire formats ──────────────────────────────────────────

def _lines(text):
    return base64.b64decode(text.strip(), validate=True).decode("utf-8").splitlines()


def public_key(config_path):
    """(key id, 32-byte Ed25519 key) from tauri.conf.json's updater pubkey."""
    try:
        pubkey = json.loads(Path(config_path).read_text(encoding="utf-8"))["plugins"]["updater"]["pubkey"]
        lines = _lines(pubkey)
        packet = base64.b64decode(lines[1], validate=True)
        if len(lines) != 2 or len(packet) != 42 or packet[:2] != b"Ed":
            raise ValueError()
    except (OSError, ValueError, TypeError, KeyError, IndexError, UnicodeError, binascii.Error) as error:
        raise Refused("invalid configured updater public key") from error
    return packet[2:10], packet[10:]


def _payload_message(path, algorithm):
    if algorithm == b"Ed":
        return path.read_bytes()
    digest = hashlib.blake2b(digest_size=64)
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.digest()


def verify_file(payload, signature_text, key):
    """Raise Refused unless `signature_text` (a Tauri .sig) verifies `payload`."""
    key_id, key_bytes = key
    try:
        lines = _lines(signature_text)
        if len(lines) != 4 or not lines[2].startswith("trusted comment: "):
            raise ValueError()
        packet = base64.b64decode(lines[1], validate=True)
        global_signature = base64.b64decode(lines[3], validate=True)
        if len(packet) != 74 or packet[:2] not in (b"Ed", b"ED") or len(global_signature) != 64:
            raise ValueError()
    except (ValueError, UnicodeError, binascii.Error) as error:
        raise Refused(f"invalid updater signature for {payload.name}") from error
    if packet[2:10] != key_id:
        raise Refused(f"{payload.name} was not signed with the configured public key")
    signature = packet[10:]
    if not ed25519_verify(key_bytes, _payload_message(payload, packet[:2]), signature):
        raise Refused(f"{payload.name} does not match its updater signature")
    trusted = lines[2][len("trusted comment: "):].encode("utf-8")
    if not ed25519_verify(key_bytes, signature + trusted, global_signature):
        raise Refused(f"{payload.name} has a tampered trusted comment")


def verify_directory(config_path, directory):
    """Verify one artifact per updater platform; return {platform: file name}."""
    key = public_key(config_path)
    names = sorted(p.name for p in Path(directory).iterdir() if p.is_file())
    verified = {}
    for platform, pattern in PATTERNS.items():
        candidates = [name for name in names if re.search(pattern, name)]
        if len(candidates) != 1:
            raise Refused(f"expected exactly one updater artifact for {platform}, found {len(candidates)}")
        artifact = Path(directory) / candidates[0]
        sig = artifact.with_name(artifact.name + ".sig")
        if not sig.is_file():
            raise Refused(f"missing signature for {artifact.name}")
        verify_file(artifact, sig.read_text(encoding="utf-8"), key)
        verified[platform] = artifact.name
    return verified


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 2:
        print("Usage: verify-updater-signatures.py <tauri.conf.json> <artifact-dir>", file=sys.stderr)
        return 1
    try:
        verified = verify_directory(args[0], args[1])
    except (Refused, OSError) as error:
        reason = str(error) if isinstance(error, Refused) else "artifact directory unreadable"
        print("Updater signature verification refused: " + reason, file=sys.stderr)
        return 1
    for platform, name in verified.items():
        print(f"verified {platform}: {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
