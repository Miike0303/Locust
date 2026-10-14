#!/usr/bin/env python3
"""Fail-closed, owned-copy JA -> EN -> ES -> strict patch/install pipeline.

Only the Locust CLI writes databases. This module reads its own databases with
mode=ro transactions, and never passes the source game as a CLI write target.
See ja_en_es_pipeline.md for the bounded engine and resume contracts.
"""
import argparse
from collections import defaultdict
from dataclasses import asdict, dataclass
import hashlib
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import sqlite3
import stat
import subprocess
import sys
import time
from urllib.parse import urlsplit
import uuid
import zipfile
import xml.etree.ElementTree as ET

VERSION = 2
OWNER = "locust-ja-en-es-pipeline"
PROVIDER = "pipeline-scripted"
FALLBACK_TEST_PROVIDER = "pipeline-google-scripted"
READY = {"translated", "reviewed", "approved"}
DUMMY_KEY = "pipeline-dummy-not-a-credential"
MODEL = "scripted-fixture"
RESERVED = {".locust", ".locust-injections"}
ARCHIVES = {".rpa", ".rpyc", ".xp3", ".zip", ".7z", ".rar", ".asar", ".nw", ".jsono"}
SENSITIVE = {".env", ".ssh", ".aws", ".git", ".gnupg", "credentials", "credentials.json",
             "tokens.json", "xai-oauth.json", "id_rsa", "id_ed25519"}
STATIC_FIELDS = ("id", "source", "file_path", "context", "tags", "metadata", "char_limit")
COLUMNS = (*STATIC_FIELDS, "created_at", "translation", "status", "provider_used", "translated_at", "reviewed_at")
RESUME_PHASES = {"english-pending", "english-ready", "spanish-pending", "spanish-ready"}
PUBLICATION_FAILURE = "pipeline-publication-failure.json"
PUBLICATION_DELAYS = (0.05, 0.1, 0.2)
UNSET = object()
RECOVERY = ("Resume refused. Retain this run; inspect owned work-game with Locust inject-status/"
            "inject-recover if needed. After any timeout, independently ensure descendants have stopped. "
            "Start a fresh output root from the untouched source; no automatic cleanup is performed.")


class PipelineError(Exception):
    """A deliberately text-free diagnostic (no provider response/source/secrets)."""


class ManifestPublicationError(PipelineError):
    """Terminal publication failure; the original error remains its cause."""


def require(condition, message):
    if not condition:
        raise PipelineError(message)


def integer(value):
    return type(value) is int and value >= 0


def nonblank(value):
    return isinstance(value, str) and bool(value.strip())


def strict_json(text):
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, "Duplicate JSON key")
            result[key] = value
        return result

    def constant(_):
        raise PipelineError("Nonfinite JSON number")

    try:
        return json.loads(text, object_pairs_hook=pairs, parse_constant=constant)
    except (ValueError, TypeError, UnicodeError) as error:
        raise PipelineError("Malformed JSON") from error


def encoded(value):
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def digest(value):
    return hashlib.sha256(encoded(value)).hexdigest()


def linked(info):
    return stat.S_ISLNK(info.st_mode) or bool(getattr(info, "st_file_attributes", 0) & 0x400)


def plain(path, missing=False):
    """Check every lexical component before resolving; includes Windows junctions."""
    path = Path(path)
    require(".." not in path.parts, "Parent traversal path refused")
    if not path.is_absolute():
        path = Path.cwd() / path
    for component in (*reversed(path.parents), path):
        try:
            info = component.lstat()
        except FileNotFoundError:
            require(missing, "Required path missing")
            continue
        require(not linked(info), "Linked/reparse path refused")
        require(stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode), "Special file refused")
        if stat.S_ISREG(info.st_mode):
            require(component == path, "Non-directory path component")
    return path


def safe_rel(value):
    require(isinstance(value, str) and value and "\\" not in value, "Unsafe relative path")
    parts = value.split("/")
    for part in parts:
        require(part not in ("", ".", "..") and ":" not in part and "\x00" not in part,
                "Unsafe relative path")
        require(part.rstrip(". ") == part, "Ambiguous filesystem name")
        stem = part.split(".")[0].upper()
        require(stem not in {"CON", "PRN", "AUX", "NUL", "CLOCK$"}
                and not re.fullmatch(r"(?:COM|LPT)\d", stem), "Reserved filesystem name")
    return value


def norm(value):
    value = str(value).replace("\\", "/").rstrip("/")
    return value[4:] if value.startswith("//?/") else value


def relative(value, root):
    require(isinstance(value, str), "Malformed resource path")
    value, root = norm(value), norm(root)
    # Windows extraction uses canonical verbatim paths. POSIX is case-sensitive.
    a, b = (value.lower(), root.lower()) if os.name == "nt" else (value, root)
    require(a.startswith(b + "/"), "Resource outside owned game")
    return safe_rel(value[len(root) + 1:])


def under(path, root):
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def directory_identity(path):
    info = plain(path).stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_ino != 0, "Directory identity unavailable")
    return info.st_dev, info.st_ino


def file_hash(path):
    path = plain(path)
    before = path.stat()
    require(stat.S_ISREG(before.st_mode), "Expected regular file")
    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    with os.fdopen(os.open(path, flags), "rb") as stream:
        opened = os.fstat(stream.fileno())
        require((opened.st_dev, opened.st_ino) == (before.st_dev, before.st_ino), "File changed while opening")
        sha = hashlib.sha256()
        size = 0
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            sha.update(chunk)
            size += len(chunk)
        after = os.fstat(stream.fileno())
    require((before.st_size, before.st_mtime_ns) == (after.st_size, after.st_mtime_ns)
            and size == before.st_size, "File changed while hashing")
    return {"sha256": sha.hexdigest(), "size": size}


def publication_file(path, expected=UNSET):
    """Identity plus bytes for a plain single-link publication file or absence.

    Check identity before hashing on retry, so an already-replaced foreign file
    is refused before opening it. Path checks are not a hostile-FS sandbox.
    """
    path = plain(path, missing=True)
    try:
        before = path.lstat()
    except FileNotFoundError:
        require(expected is UNSET or expected is None, "Publication file disappeared")
        return None
    require(not linked(before) and stat.S_ISREG(before.st_mode)
            and before.st_nlink == 1 and before.st_ino != 0, "Unsafe publication file")
    identity = {"device": before.st_dev, "inode": before.st_ino}
    if expected is not UNSET:
        require(expected is not None and all(identity[key] == expected[key] for key in identity),
                "Publication file identity changed")
    fingerprint = file_hash(path)
    after = plain(path).lstat()
    require(not linked(after) and stat.S_ISREG(after.st_mode) and after.st_nlink == 1
            and (after.st_dev, after.st_ino) == (before.st_dev, before.st_ino), "Publication file changed while checking")
    result = {**identity, **fingerprint}
    require(expected is UNSET or result == expected, "Publication file bytes changed")
    return result


def publication_state(manifest):
    """Small, nonsecret stage summary; never a claim of durable completion."""
    stages = manifest.get("stages", [])
    last = stages[-1] if stages else {}
    return {"phase": manifest.get("phase"), "injection_started": manifest.get("injection_started"),
            "uncertain_child": manifest.get("uncertain_child"),
            "active_command": manifest.get("active_command"), "stage_count": len(stages),
            "last_stage": last.get("stage"), "last_exit_code": last.get("exit_code")}


def scan(root, source=False):
    """Full tree: never blanket-ignore a .locust* prefix or an unknown file."""
    root = plain(root)
    require(root.is_dir(), "Game must be a plain directory")
    files, directories = {}, []
    stack = [root]
    while stack:
        directory = stack.pop()
        plain(directory)
        for path in sorted(directory.iterdir()):
            rel = safe_rel(path.relative_to(root).as_posix())
            info = path.lstat()
            require(not linked(info), "Linked/reparse tree member refused")
            require(stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode), "Special tree member refused")
            name = path.name.lower()
            require(name not in SENSITIVE and not name.startswith(".env.")
                    and path.suffix.lower() not in {".pem", ".key", ".p12", ".pfx"}, "Sensitive file/tree refused before read")
            if source:
                require(name not in RESERVED and not name.startswith("zzz_locust"), "Existing Locust-installed state refused")
                require(path.suffix.lower() not in ARCHIVES, "Archives/compiled scripts unsupported; KiriKiri is loose-only")
            if stat.S_ISDIR(info.st_mode):
                directories.append(rel)
                stack.append(path)
            else:
                require(info.st_nlink == 1, "Hard-linked tree member refused")
                files[rel] = file_hash(path)
    return {"files": dict(sorted(files.items())), "directories": sorted(directories)}


@dataclass
class Options:
    cli: str
    game: str
    output: str
    format: str
    provider: str
    test_endpoint: str = None
    batch_size: int = 40
    concurrency: int = 4
    timeout: float = 900
    resume: bool = False
    fallback_provider: str = None
    english_seed: str = None
    test_fallback_endpoint: str = None


def preflight(options):
    require(options.format in {"renpy", "rpgmaker-mv", "kirikiri"}, "Unsupported format")
    require(options.provider in {"google", "antigravity"}, "Unsupported provider")
    require(options.fallback_provider is None or
            (options.fallback_provider == "google" and options.provider == "antigravity"),
            "Fallback requires explicit google after primary antigravity")
    require(options.english_seed is None or (nonblank(options.english_seed) and not options.resume),
            "English seed is fresh-run-only")
    require(options.test_fallback_endpoint is None or
            (options.fallback_provider == "google" and options.test_endpoint is not None),
            "Test fallback requires explicit fallback and scripted primary")
    require(not (options.fallback_provider and options.test_endpoint) or
            (options.test_fallback_endpoint is not None and
             options.test_endpoint.rstrip("/") != options.test_fallback_endpoint.rstrip("/")),
            "Scripted fallback requires a separate explicit loopback route")
    require(type(options.batch_size) is int and 1 <= options.batch_size <= 200, "Batch size must be 1..200")
    require(type(options.concurrency) is int and 1 <= options.concurrency <= 16, "Concurrency must be 1..16")
    require(type(options.timeout) in (int, float) and 1 <= options.timeout <= 86400
            and math.isfinite(options.timeout), "Timeout must be 1..86400 seconds")
    require(Path(options.cli).is_absolute(), "--cli must be an absolute executable path")
    cli, game, output = plain(options.cli), plain(options.game), plain(options.output, missing=True)
    require(cli.is_file() and os.access(cli, os.X_OK), "CLI is not executable")
    require(game.is_dir(), "Source game must be a directory")
    require(not under(game, output) and not under(output, game), "Source/output overlap refused")
    require(not under(cli, output), "CLI cannot be inside output")
    require(output.parent.is_dir(), "Output parent must already exist")
    require(options.resume or not output.exists(), "Output occupied (even an empty directory is refused)")
    for test_endpoint in (options.test_endpoint, options.test_fallback_endpoint):
        if test_endpoint is None:
            continue
        require(isinstance(test_endpoint, str) and not any(char.isspace() or ord(char) < 32 for char in test_endpoint), "Malformed test endpoint")
        try:
            endpoint = urlsplit(test_endpoint)
            address = ipaddress.ip_address(endpoint.hostname or "")
            valid = (endpoint.scheme == "http" and address.is_loopback and endpoint.port is not None and endpoint.port >= 1
                     and not endpoint.username and not endpoint.password and not endpoint.query
                     and not endpoint.fragment and endpoint.path in ("", "/"))
        except ValueError:
            valid = False
        require(valid, "Test endpoint requires literal loopback HTTP host and port, no credentials/path/query")
    tree = scan(game, source=True)
    # Lexical containment alone misses bind mounts/alternate directory aliases.
    # Refuse a destination ancestor with any source-tree directory identity.
    source_directories = {directory_identity(game), *(directory_identity(game / path) for path in tree["directories"])}
    destination_ancestors = {directory_identity(path) for path in (output, *output.parents) if path.exists() and path.is_dir()}
    require(source_directories.isdisjoint(destination_ancestors), "Source/output filesystem alias overlap refused")
    if output.exists():
        require(output.is_dir(), "Resume output must be a directory")
        require(directory_identity(output) not in {directory_identity(path) for path in (game, *game.parents)}, "Output/source filesystem alias overlap refused")
    return cli, game, output


def copy_tree(source, destination, expected, readonly=False):
    require(not destination.exists(), "Copy destination occupied")
    destination.mkdir()
    for rel in expected["directories"]:
        (destination / rel).mkdir(parents=True, exist_ok=True)
    for rel, identity in expected["files"].items():
        src, dst = plain(source / rel), destination / rel
        require(file_hash(src) == identity, "Source changed before copy")
        with src.open("rb") as incoming, dst.open("xb") as outgoing:
            for chunk in iter(lambda: incoming.read(1024 * 1024), b""):
                outgoing.write(chunk)
        # Preserve executable bits, not hard links or original write permissions.
        mode = stat.S_IMODE(src.stat().st_mode)
        dst.chmod((mode & ~0o222) if readonly else (mode | stat.S_IRUSR | stat.S_IWUSR))
        require(file_hash(dst) == identity, "Copy hash mismatch")
    require(scan(destination) == expected and scan(source, source=True) == expected, "Copy inventory drift")
    if readonly and os.name != "nt":
        for rel in sorted(expected["directories"], key=lambda p: p.count("/"), reverse=True):
            (destination / rel).chmod(0o555)
        destination.chmod(0o555)


def permitted_config(options):
    """Current validated options, never a resume hash, authorize destinations."""
    providers = {}
    if options.test_endpoint is not None:
        providers[PROVIDER] = {"api_key": DUMMY_KEY, "model": MODEL,
                               "base_url": options.test_endpoint.rstrip("/"),
                               "extra": {"requests_per_minute": "4294967295"}}
    if options.test_fallback_endpoint is not None:
        providers[FALLBACK_TEST_PROVIDER] = {"api_key": DUMMY_KEY, "model": MODEL,
                                           "base_url": options.test_fallback_endpoint.rstrip("/"),
                                           "extra": {"requests_per_minute": "4294967295"}}
    return {"providers": providers}


def sha256_value(value):
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def fields(value, required, optional=()):
    require(isinstance(value, dict) and set(required) <= set(value)
            and set(value) <= set(required) | set(optional), "Malformed/unknown resume manifest fields")


def hash_identity(value):
    fields(value, {"sha256", "size"})
    require(sha256_value(value["sha256"]) and integer(value["size"]), "Malformed resume fingerprint")


def manifest_inventory(value):
    require(isinstance(value, dict) and value, "Malformed resume resource inventory")
    for key, sha in value.items():
        locator = strict_json(key)
        require(isinstance(locator, list) and len(locator) == 2 and nonblank(locator[1])
                and encoded(locator).decode() == key and sha256_value(sha), "Malformed resume resource identity")
        safe_rel(locator[0])


def validate_resume_manifest(value, seed_legacy=False):
    """Validate the ENTIRE resumable schema before trusting or republishing it.

    Copying, uncertain and post-injection states are refused outright; no proof
    or arbitrary extension fields from those states may enter a resumed run.
    Stage history permits only ordered, pending-only retries of known commands.
    Its hashes are evidence of drift, not authentication or config authority.
    """
    require(isinstance(value, dict) and value.get("owner") == OWNER
            and type(value.get("version")) is int and
            (value["version"] == VERSION or (seed_legacy and value["version"] == 1)),
            "Unrecognized ownership/version")
    phase = value.get("phase")
    require(isinstance(phase, str) and phase in RESUME_PHASES
            and value.get("injection_started") is False and value.get("uncertain_child") is False
            and value.get("active_command") is None, RECOVERY)
    spanish = phase.startswith("spanish-")
    required = {"owner", "version", "run_id", "phase", "settings", "binary", "source", "tool_sha256",
                "injection_started", "uncertain_child", "active_command", "status", "stages", "databases",
                "config", "japanese_inventory"}
    if spanish:
        required.add("spanish_inventory")
    legacy = value["version"] == 1
    if not legacy:
        required.add("accepted_rows")
    fields(value, required, {"diagnostic"} if legacy else {"diagnostic", "seed"})
    require(isinstance(value["run_id"], str)
            and re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", value["run_id"]),
            "Malformed resume run ID")
    require(isinstance(value["status"], str) and value["status"] in {"running", "failed"}, "Malformed resume status")
    require((value["status"] == "failed" and nonblank(value.get("diagnostic")))
            or (value["status"] == "running" and "diagnostic" not in value), "Malformed resume diagnostic/status")
    require(sha256_value(value["tool_sha256"]), "Malformed resume tool hash")
    settings = value["settings"]
    setting_keys = {"cli", "game", "output", "format", "provider", "test_endpoint", "batch_size", "concurrency", "timeout"}
    if not legacy:
        setting_keys |= {"fallback_provider", "english_seed", "test_fallback_endpoint"}
    fields(settings, setting_keys)
    if not legacy:
        require(settings["fallback_provider"] is None or
                (settings["fallback_provider"] == "google" and settings["provider"] == "antigravity"),
                "Malformed resume fallback")
        require(settings["english_seed"] is None or nonblank(settings["english_seed"]), "Malformed resume seed setting")
        require(settings["test_fallback_endpoint"] is None or
                (nonblank(settings["test_fallback_endpoint"]) and settings["fallback_provider"] == "google"
                 and nonblank(settings["test_endpoint"])
                 and settings["test_endpoint"].rstrip("/") != settings["test_fallback_endpoint"].rstrip("/")),
                "Malformed resume test fallback")
        require(not (settings["fallback_provider"] and settings["test_endpoint"]) or
                settings["test_fallback_endpoint"] is not None, "Missing resume test fallback")
    require(all(nonblank(settings[key]) for key in ("cli", "game", "output", "format", "provider"))
            and settings["format"] in {"renpy", "rpgmaker-mv", "kirikiri"}
            and settings["provider"] in {"google", "antigravity"}
            and (settings["test_endpoint"] is None or nonblank(settings["test_endpoint"]))
            and type(settings["batch_size"]) is int and 1 <= settings["batch_size"] <= 200
            and type(settings["concurrency"]) is int and 1 <= settings["concurrency"] <= 16
            and type(settings["timeout"]) in (int, float) and 1 <= settings["timeout"] <= 86400
            and math.isfinite(settings["timeout"]), "Malformed resume settings")
    hash_identity(value["binary"])
    hash_identity(value["config"])
    source = value["source"]
    fields(source, {"files", "directories"})
    require(isinstance(source["files"], dict) and isinstance(source["directories"], list)
            and all(isinstance(path, str) for path in source["directories"])
            and source["directories"] == sorted(set(source["directories"])), "Malformed resume source tree")
    for path in source["directories"]:
        safe_rel(path)
    for path, identity in source["files"].items():
        safe_rel(path)
        hash_identity(identity)
    manifest_inventory(value["japanese_inventory"])
    count = len(value["japanese_inventory"])
    if spanish:
        manifest_inventory(value["spanish_inventory"])
        require(len(value["spanish_inventory"]) == count, "Resume pivot inventory count mismatch")
    databases = value["databases"]
    fields(databases, {"english.db", "spanish.db"} if spanish else {"english.db"})
    for snapshot in databases.values():
        fields(snapshot, {"rows", "logical_sha256", "files"})
        require(integer(snapshot["rows"]) and snapshot["rows"] == count
                and sha256_value(snapshot["logical_sha256"]), "Malformed resume database count/hash")
        fields(snapshot["files"], {""}, {"-wal", "-journal"})
        for identity in snapshot["files"].values():
            hash_identity(identity)

    # Ordered command graph. A retry returns through the checkpoint's current
    # validation; extraction and pivot are never repeated. Only successful,
    # classified validation stages can advance a readiness checkpoint.
    successors = {
        None: {"cli-capability"},
        "cli-capability": {"extract-japanese"},
        "extract-japanese": {"ja-en"},
        "ja-en": {"ja-en", "validate-english"},
        "validate-english": {"ja-en", "validate-english-current"},
        "validate-english-current": {"validate-english-current", "strict-pivot", "en-es", "validate-spanish-current"},
        "strict-pivot": {"en-es", "validate-english-current"},
        "en-es": {"validate-spanish", "validate-english-current"},
        "validate-spanish": {"validate-spanish-current", "validate-english-current"},
        "validate-spanish-current": {"validate-english-current"},
    }
    if not legacy:
        successors["extract-japanese"] |= {"seed-export-english", "ja-en"}
        successors["seed-export-english"] = {"seed-import-english"}
        successors["seed-import-english"] = {"validate-seed-english"}
        successors["validate-seed-english"] = {"ja-en"}
        for translation, validation, language in (("ja-en", "validate-english", "english"),
                                                   ("en-es", "validate-spanish", "spanish")):
            partial = "validate-" + language + "-partial"
            fallback = translation + "-fallback-google"
            successors[translation].add(partial)
            successors[partial] = {fallback}
            successors[fallback] = {validation, translation, "validate-english-current"}
        successors["en-es"].add("en-es")
        successors["validate-spanish"].add("en-es")
    stages = value["stages"]
    require(isinstance(stages, list) and len(stages) >= 2, "Malformed resume stage history")
    previous, checkpoint, pivot_seen = None, "copying", False
    prior = None
    for stage in stages:
        fields(stage, {"stage", "exit_code", "seconds", "stdout_sha256", "stderr_sha256"},
               {"classified_identical_resources"})
        name, code = stage["stage"], stage["exit_code"]
        require(isinstance(name, str) and name in successors[previous], "Unordered/unknown resume stage")
        require(type(code) is int and -255 <= code <= 4294967295
                and type(stage["seconds"]) in (int, float) and 0 <= stage["seconds"] <= sys.float_info.max
                and math.isfinite(stage["seconds"]) and sha256_value(stage["stdout_sha256"])
                and sha256_value(stage["stderr_sha256"]), "Malformed resume stage result")
        classified = "classified_identical_resources" in stage
        if classified:
            identical = stage["classified_identical_resources"]
            require(name.startswith("validate-") and integer(identical) and identical <= count
                    and code == (1 if identical else 0), "Malformed resume classified validation count/code")
        if name in {"cli-capability", "extract-japanese", "strict-pivot", "seed-export-english", "seed-import-english"}:
            require(code == 0, "Failed resume prerequisite stage")
        if name == "extract-japanese":
            checkpoint = "english-pending"
        elif name in {"ja-en", "ja-en-fallback-google"}:
            require(checkpoint == "english-pending", "JA retry after readiness checkpoint")
        elif name == "validate-english":
            require(prior["exit_code"] == 0, "Validation after failed translation")
            if classified:
                checkpoint = "english-ready"
        elif name == "validate-english-current":
            require(checkpoint in {"english-ready", "spanish-pending", "spanish-ready"}, "Premature English readiness stage")
        elif name == "strict-pivot":
            require(checkpoint == "english-ready" and not pivot_seen
                    and "classified_identical_resources" in prior, "Unverified/repeated resume pivot")
            checkpoint, pivot_seen = "spanish-pending", True
        elif name in {"en-es", "en-es-fallback-google"}:
            require(checkpoint == "spanish-pending" and pivot_seen
                    and (previous in {"strict-pivot", "en-es", "en-es-fallback-google"}
                         or "classified_identical_resources" in prior),
                    "Premature Spanish translation stage")
        elif name == "validate-spanish":
            require(prior["exit_code"] == 0, "Validation after failed translation")
            if classified:
                checkpoint = "spanish-ready"
        elif name == "validate-spanish-current":
            require(checkpoint == "spanish-ready" and "classified_identical_resources" in prior,
                    "Premature Spanish readiness stage")
        if name.startswith("seed-") or name == "validate-seed-english":
            require(nonblank(settings.get("english_seed")), "Unauthorized seed stage")
        if name.endswith("-partial") or name == "validate-seed-english":
            require(prior["exit_code"] == 0, "Partial validation after failed prerequisite")
        if name.endswith("-fallback-google"):
            require(settings.get("fallback_provider") == "google" and prior["exit_code"] in (0, 1)
                    and "classified_identical_resources" in prior, "Unproven fallback stage")
        if name == "ja-en" and previous == "validate-seed-english":
            require("classified_identical_resources" in prior, "Unvalidated seed targets")
        previous, prior = name, stage
    require(checkpoint == phase and pivot_seen == spanish, "Resume phase/history mismatch")
    if not legacy:
        validate_accepted_manifest(value, count)


def stage_provider(settings, stage):
    if stage in {"ja-en", "en-es"}:
        return PROVIDER if settings["test_endpoint"] else settings["provider"]
    if stage in {"ja-en-fallback-google", "en-es-fallback-google"}:
        require(settings["fallback_provider"] == "google", "Unauthorized fallback attribution")
        return FALLBACK_TEST_PROVIDER if settings["test_fallback_endpoint"] else "google"
    if stage == "seed-import-english":
        return "import"
    raise PipelineError("Unknown attribution stage")


def validate_accepted_manifest(value, count):
    accepted = value["accepted_rows"]
    fields(accepted, {"english.db", "spanish.db"})
    require(all(isinstance(rows, dict) and len(rows) <= count for rows in accepted.values()),
            "Malformed accepted row evidence")
    successful = {s["stage"] for s in value["stages"] if s["exit_code"] == 0}
    seed = value.get("seed")
    if seed is not None:
        fields(seed, {"run_id", "database", "manifest_sha256", "snapshot", "snapshot_logical_sha256", "catalog", "targets"})
        require(isinstance(seed["run_id"], str)
                and re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", seed["run_id"])
                and sha256_value(seed["manifest_sha256"]), "Malformed seed ownership evidence")
        hash_identity(seed["database"])
        hash_identity(seed["snapshot"])
        require(sha256_value(seed["snapshot_logical_sha256"]), "Malformed seed snapshot hash")
        hash_identity(seed["catalog"])
        require(isinstance(seed["targets"], dict) and seed["targets"] and len(seed["targets"]) <= count,
                "Malformed seed targets")
        for identifier, target in seed["targets"].items():
            require(nonblank(identifier), "Malformed seed ID")
            fields(target, {"source_sha256", "target_sha256", "provider", "status"})
            require(sha256_value(target["source_sha256"]) and sha256_value(target["target_sha256"])
                    and isinstance(target["provider"], str)
                    and target["provider"] in {"antigravity", "google", PROVIDER, FALLBACK_TEST_PROVIDER}
                    and isinstance(target["status"], str) and target["status"] in READY, "Malformed seed target")
    require(("seed-import-english" in successful) == (seed is not None)
            and (value["settings"]["english_seed"] is not None) == (seed is not None), "Unbound/incomplete seed history")
    if value["phase"] != "english-pending":
        require(len(accepted["english.db"]) == count, "Incomplete English readiness attribution")
    if value["phase"] == "spanish-ready":
        require(len(accepted["spanish.db"]) == count, "Incomplete Spanish readiness attribution")
    for database, rows in accepted.items():
        permitted = {"ja-en", "ja-en-fallback-google", "seed-import-english"} if database == "english.db" else {"en-es", "en-es-fallback-google"}
        for identifier, evidence in rows.items():
            require(nonblank(identifier), "Malformed accepted ID")
            fields(evidence, {"row_sha256", "stage", "provider"})
            require(sha256_value(evidence["row_sha256"]) and isinstance(evidence["stage"], str)
                    and evidence["stage"] in permitted & successful
                    and evidence["provider"] == stage_provider(value["settings"], evidence["stage"]),
                    "Unproven accepted row attribution")
            if evidence["provider"] == "import":
                require(seed is not None and identifier in seed["targets"], "Unbound imported row")


def child_environment(root, options, subscription_login=True):
    # Allowlist prevents inherited LOCUST_CONFIG, provider keys and unknown
    # profile/endpoint overrides. PATH keeps the subscription CLI discoverable.
    keep = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "SYSTEMDRIVE", "LANG", "LC_ALL"}
    subscription = subscription_login and options.provider == "antigravity" and options.test_endpoint is None
    profile_keys = {"HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"}
    if subscription:
        # Parent-authorized: agy owns its existing login. No credential file is
        # read/copied by Python. Locust OAuth token_path uses LOCUST_DATA_DIR,
        # through AppConfig::config_dir, before any inherited home fallback.
        keep |= profile_keys
    env = {key: value for key, value in os.environ.items() if key.upper() in keep}
    defaults = {key: str(root / "profile") for key in profile_keys}
    # Windows KnownFolder APIs require these standard directories to exist for
    # the synthetic USERPROFILE used by Google and the loopback route.
    defaults["APPDATA"] = str(root / "profile/AppData/Roaming")
    defaults["LOCALAPPDATA"] = str(root / "profile/AppData/Local")
    for key, value in defaults.items():
        if not subscription or key not in env:
            env[key] = value
    env.update({"LOCUST_DATA_DIR": str(root / "profile"), "LOCUST_BACKUP_ROOT": str(root / "backups"),
                "TMP": str(root / "temp"), "TEMP": str(root / "temp"), "TMPDIR": str(root / "temp")})
    if options.test_endpoint:
        env["NO_PROXY"] = "127.0.0.1,::1"
    return env


def read_database(path):
    """Read own DB in one snapshot; WAL is visible (deliberately NOT immutable)."""
    path = plain(path)
    for suffix in ("-wal", "-shm", "-journal"):
        plain(str(path) + suffix, missing=True)
    require(path.is_file() and path.stat().st_nlink == 1, "Owned database missing/hard-linked")
    connection = None
    try:
        connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=5)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA query_only=ON")
        connection.execute("BEGIN")
        schema = dict(connection.execute("SELECT name, type FROM sqlite_master WHERE name IN ('strings','project_metadata','translation_memory')"))
        require(schema == {"strings": "table", "project_metadata": "table", "translation_memory": "table"}, "Unexpected database schema")
        require(connection.execute("PRAGMA quick_check").fetchone()[0] == "ok", "Database integrity failure")
        rows = [dict(value) for value in connection.execute("SELECT " + ",".join(COLUMNS) + " FROM strings ORDER BY id")]
        metadata = {}
        for key, value in connection.execute("SELECT key,value FROM project_metadata"):
            require(key not in metadata, "Duplicate project metadata")
            metadata[key] = strict_json(value)
        require(metadata.get("extraction_warnings") == [], "Extraction warnings missing/malformed/nonempty")
        require(connection.execute("SELECT COUNT(*) FROM translation_memory").fetchone()[0] == 0, "Translation memory must remain disabled")
        for value in rows:
            value["tags"] = strict_json(value["tags"])
            value["metadata"] = strict_json(value["metadata"])
        return rows, metadata
    except sqlite3.Error as error:
        raise PipelineError("Owned database cannot be read/validated") from error
    finally:
        if connection:
            connection.close()


def inventory(rows, root, format_id):
    require(isinstance(rows, list) and rows, "Empty/shrunk resource inventory")
    result = {}
    for value in rows:
        require(isinstance(value, dict) and all(key in value for key in COLUMNS), "Malformed row schema")
        require(nonblank(value["id"]) and nonblank(value["source"]), "Blank resource identity/source")
        path = relative(value["file_path"], root)
        require(isinstance(value["tags"], list) and all(isinstance(tag, str) for tag in value["tags"]), "Malformed tags")
        metadata = value["metadata"]
        require(isinstance(metadata, dict) and all(isinstance(key, str) for key in metadata), "Malformed format metadata")
        if "format_id" in metadata:
            require(metadata["format_id"] == format_id, "Malformed/mismatched format metadata")
        else:
            # Current loose Ren'Py/KAG extractors predate the explicit stamp.
            # Accept absence ONLY for their unambiguous physical locators.
            legacy = ((format_id == "renpy" and path.endswith(".rpy") and re.search(r"#\d+$", value["id"]))
                      or (format_id == "kirikiri" and path.endswith(".ks") and re.search(r"#kag:\d+(?::attr:\d+:[\w]+)?$", value["id"])))
            require(legacy, "Missing recognized format provenance")
        require("locust_stale_translation" not in metadata, "Stale/malformed translation provenance")
        require(not any("warning" in key.lower() or key == "iostore_unread_containers" for key in metadata), "Unexpected row extraction warning metadata")
        if "locust_injection_source" in metadata:
            require(nonblank(metadata["locust_injection_source"]), "Malformed physical provenance")
        require("locust_injection_capacity" not in metadata, "Binary-slot provenance unsupported in loose pipeline")
        require(value["context"] is None or isinstance(value["context"], str), "Malformed context")
        require(value["char_limit"] is None or integer(value["char_limit"]), "Malformed character limit")
        require(nonblank(value["created_at"]), "Malformed creation timestamp")
        require(isinstance(value["status"], str) and value["status"] in {"pending", "translated", "reviewed", "approved", "error"}, "Malformed status")
        key = encoded([path, value["id"]]).decode()
        require(key not in result, "Duplicate resource identity")
        result[key] = digest({field: value[field] for field in STATIC_FIELDS})
    return dict(sorted(result.items()))


def kag_controls(text):
    # Conservative KAG tag scan with nested brackets and quoted arguments.
    # An unmatched tag fails, rather than guessing at a readable fragment.
    tokens, position = [], 0
    while position < len(text):
        if text[position] != "[":
            position += 1
            continue
        start, depth, quote = position, 0, None
        while position < len(text):
            char = text[position]
            if quote:
                if char == quote and (position == 0 or text[position - 1] != "\\"):
                    quote = None
            elif char in "\"'":
                quote = char
            elif char == "[":
                depth += 1
            elif char == "]":
                depth -= 1
                if depth == 0:
                    position += 1
                    break
            position += 1
        require(depth == 0 and quote is None, "Malformed KAG tag")
        tokens.append(text[start:position])
    return tokens


def gate_rows(rows, baseline, root, format_id, ready=False):
    require(inventory(rows, root, format_id) == baseline, "Exact resource inventory/source/metadata drift")
    for value in rows:
        translation = value["translation"]
        require(translation is None or isinstance(translation, str), "Malformed translation")
        if ready:
            require(nonblank(translation) and value["status"] in {"translated", "reviewed", "approved"}, "Missing/blank/pending translation")
            require(nonblank(value["provider_used"]) and nonblank(value["translated_at"]), "Missing translation attribution")
        else:
            require(value["status"] in {"pending", "translated", "reviewed", "approved", "error"}, "Invalid pending state")
            require(translation is None or nonblank(translation), "Blank saved translation")
        if format_id == "kirikiri" and translation is not None:
            physical = value["metadata"].get("locust_injection_source", value["source"])
            require(kag_controls(physical) == kag_controls(translation)
                    and kag_controls(value["source"]) == kag_controls(translation), "Altered KAG control tag")
    return rows


def pivot_inventory(english, root, format_id):
    expected = []
    for prior in english:
        value = dict(prior)
        value["metadata"] = dict(prior["metadata"])
        value["metadata"]["locust_injection_source"] = prior["metadata"].get("locust_injection_source", prior["source"])
        value["source"] = prior["translation"]
        expected.append(value)
    return inventory(expected, root, format_id)


def gate_validation(report, rows=None):
    require(isinstance(report, dict) and set(report) == {"count", "issues"}
            and integer(report["count"]) and isinstance(report["issues"], list)
            and report["count"] == len(report["issues"]), "Validation issues/malformed report")
    by_id = {value["id"]: value for value in (rows or [])}
    seen = set()
    for issue in report["issues"]:
        require(isinstance(issue, dict) and set(issue) == {"entry_id", "kind", "message"}
                and issue["kind"] == "IdenticalToSource"
                and issue["message"] == "translation is identical to source", "Validation issue cannot release")
        identifier = issue["entry_id"]
        require(isinstance(identifier, str) and identifier in by_id and identifier not in seen,
                "Validation resource identity mismatch")
        value = by_id[identifier]
        require(nonblank(value["translation"]) and value["translation"] == value["source"], "Unproven identical validation issue")
        seen.add(identifier)


def gate_injection(report, rows, root, backups, informational_notice=None):
    require(isinstance(report, dict) and report.get("mode") == "direct", "Malformed Direct injection report")
    require(report.get("languages_processed") == ["es"] and report.get("languages_failed") == [], "Injection language failure")
    warnings = report.get("warnings")
    require(isinstance(warnings, list) and (warnings == [] or
            (informational_notice is not None and warnings == [informational_notice])), "Injection warning")
    nested = report.get("reports")
    require(isinstance(nested, dict) and set(nested) == {"es"} and isinstance(nested["es"], dict), "Malformed per-language report")
    inner = nested["es"]
    for key in ("files_modified", "strings_written", "strings_skipped"):
        require(integer(report.get(key)) and integer(inner.get(key)) and report[key] == inner[key], "Malformed/inconsistent injection count")
    require(inner.get("warnings") == warnings, "Per-language injection warning")
    written, skipped = report["strings_written"], report["strings_skipped"]
    require(written + skipped == len(rows), "Injection inventory count mismatch")
    reasons = inner.get("skip_reasons")
    require(isinstance(reasons, dict) and all(integer(count) for count in reasons.values())
            and set(reasons) <= {"unchanged"} and sum(reasons.values()) == skipped, "Unclassified/unsafe injection skip")
    unchanged = sum(value["translation"] == value["metadata"].get("locust_injection_source", value["source"]) for value in rows)
    require(skipped <= unchanged, "Unchanged skip lacks physical equivalence")
    paths = report.get("files_written")
    require(isinstance(paths, list) and paths == inner.get("files_written"), "Malformed file inventory")
    rels = [relative(path, root) for path in paths]
    require(len(set(rels)) == len(rels) == report["files_modified"] and rels, "Empty/duplicate injection output")
    require(not any(path.split("/")[0].lower() in RESERVED for path in rels), "Reserved injection output")
    outcomes = report.get("outcomes")
    require(outcomes == [["es", {"Recorded": {"files": len(rels)}}]]
            and integer(outcomes[0][1]["Recorded"]["files"]), "Recording outcome cannot release patch")
    for key in ("backup_id", "pristine_backup_id"):
        require(nonblank(report.get(key)), "Missing backup provenance")
    relative(report.get("backup_path"), backups)
    return set(rels)


def fingerprint_identity(value):
    require(isinstance(value, dict) and set(value) == {"sha256", "size", "readonly", "unix_mode"}
            and isinstance(value["sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", value["sha256"])
            and integer(value["size"]) and type(value["readonly"]) is bool
            and (value["unix_mode"] is None or integer(value["unix_mode"])), "Malformed transaction fingerprint")
    return {"sha256": value["sha256"], "size": value["size"]}


def verify_injection_evidence(report, root, backups, original, format_id):
    """Derive the ONLY informational notice from independently verified state.

    This proof is provisional: completion still requires full work/install/final
    resource equivalence. Never extract a path or trust success from warning text.
    """
    require(isinstance(report, dict), "Malformed injection report")
    active = read_json_file(root / ".locust-injections/active.json")
    require(isinstance(active, dict) and type(active.get("schema_version")) is int
            and active["schema_version"] == 1 and active.get("mode") == "direct"
            and active.get("language") == "es" and active.get("format") == format_id
            and norm(active.get("game_root")) == norm(root), "Injection transaction identity mismatch")
    transaction = active.get("transaction_id")
    require(isinstance(transaction, str) and re.fullmatch(r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", transaction), "Malformed transaction ID")
    operations = plain(root / ".locust-injections/operations")
    require({path.name for path in operations.iterdir()} == {transaction}, "Unexpected injection generations")
    operation = plain(operations / transaction)
    require(read_json_file(operation / "identity.json") == active
            and read_json_file(operation / "phase.json") == "completed", "Injection transaction is not completed")
    plan = read_json_file(operation / "plan.json")
    require(isinstance(plan, dict) and type(plan.get("schema_version")) is int and plan["schema_version"] == 1
            and plan.get("transaction_id") == transaction and norm(plan.get("game_root")) == norm(root)
            and nonblank(plan.get("prepared_at")), "Malformed injection plan provenance")
    require(isinstance(plan.get("files"), list) and plan["files"], "Empty injection plan")
    changes, retained = {}, {}
    for ordinal, change in enumerate(plan["files"]):
        require(isinstance(change, dict) and set(change) == {"path", "original", "result"}, "Malformed injection change")
        path = safe_rel(change["path"])
        require(path not in changes and path.split("/")[0].lower() not in RESERVED, "Duplicate/reserved injection change")
        result = fingerprint_identity(change["result"])
        require(file_hash(root / path) == result, "Injection result hash mismatch")
        prior = original["files"].get(path)
        if prior is None:
            require(change["original"] is None, "Unproven injection addition")
        else:
            require(fingerprint_identity(change["original"]) == prior, "Injection original provenance mismatch")
            require(file_hash(operation / "originals" / str(ordinal)) == prior, "Retained injection original hash mismatch")
            retained[str(ordinal)] = prior
        changes[path] = result
    require(scan(operation / "originals") == {"files": retained, "directories": []}, "Unexpected retained originals inventory")
    reported = report.get("files_written")
    require(isinstance(reported, list) and {relative(path, root) for path in reported} == set(changes), "Injection transaction/report inventory mismatch")
    created = plan.get("created_dirs")
    require(isinstance(created, list) and all(isinstance(path, str) for path in created)
            and len(set(created)) == len(created), "Malformed injection directories")
    for path in created:
        require(safe_rel(path).split("/")[0].lower() not in RESERVED
                and path not in original["directories"] and plain(root / path).is_dir(), "Unproven injection directory")

    # Independently check the whole V2 pre-write backup, not just the changed
    # ordinal originals. All paths are derived locally; foreign report paths
    # are rejected BEFORE opening them.
    identifier = report.get("backup_id")
    require(isinstance(identifier, str) and safe_rel(identifier) == identifier and "/" not in identifier
            and report.get("pristine_backup_id") == identifier, "Malformed/mismatched backup ID")
    backup = backups / identifier
    require(norm(report.get("backup_path")) == norm(backup), "Backup path outside owned generation")
    manifest = read_json_file(backup / "manifest.json")
    require(isinstance(manifest, dict) and type(manifest.get("version")) is int and manifest["version"] == 2
            and manifest.get("source_kind") == "directory" and norm(manifest.get("source_path")) == norm(root)
            and nonblank(manifest.get("created_at")) and integer(manifest.get("file_count"))
            and manifest["file_count"] == len(original["files"]) and integer(manifest.get("size_bytes"))
            and manifest["size_bytes"] == sum(value["size"] for value in original["files"].values()), "Malformed backup provenance/counts")
    require(isinstance(manifest.get("inventory"), dict), "Malformed backup inventory")
    expected_inventory = {path: {"kind": "directory"} for path in original["directories"]}
    expected_inventory.update({path: {"kind": "file", **identity} for path, identity in original["files"].items()})
    actual_inventory = {}
    for path, value in manifest["inventory"].items():
        path = safe_rel(norm(path))
        require(path not in actual_inventory and isinstance(value, dict), "Duplicate/malformed backup entry")
        if value.get("kind") == "file":
            require(integer(value.get("size")), "Malformed backup size")
        actual_inventory[path] = value
    require(actual_inventory == expected_inventory and scan(backup / "payload") == original, "Backup payload/manifest hash mismatch")
    require(scan(backup)["files"].keys() == {"manifest.json", *("payload/" + path for path in original["files"])}, "Unexpected backup files")
    notice = f"injection {transaction} completed; verified originals retained at {operation / 'originals'}"
    evidence = {"transaction_id": transaction, "retained_originals_verified": True,
                "transaction_plan_sha256": file_hash(operation / "plan.json")["sha256"],
                "backup_manifest_sha256": file_hash(backup / "manifest.json")["sha256"]}
    return notice, evidence


def resource_slots(rows, root, format_id):
    """Bounded qa_replay_oracle pattern, not a general identity migration."""
    result, groups = {}, defaultdict(list)
    for value in rows:
        path = relative(value["file_path"], root)
        identifier = norm(value["id"])
        prefix = norm(root)
        if identifier.lower().startswith(prefix.lower() + "/"):
            identifier = identifier[len(prefix) + 1:]
        command = re.fullmatch(r"(.*)#cmd_(\d+)#(.*)", identifier) if format_id == "rpgmaker-mv" else None
        if command:
            groups[(path, command[1])].append((int(command[2]), command[3], value))
        else:
            key = (path, identifier)
            require(key not in result, "Duplicate reextraction identity")
            result[key] = value
    for group, values in groups.items():
        require(len({(index, kind) for index, kind, _ in values}) == len(values), "Duplicate structural slot")
        for ordinal, (_, kind, value) in enumerate(sorted(values, key=lambda item: (item[0], item[1]))):
            result[(*group, ordinal, kind)] = value
    return result


def compare_reextraction(expected, expected_root, actual, actual_root, format_id):
    wanted = resource_slots(expected, expected_root, format_id)
    found = resource_slots(actual, actual_root, format_id)
    require(wanted.keys() == found.keys(), "Reextraction resource identities differ")
    normalized = 0
    for key, value in wanted.items():
        source, translation = found[key]["source"], value["translation"]
        if source == translation:
            continue
        # Only message/description author wrapping, never generic whitespace
        # folding or choice/name text. Structural order and kind remain exact.
        wrapping = format_id == "rpgmaker-mv" and (value["id"].endswith("#msg") or value["id"].endswith("#description"))
        physical = value["metadata"].get("locust_injection_source", value["source"])
        require(wrapping and "\n" in physical and source.replace("\n", " ") == translation,
                "Reextracted Spanish differs")
        normalized += 1
    return {"resources_verified": len(wanted), "normalized_rpg_dialogues": normalized}


def indexed(entries):
    require(isinstance(entries, list), "Malformed artifact entries")
    result = {}
    for entry in entries:
        require(isinstance(entry, dict), "Malformed artifact entry")
        path = safe_rel(entry.get("path"))
        require(path not in result and path.split("/")[0].lower() not in RESERVED, "Duplicate/reserved artifact path")
        result[path] = entry
    return result


def verify_zip(path, original, work, format_id, reported):
    """Stream every payload and compare exactly to the recorded tree delta."""
    delta = {key for key, value in work["files"].items() if original["files"].get(key) != value}
    require(delta and set(original["files"]) <= set(work["files"]), "Empty patch or deleted game file")
    require(delta == reported, "Injection report does not describe full game delta")
    try:
        with zipfile.ZipFile(plain(path)) as archive:
            names = archive.namelist()
            require(len(set(names)) == len(names) and set(names) == delta | {"locust-patch.json", "README.txt"}, "Unexpected ZIP inventory")
            for info in archive.infolist():
                safe_rel(info.filename)
                mode = stat.S_IFMT(info.external_attr >> 16)
                require(not info.is_dir() and mode in (0, stat.S_IFREG) and not (info.flag_bits & 1), "Unsafe ZIP member")
            require(archive.getinfo("locust-patch.json").file_size <= 8 * 1024 * 1024, "Patch manifest too large")
            manifest = strict_json(archive.read("locust-patch.json"))
            require(isinstance(manifest, dict) and type(manifest.get("schema_version")) is int
                    and manifest["schema_version"] == 1 and manifest.get("engine") == format_id
                    and manifest.get("language") == "es", "Malformed/mismatched patch manifest")
            for key in ("patch_id", "patch_version", "generator_version", "created_at", "game_name"):
                require(nonblank(manifest.get(key)), "Missing patch metadata")
            entries = indexed(manifest.get("files"))
            require(set(entries) == delta, "Manifest inventory mismatch")
            strict = False
            for rel, entry in entries.items():
                sha, size = hashlib.sha256(), 0
                with archive.open(rel) as stream:
                    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                        sha.update(chunk)
                        size += len(chunk)
                identity = {"sha256": sha.hexdigest(), "size": size}
                require(identity == work["files"][rel] and integer(entry.get("size"))
                        and entry["size"] == size and entry.get("patched_sha256") == identity["sha256"], "ZIP payload hash/size mismatch")
                prior = original["files"].get(rel)
                require(type(entry.get("added", False)) is bool, "Malformed added flag")
                if prior:
                    strict = True
                    require(entry.get("added", False) is False and entry.get("original_sha256") == prior["sha256"], "Pristine manifest hash mismatch")
                else:
                    require(entry.get("added") is True and entry.get("original_sha256") is None, "Unproven patch addition")
            require(strict, "Structural/added-only patch unsupported")
            return manifest
    except (zipfile.BadZipFile, RuntimeError, OSError) as error:
        raise PipelineError("Patch ZIP cannot be verified") from error


def read_json_file(path):
    path = plain(path)
    require(path.stat().st_size <= 16 * 1024 * 1024, "JSON artifact too large")
    return strict_json(path.read_bytes())


def verify_install(root, original, manifest, work):
    tree = scan(root)
    receipt = read_json_file(root / ".locust/receipt.json")
    backup = read_json_file(root / ".locust/backup/manifest.json")
    require(isinstance(receipt, dict) and type(receipt.get("schema_version")) is int
            and receipt["schema_version"] == 1 and receipt.get("verification") == "strict"
            and receipt.get("baseline") == "pristine" and receipt.get("forced") is False, "Install receipt is not strict pristine")
    for key in ("patch_id", "patch_version", "generator_version", "language", "engine"):
        require(receipt.get(key) == manifest[key], "Install receipt metadata mismatch")
    require(nonblank(receipt.get("applied_at")), "Missing install timestamp")
    entries = indexed(manifest["files"])
    replaced = {path: entry for path, entry in entries.items() if path in original["files"]}
    added = {path: entry for path, entry in entries.items() if path not in original["files"]}
    actual_replaced, actual_added = indexed(receipt.get("replaced")), indexed(receipt.get("added"))
    require(actual_replaced == {path: {"path": path, "original_sha256": entry["original_sha256"], "patched_sha256": entry["patched_sha256"]} for path, entry in replaced.items()}
            and actual_added == {path: {"path": path, "patched_sha256": entry["patched_sha256"]} for path, entry in added.items()}, "Receipt file hashes/inventory mismatch")
    require(isinstance(backup, dict) and type(backup.get("schema_version")) is int
            and backup["schema_version"] == 1 and backup.get("baseline") == "pristine"
            and nonblank(backup.get("created_at")), "Malformed backup manifest")
    backups = indexed(backup.get("files"))
    require(backups == {path: {"path": path, **original["files"][path]} for path in replaced}, "Backup inventory/hash mismatch")
    expected = dict(original["files"])
    for path in entries:
        expected[path] = work["files"][path]
    for path in replaced:
        expected[".locust/backup/files/" + path] = original["files"][path]
    for path in (".locust/receipt.json", ".locust/backup/manifest.json"):
        expected[path] = tree["files"][path]
    require(tree["files"] == expected, "Installed or backup bytes drift/unexpected files")
    game_dirs = {directory for directory in tree["directories"] if directory != ".locust" and not directory.startswith(".locust/")}
    wanted_dirs = set(original["directories"])
    for path in entries:
        wanted_dirs.update(parent.as_posix() for parent in Path(path).parents if str(parent) != ".")
    require(game_dirs == wanted_dirs, "Unexpected installed game directories")
    created = receipt.get("created_dirs")
    require(isinstance(created, list) and len(set(created)) == len(created)
            and all(safe_rel(path) in wanted_dirs - set(original["directories"]) for path in created), "Malformed created directories receipt")
    return {"verification": "strict", "baseline": "pristine", "replaced": len(replaced), "added": len(added),
            "receipt_sha256": tree["files"][".locust/receipt.json"]["sha256"]}


def seed_owner(path, original, format_id, output, game):
    """Manifest is closed data evidence, NEVER executable/config/path authority."""
    path = plain(path)
    require(path.is_absolute() and path.name == "english.db", "Seed must be owned english.db")
    root = path.parent
    require(not any(under(root, p) or under(p, root) for p in (output, game)), "Conflicting seed root")
    for name in ("pipeline.lock", "pipeline-manifest.next", PUBLICATION_FAILURE, "spanish.db",
                 "spanish.db-wal", "spanish.db-shm", "spanish.db-journal", "spanish-patch.zip"):
        require(not plain(root / name, missing=True).exists(), "Seed active/pivot/publication remnant refused")
    for suffix in ("-wal", "-shm", "-journal"):
        require(not plain(str(path) + suffix, missing=True).exists(), "Seed SQLite sidecar refused")
    identity = publication_file(path)
    owner_path = root / "pipeline-manifest.json"
    owner_identity = publication_file(owner_path)
    manifest = read_json_file(owner_path)
    validate_resume_manifest(manifest, seed_legacy=True)
    require(manifest["status"] == "failed" and manifest["phase"] in {"english-pending", "english-ready"}
            and "seed" not in manifest and manifest["settings"]["format"] == format_id
            and norm(manifest["settings"]["output"]) == norm(root)
            and manifest["source"] == original, "Seed owner/source/format mismatch")
    recorded_files = manifest["databases"]["english.db"]["files"]
    require(recorded_files.get("") == {k: identity[k] for k in ("sha256", "size")}
            and set(recorded_files) <= {"", "-wal"}
            and ("-wal" not in recorded_files or recorded_files["-wal"] ==
                 {"sha256": hashlib.sha256(b"").hexdigest(), "size": 0}),
            "Seed database fingerprint/nonempty recorded WAL mismatch")
    require(scan(root / "work-game", source=True) == original
            and scan(root / "pristine", source=True) == original
            and scan(root / "install-game", source=True) == original, "Seed physical Japanese provenance drift")
    return path, identity, owner_path, owner_identity, manifest


def seed_database_evidence(path, providers):
    """Only called on the exclusive snapshot, never on the input DB."""
    connection = None
    try:
        connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
        connection.execute("PRAGMA query_only=ON")
        require(connection.execute("SELECT COUNT(*) FROM injected_files").fetchone()[0] == 0,
                "Seed injection recording refused")
        runs = list(connection.execute("SELECT provider,source_lang,target_lang,strings_translated FROM translation_runs"))
        require(runs and all(p in providers and s == "ja" and t == "en" and integer(n) for p, s, t, n in runs),
                "Seed translation-run language/provider evidence malformed")
        return {p for p, _, _, n in runs if n > 0}
    except sqlite3.Error as error:
        raise PipelineError("Seed provenance tables malformed") from error
    finally:
        if connection is not None:
            connection.close()


def remapped_inventory(rows, old_root, new_root, format_id):
    # IDs, context and metadata are NEVER normalized. Only the known root in
    # file_path changes. This deliberately refuses root-dependent extractor IDs.
    inventory(rows, old_root, format_id)
    mapped = [dict(r, file_path=str(new_root / relative(r["file_path"], old_root))) for r in rows]
    return inventory(mapped, new_root, format_id)


def verify_seed_catalog(path, targets):
    path = plain(path)
    require(path.stat().st_size <= 64 * 1024 * 1024, "Seed catalog size refused")
    data = path.read_bytes()
    require(b"<!" not in data, "Seed catalog declaration refused")
    try:
        xml = ET.fromstring(data)
    except ET.ParseError as error:
        raise PipelineError("Malformed seed XLIFF") from error
    namespace = "{urn:oasis:names:tc:xliff:document:1.2}"
    require(xml.tag == namespace + "xliff" and xml.attrib == {"version": "1.2"}
            and len(xml) == 1, "Unexpected seed catalog structure")
    file = xml[0]
    require(file.tag == namespace + "file" and file.attrib ==
            {"source-language": "ja", "target-language": "en", "datatype": "plaintext"}
            and len(file) == 1 and file[0].tag == namespace + "body" and file[0].attrib == {},
            "Seed catalog language/structure mismatch")
    found = {}
    for unit in file[0]:
        require(unit.tag == namespace + "trans-unit" and set(unit.attrib) == {"id"} and len(unit) == 2,
                "Malformed seed catalog unit")
        identifier = unit.attrib["id"]
        require(identifier in targets and identifier not in found, "Unknown/duplicate seed catalog ID")
        source, target = unit
        require(source.tag == namespace + "source" and target.tag == namespace + "target"
                and not source.attrib and not target.attrib and not len(source) and not len(target)
                and source.text == targets[identifier]["source"] and target.text == targets[identifier]["translation"],
                "Seed catalog source/target mismatch")
        found[identifier] = target.text
    require(found.keys() == targets.keys(), "Incomplete seed catalog")
    return file_hash(path)


class Pipeline:
    def __init__(self, options):
        self.options = options
        self.cli, self.game, self.root = preflight(options)
        self.manifest = None
        self.env = child_environment(self.root, options)
        self.lock = None
        self.publication_failure = None
        self.owned_directories = None
        self.durable_file = None
        self.durable_state = None
        self.seed_input = None
        self.original = scan(self.game, source=True)
        self.binary = file_hash(self.cli)
        self.settings = asdict(options)
        self.settings.pop("resume")
        self.settings.update(cli=str(self.cli), game=str(self.game), output=str(self.root))
        self.work, self.pristine, self.install = (self.root / name for name in ("work-game", "pristine", "install-game"))
        self.en, self.es = self.root / "english.db", self.root / "spanish.db"
        self.archive = self.root / "spanish-patch.zip"

    def guard_publication_directories(self):
        require(self.owned_directories is not None, "Publication directory ownership unavailable")
        for path, identity in self.owned_directories.items():
            require(directory_identity(path) == identity, "Publication directory identity changed")

    def write_publication_failure(self, record):
        # One small exclusive side record, never ordinary save/replace, cleanup,
        # another filename, or a retry. A foreign root/record is not ours to edit.
        data = encoded(record) + b"\n"
        require(len(data) <= 4096, "Publication failure record too large")
        self.guard_publication_directories()
        path = plain(self.root / PUBLICATION_FAILURE, missing=True)
        with path.open("xb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())

    def save(self):
        if self.publication_failure is not None:
            raise self.publication_failure
        target, temporary = self.root / "pipeline-manifest.json", self.root / "pipeline-manifest.next"
        attempts, original_error, next_file = 0, None, None
        reason = "publication-prepare-failed"
        attempted_state = publication_state(self.manifest)
        try:
            if self.owned_directories is None:
                self.owned_directories = {path: directory_identity(path) for path in (self.root.parent, self.root)}
            self.guard_publication_directories()
            publication_file(target, self.durable_file)
            temporary = plain(temporary, missing=True)
            require(not temporary.exists(), "Interrupted manifest publication; " + RECOVERY)
            data = encoded(self.manifest) + b"\n"
            with temporary.open("xb") as stream:
                opened = os.fstat(stream.fileno())
                require(stat.S_ISREG(opened.st_mode) and opened.st_nlink == 1 and opened.st_ino != 0,
                        "Unsafe exclusive publication file")
                next_file = {"device": opened.st_dev, "inode": opened.st_ino,
                             "sha256": hashlib.sha256(data).hexdigest(), "size": len(data)}
                stream.write(data)
                stream.flush()
                os.fsync(stream.fileno())
            for attempt in range(len(PUBLICATION_DELAYS) + 1):
                if attempt:
                    time.sleep(PUBLICATION_DELAYS[attempt - 1])
                reason = "retry-guard-refused" if original_error is not None else "publication-guard-refused"
                # AFTER each delay, verify the same root/parent, exclusive next
                # inode/bytes, and old durable target (including prior absence).
                self.guard_publication_directories()
                publication_file(temporary, next_file)
                publication_file(target, self.durable_file)
                attempts += 1
                reason = "replacement-failed"
                try:
                    os.replace(temporary, target)
                except PermissionError as error:
                    reason = "permission-denied"
                    if original_error is None:
                        original_error = error
                    if attempt == len(PUBLICATION_DELAYS):
                        raise
                else:
                    self.durable_file, self.durable_state = next_file, attempted_state
                    return
        except (PipelineError, OSError, ValueError, TypeError, KeyError) as error:
            original_error = original_error if original_error is not None else error
            failure = ManifestPublicationError("Manifest publication failed (" + reason + "); retained state is ambiguous. " + RECOVERY)
            self.publication_failure = failure
            record = {"owner": OWNER, "version": VERSION, "run_id": self.manifest.get("run_id"),
                      "kind": "manifest-publication", "status": "failed", "resume_safe": False,
                      "reason": reason, "attempts": attempts, "error_type": type(original_error).__name__,
                      "errno": getattr(original_error, "errno", None), "winerror": getattr(original_error, "winerror", None),
                      "expected_next": next_file, "expected_durable": self.durable_file,
                      "attempted_state": attempted_state, "durable_state": self.durable_state}
            try:
                self.write_publication_failure(record)
            except (PipelineError, OSError, ValueError, TypeError, KeyError):
                # The original publication error wins even if recording it is
                # denied, occupied, interrupted, or unsafe. Retain partial data.
                pass
            raise failure from original_error

    def invariants(self, before_injection=False):
        require(scan(self.game, source=True) == self.original, "Original source changed")
        require(file_hash(self.cli) == self.binary, "CLI binary changed")
        require(scan(self.pristine) == self.original, "Pristine copy changed")
        if before_injection:
            require(scan(self.work) == self.original and scan(self.install) == self.original, "Pre-injection copy drift")
        if self.seed_input is not None:
            path, identity, owner_path, owner_identity, _ = self.seed_input
            publication_file(path, identity)
            publication_file(owner_path, owner_identity)
            for suffix in ("-wal", "-shm", "-journal"):
                require(not plain(str(path) + suffix, missing=True).exists(), "Seed sidecar appeared")
            for name in ("pipeline.lock", "pipeline-manifest.next", PUBLICATION_FAILURE):
                require(not plain(path.parent / name, missing=True).exists(), "Seed owner became active/uncertain")
        # Refuse any new linked/special member anywhere in the owned run.
        scan(self.root)

    def db_rows(self, path, root=None):
        rows, metadata = read_database(path)
        expected_root = root or self.work
        require(norm(metadata.get("game_root")) == norm(expected_root), "Database root provenance mismatch")
        inventory(rows, expected_root, self.options.format)
        return rows

    def snapshot(self, path):
        rows, metadata = read_database(path)
        physical = {}
        for suffix in ("", "-wal", "-journal"):
            item = Path(str(path) + suffix)
            if item.exists():
                physical[suffix] = file_hash(item)
        return {"rows": len(rows), "logical_sha256": digest([rows, metadata]), "files": physical}

    def checkpoints(self):
        return {path.name: self.snapshot(path) for path in (self.en, self.es) if path.exists()}

    def command(self, stage, args, structured=False, accepted_codes=(0,)):
        self.invariants(before_injection=not self.manifest["injection_started"])
        self.manifest["active_command"] = stage
        self.save()
        start = time.monotonic()
        try:
            result = subprocess.run([str(self.cli), "--config", str(self.root / "profile/config.json"), *map(str, args)],
                                    cwd=self.root, env=self.env if args[0] == "translate" and not stage.endswith("-fallback-google") else
                                    child_environment(self.root, self.options, subscription_login=False), stdin=subprocess.DEVNULL,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    timeout=self.options.timeout, check=False)
        except subprocess.TimeoutExpired as error:
            # subprocess.run kills its immediate process, NOT guaranteed agy or
            # other descendants. Preserve active marker; never retry this run.
            self.manifest["uncertain_child"] = True
            self.save()
            raise PipelineError("Child timeout; descendant cleanup is NOT guaranteed. " + RECOVERY) from error
        except OSError as error:
            raise PipelineError("CLI spawn failed; " + RECOVERY) from error
        self.manifest["active_command"] = None
        self.manifest["stages"].append({"stage": stage, "exit_code": result.returncode,
                                       "seconds": round(time.monotonic() - start, 3),
                                       "stdout_sha256": hashlib.sha256(result.stdout).hexdigest(),
                                       "stderr_sha256": hashlib.sha256(result.stderr).hexdigest()})
        self.save()
        require(result.returncode in accepted_codes, "Child command failed: " + stage)
        return strict_json(result.stdout) if structured else None

    def validate(self, path, stage):
        report = self.command(stage, ["validate", path, "--json"], structured=True, accepted_codes=(0, 1))
        gate_validation(report, self.db_rows(path))
        require(self.manifest["stages"][-1]["exit_code"] == (1 if report["count"] else 0), "Validation exit/report mismatch")
        self.manifest["stages"][-1]["classified_identical_resources"] = report["count"]
        self.save()

    def ready(self, path, baseline, stage):
        rows = self.db_rows(path)
        gate_rows(rows, baseline, self.work, self.options.format, ready=True)
        self.proven_rows(path, rows)
        self.validate(path, stage)
        return rows

    def proven_rows(self, path, rows):
        evidence = self.manifest["accepted_rows"][path.name]
        ready = {r["id"]: r for r in rows if r["status"] in READY}
        require(ready.keys() == evidence.keys(), "Unproven ready row inventory")
        for identifier, r in ready.items():
            proof = evidence[identifier]
            require(proof["row_sha256"] == digest(r) and proof["provider"] == r["provider_used"]
                    and proof["provider"] == stage_provider(self.settings, proof["stage"])
                    and any(s["stage"] == proof["stage"] and s["exit_code"] == 0 for s in self.manifest["stages"]),
                    "Unexpected translation provider/row attribution")
            if proof["provider"] == "import":
                target = self.manifest["seed"]["targets"].get(identifier)
                require(target is not None and target["source_sha256"] == digest(r["source"])
                        and target["target_sha256"] == digest(r["translation"]), "Unbound imported target")

    def accept_changes(self, path, before, stage, baseline, targets=None):
        after = self.db_rows(path)
        gate_rows(after, baseline, self.work, self.options.format)
        prior = {r["id"]: r for r in before}
        provider = stage_provider(self.settings, stage)
        accepted = dict(self.manifest["accepted_rows"][path.name])
        for r in after:
            old = prior[r["id"]]
            if old["status"] != "pending":
                require(r == old, "Translation changed previously accepted/nonpending row columns")
            elif r["status"] in READY:
                require(nonblank(r["translation"]) and nonblank(r["translated_at"])
                        and r["provider_used"] == provider, "Unproven stage translation attribution")
                # Static inventory excludes creation/review timestamps; they too
                # are immutable for pending-only translation/import.
                require(r["created_at"] == old["created_at"] and r["reviewed_at"] == old["reviewed_at"],
                        "Translation changed immutable timestamp columns")
                if targets is not None:
                    require(r["id"] in targets and r["translation"] == targets[r["id"]]["translation"],
                            "Unbound seed import")
                accepted[r["id"]] = {"row_sha256": digest(r), "stage": stage, "provider": provider}
            else:
                require(r == old, "Unaccepted pending row changed")
        if targets is not None:
            require(set(accepted) == set(targets), "Incomplete seed import")
        self.manifest["accepted_rows"][path.name] = accepted
        self.manifest["databases"] = self.checkpoints()
        self.save()
        self.proven_rows(path, after)
        return after

    def translate(self, path, source, target):
        baseline = self.manifest["japanese_inventory" if source == "ja" else "spanish_inventory"]
        language = "english" if source == "ja" else "spanish"
        before = self.db_rows(path)
        gate_rows(before, baseline, self.work, self.options.format)
        self.proven_rows(path, before)
        stage = source + "-" + target
        def dispatch(name):
            self.command(name, ["translate", path, "-p", stage_provider(self.settings, name),
                                "-s", source, "-t", target, "--batch-size", self.options.batch_size,
                                "--concurrency", self.options.concurrency, "--no-memory"])
        # Even an all-seeded project gets an orderly no-work primary result.
        dispatch(stage)
        after = self.accept_changes(path, before, stage, baseline)
        if self.options.fallback_provider and any(r["status"] == "pending" for r in after):
            # Validate already accepted controls before handing pending work to
            # the fallback. This is NOT a readiness checkpoint.
            self.validate(path, "validate-" + language + "-partial")
            fallback = stage + "-fallback-google"
            dispatch(fallback)
            self.accept_changes(path, after, fallback, baseline)

    def import_seed(self):
        self.seed_input = seed_owner(self.options.english_seed, self.original, self.options.format, self.root, self.game)
        path, identity, _, owner_identity, owner = self.seed_input
        snapshot = self.root / "seed-snapshot.db"
        publication_file(path, identity)
        with path.open("rb") as incoming, snapshot.open("xb") as outgoing:
            for chunk in iter(lambda: incoming.read(1024 * 1024), b""):
                outgoing.write(chunk)
            outgoing.flush()
            os.fsync(outgoing.fileno())
        self.invariants(before_injection=True)
        require(file_hash(snapshot) == {k: identity[k] for k in ("sha256", "size")}, "Seed snapshot hash mismatch")
        rows, metadata = read_database(snapshot)
        old_root = path.parent / "work-game"
        require(set(metadata) == {"game_root", "extraction_warnings"} and metadata["extraction_warnings"] == []
                and norm(metadata["game_root"]) == norm(old_root), "Seed root/metadata/pivot mismatch")
        require(remapped_inventory(rows, old_root, self.work, self.options.format) == self.manifest["japanese_inventory"]
                and inventory(rows, old_root, self.options.format) == owner["japanese_inventory"]
                and digest([rows, metadata]) == owner["databases"]["english.db"]["logical_sha256"]
                and len(rows) == owner["databases"]["english.db"]["rows"], "Seed complete Japanese inventory/snapshot drift")
        successful = {s["stage"] for s in owner["stages"] if s["exit_code"] == 0}
        providers = {stage_provider(owner["settings"], s) for s in successful
                     if s in {"ja-en", "ja-en-fallback-google"}}
        run_providers = seed_database_evidence(snapshot, providers)
        targets = {r["id"]: r for r in rows if r["status"] in READY}
        require(targets, "Seed has no accepted English targets")
        if owner["version"] == VERSION:
            proofs = owner["accepted_rows"]["english.db"]
            require(proofs.keys() == targets.keys()
                    and all(proofs[i]["row_sha256"] == digest(r) and proofs[i]["provider"] == r["provider_used"]
                            for i, r in targets.items()), "Seed accepted row binding mismatch")
        for r in targets.values():
            gate_rows([r], inventory([r], old_root, self.options.format), old_root, self.options.format, ready=True)
            require(r["provider_used"] in run_providers and "locust_injection_source" not in r["metadata"],
                    "Seed row language/provider/pivot evidence mismatch")
        catalog = self.root / "seed-english.xliff"
        self.command("seed-export-english", ["export", snapshot, "-f", "xliff", "-l", "en", "-o", catalog,
                                            "--status", "translated", "--status", "reviewed", "--status", "approved"])
        catalog_hash = verify_seed_catalog(catalog, targets)
        # Export may migrate only the owned copy. Compare all row columns after
        # export, not just catalog text.
        require(read_database(snapshot) == (rows, metadata), "Seed snapshot rows changed during export")
        before = self.db_rows(self.en)
        self.command("seed-import-english", ["import", self.en, "-f", "xliff", "-l", "en", "-i", catalog, "--keep-existing"])
        require(file_hash(catalog) == catalog_hash, "Seed catalog changed during import")
        # The fresh DB has only catalog targets and untouched pending rows;
        # invalid retained translations of pending seed rows are never imported.
        self.validate(self.en, "validate-seed-english")
        self.manifest["seed"] = {"run_id": owner["run_id"], "database": {k: identity[k] for k in ("sha256", "size")},
                                 "manifest_sha256": owner_identity["sha256"], "snapshot": file_hash(snapshot),
                                 "snapshot_logical_sha256": digest([rows, metadata]), "catalog": catalog_hash,
                                 "targets": {i: {"source_sha256": digest(r["source"]), "target_sha256": digest(r["translation"]),
                                                   "provider": r["provider_used"], "status": r["status"]} for i, r in targets.items()}}
        self.accept_changes(self.en, before, "seed-import-english", self.manifest["japanese_inventory"], targets)
        self.invariants(before_injection=True)

    def verify_seed_resume(self, candidate):
        evidence = candidate.get("seed")
        require((candidate["settings"]["english_seed"] is not None) == (evidence is not None),
                "Incomplete seed checkpoint cannot resume")
        if evidence is None:
            return
        snapshot, catalog = self.root / "seed-snapshot.db", self.root / "seed-english.xliff"
        require(file_hash(snapshot) == evidence["snapshot"] and file_hash(catalog) == evidence["catalog"],
                "Resume seed artifact hash drift")
        rows, metadata = read_database(snapshot)
        require(digest([rows, metadata]) == evidence["snapshot_logical_sha256"]
                and set(metadata) == {"game_root", "extraction_warnings"}, "Resume seed snapshot drift")
        # Historical seed path is only a root-normalization datum. Never open
        # the old input/manifest/config/executable or re-export/re-import it.
        old_root = Path(candidate["settings"]["english_seed"]).parent / "work-game"
        require(norm(metadata["game_root"]) == norm(old_root)
                and remapped_inventory(rows, old_root, self.work, self.options.format) == candidate["japanese_inventory"],
                "Resume seed full Japanese inventory drift")
        targets = {r["id"]: r for r in rows if r["status"] in READY}
        require(targets.keys() == evidence["targets"].keys(), "Resume seed target inventory drift")
        for i, r in targets.items():
            expected = evidence["targets"][i]
            require(expected == {"source_sha256": digest(r["source"]), "target_sha256": digest(r["translation"]),
                                 "provider": r["provider_used"], "status": r["status"]}, "Resume seed target provenance drift")
        require(verify_seed_catalog(catalog, targets) == evidence["catalog"], "Resume seed catalog drift")
        providers = seed_database_evidence(snapshot, {"antigravity", "google", PROVIDER, FALLBACK_TEST_PROVIDER})
        require({r["provider_used"] for r in targets.values()} <= providers, "Resume seed run attribution drift")

    def change_phase(self, phase):
        self.manifest["phase"] = phase
        self.manifest["databases"] = self.checkpoints()
        self.save()

    def initialize(self):
        if self.options.resume:
            require(self.root.is_dir(), "Resume root missing")
            scan(self.root)
            require(not (self.root / "pipeline-manifest.next").exists()
                    and not (self.root / PUBLICATION_FAILURE).exists(), "Interrupted manifest publication; " + RECOVERY)
            candidate = read_json_file(self.root / "pipeline-manifest.json")
            validate_resume_manifest(candidate)
            expected_settings = dict(self.settings, english_seed=candidate["settings"]["english_seed"])
            require(candidate["settings"] == expected_settings and candidate["binary"] == self.binary
                    and candidate["source"] == self.original
                    and candidate["tool_sha256"] == file_hash(Path(__file__).absolute())["sha256"], "Resume inputs/binary/settings/tool drift")
            config_path = plain(self.root / "profile/config.json")
            expected = permitted_config(self.options)
            expected_bytes = encoded(expected)
            # A coordinated config + manifest forgery can match every stored
            # hash. Independently reconstruct both semantics and exact bytes.
            require(config_path.stat().st_size == len(expected_bytes), "Resume config is not permitted")
            config_bytes = config_path.read_bytes()
            require(strict_json(config_bytes) == expected and config_bytes == expected_bytes,
                    "Resume config is not permitted")
            expected_identity = {"sha256": hashlib.sha256(expected_bytes).hexdigest(), "size": len(expected_bytes)}
            require(candidate["config"] == expected_identity == file_hash(config_path), "Resume config drift")
            self.invariants(before_injection=True)
            self.verify_seed_resume(candidate)
            self.settings = expected_settings
            require(candidate["databases"] == self.checkpoints(), "Resume database hashes/inventory drift")
            require(not self.es.exists() or candidate["phase"] in {"spanish-pending", "spanish-ready"}, "Ambiguous pivot; " + RECOVERY)
            # Invalid data is never assigned to the live manifest or republished.
            self.manifest = candidate
            self.owned_directories = {path: directory_identity(path) for path in (self.root.parent, self.root)}
            self.durable_file = publication_file(self.root / "pipeline-manifest.json")
            self.durable_state = publication_state(candidate)
        else:
            self.root.mkdir(mode=0o700)
            self.manifest = {"owner": OWNER, "version": VERSION, "run_id": str(uuid.uuid4()), "phase": "copying",
                             "settings": self.settings, "binary": self.binary, "source": self.original,
                             "tool_sha256": file_hash(Path(__file__).absolute())["sha256"],
                             "injection_started": False, "uncertain_child": False, "active_command": None,
                             "status": "running", "stages": [], "databases": {},
                             "accepted_rows": {"english.db": {}, "spanish.db": {}}}
            self.save()
        lock_path = plain(self.root / "pipeline.lock", missing=True)
        try:
            self.lock = lock_path.open("xb")
        except FileExistsError as error:
            raise PipelineError("Run is active/interrupted (exclusive lock remains); " + RECOVERY) from error
        if not self.options.resume:
            for name in ("profile", "backups", "temp", "profile/AppData/Local", "profile/AppData/Roaming"):
                (self.root / name).mkdir(mode=0o700, parents=True, exist_ok=True)
            with (self.root / "profile/config.json").open("xb") as stream:
                stream.write(encoded(permitted_config(self.options)))
            self.manifest["config"] = file_hash(self.root / "profile/config.json")
            copy_tree(self.game, self.pristine, self.original, readonly=True)
            copy_tree(self.game, self.work, self.original)
            copy_tree(self.game, self.install, self.original)
            self.command("cli-capability", ["inject", "--help"])
            self.command("extract-japanese", ["extract", self.work, "--format", self.options.format, "-o", self.en])
            rows = self.db_rows(self.en)
            require(all(value["status"] == "pending" and value["translation"] is None
                        and "locust_injection_source" not in value["metadata"] for value in rows), "Fresh extraction is not pristine pending")
            self.manifest["japanese_inventory"] = inventory(rows, self.work, self.options.format)
            self.change_phase("english-pending")

    def execute(self):
        self.initialize()
        phase = self.manifest["phase"]
        baseline = self.manifest.get("japanese_inventory")
        gate_rows(self.db_rows(self.en), baseline, self.work, self.options.format)
        if phase == "english-pending":
            if self.options.english_seed:
                self.import_seed()
            self.translate(self.en, "ja", "en")
            self.ready(self.en, baseline, "validate-english")
            self.change_phase("english-ready")
        english = self.ready(self.en, baseline, "validate-english-current")
        spanish_inventory = pivot_inventory(english, self.work, self.options.format)
        if self.manifest["phase"] == "english-ready":
            require(not any(Path(str(self.es) + suffix).exists() for suffix in ("", "-wal", "-shm", "-journal")), "Pivot destination occupied; " + RECOVERY)
            before = self.snapshot(self.en)
            self.command("strict-pivot", ["pivot", self.en, "-o", self.es, "--require-complete"])
            require(self.snapshot(self.en) == before, "Pivot changed English database")
            rows = self.db_rows(self.es)
            gate_rows(rows, spanish_inventory, self.work, self.options.format)
            require(all(value["translation"] is None and value["status"] == "pending" and value["provider_used"] is None for value in rows), "Pivot is not a fresh semantic project")
            self.manifest["spanish_inventory"] = spanish_inventory
            self.change_phase("spanish-pending")
        require(self.manifest.get("spanish_inventory") == spanish_inventory, "Pivot provenance drift")
        gate_rows(self.db_rows(self.es), spanish_inventory, self.work, self.options.format)
        if self.manifest["phase"] == "spanish-pending":
            self.translate(self.es, "en", "es")
            self.ready(self.es, spanish_inventory, "validate-spanish")
            self.change_phase("spanish-ready")
        spanish = self.ready(self.es, spanish_inventory, "validate-spanish-current")
        self.invariants(before_injection=True)
        # The durable boundary precedes the command, including spawn/timeout.
        self.manifest["injection_started"] = True
        self.manifest["phase"] = "injection-started"
        self.save()
        report = self.command("inject-spanish", ["inject", self.work, "-P", self.es, "--direct", "-l", "es", "--json"], structured=True)
        notice, evidence = verify_injection_evidence(report, self.work, self.root / "backups", self.original, self.options.format)
        reported = gate_injection(report, spanish, self.work, self.root / "backups", informational_notice=notice)
        self.manifest["injection"] = {"strings_written": report["strings_written"], "strings_skipped": report["strings_skipped"],
                                      "files_modified": report["files_modified"], "skip_reasons": report["reports"]["es"]["skip_reasons"],
                                      "outcome": "Recorded", "backup_id": report["backup_id"], **evidence}
        self.save()
        # Extract a separate resource-only work copy. Never reextract the pivot
        # DB or the recorded Direct tree (which deliberately refuses refresh).
        work_tree = scan(self.work)
        clean_work = {"files": {path: value for path, value in work_tree["files"].items() if path.split("/")[0] not in RESERVED},
                      "directories": [path for path in work_tree["directories"] if path.split("/")[0] not in RESERVED]}
        proof = self.root / "proof-game"
        # copy_tree's source guard intentionally rejects installed metadata; use
        # an explicit known resource inventory, checking full work around copy.
        self.copy_proof(proof, clean_work)
        self.command("reextract-work", ["extract", proof, "--format", self.options.format, "-o", self.root / "work-proof.db"])
        self.manifest["work_equivalence"] = compare_reextraction(spanish, self.work, self.db_rows(self.root / "work-proof.db", proof), proof, self.options.format)
        self.command("pack-spanish", ["patch", self.work, "-P", self.es, "-l", "es", "--pristine", self.pristine,
                                      "--no-detect-id", "-o", self.archive])
        manifest = verify_zip(self.archive, self.original, clean_work, self.options.format, reported)
        archive_hash = file_hash(self.archive)
        self.manifest["artifact"] = archive_hash
        self.save()
        self.command("strict-apply-dry-run", ["apply", self.install, self.archive, "--dry-run"])
        require(scan(self.install) == self.original, "Dry-run changed install game")
        self.command("strict-apply", ["apply", self.install, self.archive])
        self.manifest["first_install"] = verify_install(self.install, self.original, manifest, clean_work)
        self.command("reextract-install", ["extract", self.install, "--format", self.options.format, "-o", self.root / "install-proof.db"])
        self.manifest["install_equivalence"] = compare_reextraction(spanish, self.work, self.db_rows(self.root / "install-proof.db", self.install), self.install, self.options.format)
        self.command("rollback-proof", ["patch-rollback", self.install])
        require(scan(self.install) == self.original, "Rollback did not restore byte-exact pristine tree")
        self.manifest["rollback_verified"] = True
        self.save()
        require(file_hash(self.archive) == archive_hash, "Verified ZIP changed")
        # Keep a usable Spanish installation, NOT the rolled-back proof copy.
        self.command("final-reapply", ["apply", self.install, self.archive])
        self.manifest["final_install"] = verify_install(self.install, self.original, manifest, clean_work)
        self.command("reextract-final-install", ["extract", self.install, "--format", self.options.format, "-o", self.root / "final-install-proof.db"])
        self.manifest["final_equivalence"] = compare_reextraction(spanish, self.work, self.db_rows(self.root / "final-install-proof.db", self.install), self.install, self.options.format)
        require(file_hash(self.archive) == archive_hash, "Final ZIP drift")
        require(verify_install(self.install, self.original, manifest, clean_work) == self.manifest["final_install"], "Final install changed during reextraction")
        self.invariants()
        require(self.db_rows(self.en) == english and self.db_rows(self.es) == spanish, "Semantic database rows changed after injection")
        final_notice, final_evidence = verify_injection_evidence(report, self.work, self.root / "backups", self.original, self.options.format)
        require(final_notice == notice and final_evidence == evidence, "Injection recovery provenance changed")
        self.manifest["injection"]["informational_notice_verified"] = report["warnings"] == [notice]
        self.manifest["injection"]["complete_equivalence_verified"] = True
        self.manifest.update(status="completed", phase="completed", patch=str(self.archive), translated_copy=str(self.install),
                             semantic_evidence="Exact expected Spanish resource text, not linguistic quality",
                             runtime_evidence="Not tested; no game was launched")
        self.manifest["databases"] = self.checkpoints()
        self.save()
        return {"status": "completed", "output": str(self.root), "patch": str(self.archive),
                "translated_copy": str(self.install), "patch_sha256": archive_hash["sha256"],
                "resources_verified": len(spanish), "rollback_verified": True,
                "report": str(self.root / "pipeline-manifest.json"), "runtime_verified": False}

    def copy_proof(self, proof, inventory_value):
        proof.mkdir()
        for rel in inventory_value["directories"]:
            (proof / rel).mkdir(parents=True, exist_ok=True)
        for rel, identity in inventory_value["files"].items():
            source = plain(self.work / rel)
            require(file_hash(source) == identity, "Work proof drift")
            with source.open("rb") as incoming, (proof / rel).open("xb") as outgoing:
                for chunk in iter(lambda: incoming.read(1024 * 1024), b""):
                    outgoing.write(chunk)
        require(scan(proof) == inventory_value, "Work proof copy mismatch")

    def run(self):
        try:
            return self.execute()
        except (PipelineError, OSError, ValueError, TypeError, KeyError) as error:
            diagnostic = str(error) if isinstance(error, PipelineError) else "Malformed artifact or owned filesystem operation failed"
            if self.manifest is not None and self.lock is not None:
                self.manifest.update(status="failed", diagnostic=diagnostic)
                if self.publication_failure is None:
                    try:
                        if not self.manifest.get("injection_started") and not self.manifest.get("active_command"):
                            try:
                                self.manifest["databases"] = self.checkpoints()
                            except PipelineError:
                                self.manifest["phase"] = "unverified-failure"
                        self.save()
                    except (PipelineError, OSError, ValueError, TypeError, KeyError):
                        # A secondary publication/reporting failure must not
                        # mask the original gate/child/IO failure. save() records
                        # publication failures independently, when safe to do so.
                        pass
            if isinstance(error, PipelineError):
                raise
            raise PipelineError(diagnostic) from error
        finally:
            if self.lock is not None:
                unwinding = sys.exc_info()[0] is not None
                try:
                    self.lock.close()
                    # Publication ambiguity retains this owned marker too. Do
                    # not touch a possibly foreign root while unwinding failure.
                    if self.publication_failure is None:
                        plain(self.root / "pipeline.lock").unlink()
                except (PipelineError, OSError):
                    if not unwinding:
                        raise


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", required=True)
    parser.add_argument("--game", required=True)
    parser.add_argument("--output", required=True, help="Fresh, absent directory (or an owned pre-injection run with --resume)")
    parser.add_argument("--format", required=True, choices=("renpy", "rpgmaker-mv", "kirikiri"))
    parser.add_argument("--provider", required=True, choices=("google", "antigravity"))
    parser.add_argument("--fallback-provider", choices=("google",), help="Explicit pending-only Google fallback after orderly Antigravity success")
    parser.add_argument("--english-seed", help="Fresh-run-only closed owned English DB; copy/export/import verified ready targets")
    parser.add_argument("--test-fallback-endpoint", help="Test-only separate literal-loopback fallback route; requires scripted primary and explicit Google fallback")
    parser.add_argument("--test-endpoint", help="Dummy-key OpenAI-compatible literal-loopback HTTP endpoint only")
    parser.add_argument("--batch-size", type=int, default=40)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--timeout", type=float, default=900, help="Per-child wall timeout in seconds (descendant cleanup not guaranteed)")
    parser.add_argument("--resume", action="store_true")
    try:
        summary = Pipeline(Options(**vars(parser.parse_args(argv)))).run()
        print(json.dumps(summary, sort_keys=True))
        return 0
    except (PipelineError, OSError, ValueError, TypeError, KeyError) as error:
        diagnostic = str(error) if isinstance(error, PipelineError) else "Malformed input or filesystem preflight failed"
        print(json.dumps({"status": "failed", "diagnostic": diagnostic}, sort_keys=True))
        return 1


if __name__ == "__main__":
    sys.exit(main())
