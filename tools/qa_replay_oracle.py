"""Resource-aware oracle for the small saved five-engine QA fixtures.

HTML byte offsets and RPG dialogue command offsets may move when text grows.
For those resources, compare their ordered structural slots within the same
file. All other fixture resources retain their extraction identity. This is an
oracle for these fixtures, not a universal engine identity migration algorithm.
"""
from collections import defaultdict
import re


def normalized(value):
    value = str(value).replace("\\", "/").rstrip("/")
    return value[4:] if value.startswith("//?/") else value


def relative(value, root):
    value, root = normalized(value), normalized(root)
    if not value.lower().startswith(root.lower() + "/"):
        raise AssertionError(f"resource outside root: {value}")
    result = value[len(root) + 1:]
    if any(part in ("", ".", "..") for part in result.split("/")):
        raise AssertionError(f"unsafe resource path: {result}")
    return result


def slots(engine, entries, root):
    result, ordered = {}, defaultdict(list)
    for entry in entries:
        path = relative(entry["file_path"], root)
        identifier = normalized(entry["id"])
        prefix = normalized(root)
        if identifier.lower().startswith(prefix.lower() + "/"):
            identifier = identifier[len(prefix) + 1:]
        if engine == "html-game":
            metadata = entry["metadata"]
            ordered[(path, "html")].append((metadata["html_start"], metadata["html_kind"], entry))
            continue
        command = re.fullmatch(r"(.*)#cmd_(\d+)#(.*)", identifier) if engine == "rpgmaker-mv" else None
        if command:
            ordered[(path, command[1])].append((int(command[2]), command[3], entry))
            continue
        key = (path, identifier)
        if key in result:
            raise AssertionError(f"duplicate resource identity: {key}")
        result[key] = entry
    for group, rows in ordered.items():
        # One RPG choice command contains several separately identified choices.
        positions = [(row[0], row[1]) for row in rows]
        if len(set(positions)) != len(positions):
            raise AssertionError(f"duplicate structural slot: {group}")
        for ordinal, (_, kind, entry) in enumerate(sorted(rows, key=lambda row: (row[0], row[1]))):
            key = (*group, ordinal, kind)
            if key in result:
                raise AssertionError(f"duplicate normalized resource: {key}")
            result[key] = entry
    return result


def assert_reextracted(engine, expected, expected_root, extracted, extracted_root):
    wanted = slots(engine, expected, expected_root)
    actual = slots(engine, extracted, extracted_root)
    if wanted.keys() != actual.keys():
        raise AssertionError(f"resource identities differ: missing={sorted(wanted.keys()-actual.keys())}, unexpected={sorted(actual.keys()-wanted.keys())}")
    normalized_dialogues = 0
    for key, entry in wanted.items():
        translated, source = entry["translation"], actual[key]["source"]
        if source == translated:
            continue
        # MV's message writer can wrap one dialogue into adjacent 401 commands;
        # re-extraction rejoins those lines. Keep this exception engine-scoped.
        if engine == "rpgmaker-mv" and source.replace("\n", " ") == translated:
            normalized_dialogues += 1
            continue
        raise AssertionError(f"translation differs at resource {key}: {source!r} != {translated!r}")
    return {"resources_verified": len(wanted), "normalized_rpg_dialogues": normalized_dialogues}
