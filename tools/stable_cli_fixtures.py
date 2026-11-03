"""Small, independently predicted Stable-format games; no production serializers.

The RGSS builders use the bounded wire vocabulary from qa_rgss_oracle, never
its repository scratch directory or CLI. These are transport fixtures, not
engine-launch or linguistic certification.
"""
from dataclasses import dataclass
import json
from pathlib import Path

from qa_rgss_oracle import Wire, decode, packed


# Short sources exercise Mock's same-slot reverse and its exact-fit tag.
# Targets are explicit predictions, not copied out of the translated database.
TEXT = {
    "name": ("案内人", "人内案"),
    "description": ("静かな人です。", "。すで人なか静"),
    "hello": ("こんにちは。", "。はちにんこ"),
    "later": ("また会いましょう。", "[MOCK:en]"),
    "yes": ("進む", "む進"),
    "no": ("待つ", "つ待"),
    "title": ("小さな町", "町なさ小"),
}
SENTINEL = b"\x00\xffSTABLE-CLI-RESOURCE\x80\x00"


@dataclass(frozen=True)
class Case:
    name: str
    format_id: str
    extension: str = ""


CASES = (
    Case("mv", "rpgmaker-mv"),
    Case("mz", "rpgmaker-mv"),
    Case("xp", "rpgmaker-vxa", "rxdata"),
    Case("vx", "rpgmaker-vxa", "rvdata"),
    Case("vx-ace", "rpgmaker-vxa", "rvdata2"),
    Case("renpy", "renpy"),
)


def texts(translated):
    return {key: pair[int(translated)] for key, pair in TEXT.items()}


def expected_rows(case, translated=False):
    t = texts(translated)
    if case.extension:
        ext = case.extension
        prefix = f"Map001.{ext}#0#event_1#page_0"
        first, later, choice = (0, 1, 2) if case.name == "xp" else (1, 3, 4)
        return {
            f"Actors.{ext}#1#@name": t["name"],
            f"Actors.{ext}#1#@description": t["description"],
            f"{prefix}#cmd_{first}#msg": t["hello"],
            f"{prefix}#cmd_{later}#msg": t["later"],
            f"{prefix}#cmd_{choice}#choice_0": t["yes"],
            f"{prefix}#cmd_{choice}#choice_1": t["no"],
        }
    if case.name == "renpy":
        return {f"script.rpy#{line}": t[key] for line, key in (
            (1, "name"), (4, "hello"), (5, "later"), (7, "yes"), (9, "no"))}
    prefix = "Map001.json#0#event_1#page_0"
    return {
        "Actors.json#1#name": t["name"],
        "Actors.json#1#profile": t["description"],
        "System.json#gameTitle": t["title"],
        f"{prefix}#cmd_1#msg": t["hello"],
        f"{prefix}#cmd_3#msg": t["later"],
        f"{prefix}#cmd_4#choice_0": t["yes"],
        f"{prefix}#cmd_4#choice_1": t["no"],
    }


def rgss_files(case, t):
    w = Wire()
    actor = w.array([lambda: b"0", lambda: w.obj("RPG::Actor", [
        ("@name", lambda: w.text(t["name"])),
        ("@description", lambda: w.text(t["description"])),
        ("@opaque", lambda: w.text(b"\x00\xff\x80physical", None)),
        ("@ascii", lambda: w.text("ASCII sentinel", False)),
    ])])
    w = Wire()

    def commands():
        def header():
            return w.command(101, [lambda: w.text("", False),
                                  lambda: b"i\x00", lambda: b"i\x00",
                                  lambda: b"i" + packed(2)])
        builders = []
        for key in ("hello", "later"):
            if case.name == "xp":
                builders.append(lambda key=key: w.command(101, [lambda: w.text(t[key])]))
            else:
                builders.extend([header, lambda key=key: w.command(401, [lambda: w.text(t[key])])])
        builders.extend([
            lambda: w.command(102, [lambda: w.array([
                lambda: w.text(t["yes"]), lambda: w.text(t["no"])]), lambda: b"i\x00"]),
            lambda: w.command(0, []),
        ])
        return w.array(builders)

    def events():
        w.register()
        return b"{\x06i\x06" + w.obj("RPG::Event", [
            ("@pages", lambda: w.array([lambda: w.obj("RPG::Event::Page", [
                ("@list", commands), ("@opaque", lambda: w.text("PAGE_SENTINEL", False)),
            ])])),
        ])

    map_data = w.obj("RPG::Map", [
        ("@events", events), ("@opaque", lambda: w.text(b"\x00\xffmap", None)),
    ])
    ext = case.extension
    return {
        f"Data/Actors.{ext}": b"\x04\x08" + actor,
        f"Data/Map001.{ext}": b"\x04\x08" + map_data,
        f"Data/System.{ext}": b'\x04\x08[\x07"\x0bSYSTEMi\x16',
        "Graphics/sentinel.bin": SENTINEL,
    }


def files(case, translated=False):
    t = texts(translated)
    if case.extension:
        return rgss_files(case, t)
    if case.name == "renpy":
        script = (
            f'define e = Character("{t["name"]}")\n'
            'label start:\n'
            '    $ asset = "NON_TEXT_SENTINEL.png"\n'
            f'    e "{t["hello"]}"\n'
            f'    e "{t["later"]}"\n'
            '    menu:\n'
            f'        "{t["yes"]}":\n'
            '            return\n'
            f'        "{t["no"]}":\n'
            '            return\n'
        )
        return {"game/script.rpy": script.encode("utf-8"), "game/sentinel.bin": SENTINEL}

    def command(code, parameters):
        return {"code": code, "indent": 0, "parameters": parameters}

    header = ["", 0, 0, 2] + ([""] if case.name == "mz" else [])
    listing = [command(101, header), command(401, [t["hello"]]),
               command(101, header), command(401, [t["later"]]),
               command(102, [[t["yes"], t["no"]], 1, 0, 2, 0]), command(0, [])]
    documents = {
        "data/Actors.json": [None, {"id": 1, "name": t["name"], "nickname": "",
                                   "profile": t["description"], "note": "NON_TEXT_SENTINEL"}],
        "data/System.json": {"gameTitle": t["title"], "terms": {"basic": [], "commands": []},
                             "switches": [None, "NON_TEXT_SENTINEL"]},
        "data/Map001.json": {"displayName": "", "note": "NON_TEXT_SENTINEL",
                              "width": 1, "height": 1, "data": [17, 23, 42],
                              "events": [None, {"id": 1, "name": "NON_TEXT_SENTINEL", "pages": [{
                                  "conditions": {"switch1Id": 7, "switch1Valid": True},
                                  "image": {"characterName": "NON_TEXT_SENTINEL.png", "direction": 2},
                                  "list": listing}]}]},
    }
    result = {name: (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
              for name, value in documents.items()}
    core = "rmmz_core.js" if case.name == "mz" else "rpg_core.js"
    result["js/" + core] = b"// Synthetic engine marker; not a game runtime.\n"
    result["img/sentinel.bin"] = SENTINEL
    return result


def create(case, root):
    root = Path(root)
    root.mkdir()  # Never replace an existing game.
    for name, data in files(case).items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


def check_payload(case, root, translated=True):
    """Check full JSON/Marshal graphs or script bytes, including nontext fields.

    JSON whitespace and Marshal encoding choices may change on injection; the
    ordered Marshal graph is checked independently. Rollback has a separate,
    stricter complete path/byte/hash comparison in the lifecycle runner.
    """
    for name, expected in files(case, translated).items():
        actual = (Path(root) / name).read_bytes()
        if name.endswith(".json"):
            equal = json.loads(actual) == json.loads(expected)
        elif case.extension and name.endswith("." + case.extension):
            equal = decode(actual) == decode(expected)
        else:
            equal = actual == expected
        if not equal:
            raise AssertionError("unexpected translated payload or nontext sentinel: " + name)
