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


# Pure, bounded mirror of crates/formats/src/rpgmaker_mv.rs:
# rewrap_to_source_width (1658), message_paragraphs/preserve_blank_line_layout,
# source_line_markers/split_before_markers, wrap_message, split_name_tag (1800),
# visible_len (1616), and apply_message_block (1353). Replace first rewraps
# every changed entry (2148); messages then wrap again against physical lines.
# No CLI/database access, and no normalization of the observed output.
_MARKERS = "・■□●○◆◇★☆※▶▷•"
# Rust char::is_whitespace uses Unicode White_Space, unlike Python's extra C0s.
_WS = " \t\n\r\v\f\u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000"
_WORDS = re.compile("[^" + _WS + "]+")
_NAME = re.compile(r"^(\\[A-Za-z]+<[^>]*>)(.*)$", re.DOTALL)


def _name_body(text):
    match = _NAME.match(text)
    return (match[1], match[2].lstrip(_WS)) if match else ("", text)


def _visible_len(text):
    # Deliberately matches Rust's scanner even for intermediate word fragments
    # of a name box. Input syntax is independently checked before replay.
    position, visible = 0, 0
    while position < len(text):
        if text[position] != "\\":
            visible += 1
            position += 1
            continue
        position += 1
        start = position
        while position < len(text) and text[position].isascii() and text[position].isalpha():
            position += 1
        letters = position > start
        if position < len(text) and (text[position] == "[" or letters and text[position] == "<"):
            end = "]" if text[position] == "[" else ">"
            stop = text.find(end, position)
            position = len(text) if stop < 0 else stop + 1
        elif not letters and position < len(text) and text[position] in "{}$.|!><^\\":
            position += 1
    return visible


def _wrap(text, width):
    lines, current = [], ""
    for word in _WORDS.findall(text):
        if not current:
            current = word
        elif _visible_len(current) + 1 + _visible_len(word) <= width:
            current += " " + word
        else:
            lines.append(current)
            current = word
    return lines + [current] if current or not lines else lines


def _rust_lines(text):
    if not text:
        return []
    lines = text.split("\n")
    return [line.removesuffix("\r") for line in lines[:-1]] + ([lines[-1]] if lines[-1] else [])


def _paragraphs(text):
    paragraphs, blanks, current = [], [0], []
    for line in text.split("\n"):
        if not line.strip(_WS):
            if current:
                paragraphs.append("\n".join(current))
                current = []
                blanks.append(0)
            blanks[-1] += 1
        else:
            current.append(line)
    if current:
        paragraphs.append("\n".join(current))
        blanks.append(0)
    return paragraphs, blanks


def _blank_layout(source, translation, wrap):
    source_paragraphs, blanks = _paragraphs(source)
    paragraphs, _ = _paragraphs(translation)
    if not any(blanks) or not paragraphs or len(paragraphs) != len(source_paragraphs):
        return None
    lines = [""] * blanks[0]
    for paragraph, count in zip(paragraphs, blanks[1:]):
        lines.extend(wrap(paragraph))
        lines.extend([""] * count)
    return lines


def _rewrap(source, translation):
    if "\n" not in source:
        return translation
    width = max(map(_visible_len, _rust_lines(source)), default=0)
    if not width:
        return translation
    fits = lambda text: all(_visible_len(line) <= width for line in _rust_lines(text))
    lines = _blank_layout(source, translation,
                          lambda text: text.split("\n") if fits(text) else _wrap(text, width))
    if lines is not None:
        return "\n".join(lines)
    return translation if fits(translation) else "\n".join(_wrap(" ".join(_WORDS.findall(translation)), width))


def _message_layout(source, translation):
    source_lines = source.split("\n")
    width = max(40, *map(_visible_len, source_lines))
    markers = {body.lstrip(_WS)[0] for line in source_lines
               for _, body in [_name_body(line)] if body.lstrip(_WS) and body.lstrip(_WS)[0] in _MARKERS}

    def wrap(text):
        tag, body = _name_body(" ".join(_WORDS.findall(text)))
        starts = [0] + [i for i, char in enumerate(body) if i > 0 and char in markers]
        items = [body[a:b].strip(_WS) for a, b in zip(starts, starts[1:] + [len(body)])]
        lines = [line for i, item in enumerate(items) if item or i == len(items) - 1
                 for line in _wrap(item, width)]
        lines[0] = tag + lines[0]
        return lines

    lines = _blank_layout(source, translation, wrap)
    return "\n".join(wrap(translation) if lines is None else lines)


def _controls(text):
    """Independent ordered inventory; unknown/nested/malformed escapes refuse.

    Only standard numeric RPG controls, symbol controls and YEP name boxes are
    classified. Name text is translatable; replay still compares it exactly.
    """
    if not isinstance(text, str) or any(ord(c) < 32 and c not in "\t\r\n" for c in text):
        raise AssertionError("unsupported RPG text")
    tokens, position = [], 0
    # Numeric codes are case-insensitive in RPG Maker; their literal spelling
    # remains protected. A literal escaped '#' takes Rust's visible fallback
    # (the slash is skipped, '#' counts as one), NOT the zero-width symbol path.
    pattern = re.compile(r"\\(?:[CNVPI]\[[0-9]+\]|(?:FS|OC|OW|PX|PY)\[[0-9]+\]|G(?![A-Za-z\[])|[{}$.|!><^\\#])", re.IGNORECASE)
    while position < len(text):
        if text[position] != "\\":
            if text[position] in "<>":
                raise AssertionError("unsupported RPG tag")
            position += 1
            continue
        name = re.match(r"\\(n|nc|nr)<([^<>\r\n]+)>", text[position:], re.IGNORECASE)
        token = pattern.match(text, position) if not name else None
        if name:
            if not name[2].strip(_WS):
                raise AssertionError("blank RPG name box")
            tokens.append("\\" + name[1] + "<>")
            tokens.extend(_controls(name[2]))  # embedded color/actor codes stay protected
            position += name.end()
        elif token:
            if token[0] != r"\#":  # writer-visible literal, not an RPG control
                tokens.append(token[0])
            position = token.end()
        else:
            raise AssertionError("unknown or malformed RPG control")
    return tokens


def expected_layout(engine, entry, actual):
    """Return one exact writer result, never an equivalence class of layouts.

    Identity/inventory checks belong to the caller and run first. Unsupported
    engines/kinds stay literal-only, even if their source happens to be multiline.
    """
    literal = entry["translation"]
    if engine != "rpgmaker-mv":
        return literal
    identifier = entry["id"]
    message = re.fullmatch(r".+#cmd_[0-9]+#msg", identifier) is not None
    field = re.fullmatch(r".+#[0-9]+#(?:description|profile)", identifier) is not None
    if not (message or field):
        return literal
    metadata = entry.get("metadata", {})
    if metadata.get("format_id", engine) != engine:
        raise AssertionError("wrong RPG format")
    tags = entry.get("tags")
    if tags is not None and tags not in ([["dialogue"], ["scroll_text"]] if message else [["description"]]):
        raise AssertionError("unsupported RPG slot tags")
    physical = metadata.get("locust_injection_source", entry["source"])
    # Independent controls are checked BEFORE the layout branch, including when
    # actual == literal. Exact replay below additionally pins positions/names.
    if _controls(physical) != _controls(literal) or _controls(literal) != _controls(actual):
        raise AssertionError("RPG control inventory differs")
    if literal == physical:  # production's unchanged skip keeps original bytes
        return literal
    wrapped = _rewrap(physical, literal)
    return _message_layout(physical, wrapped) if message else wrapped


def assert_reextracted(engine, expected, expected_root, extracted, extracted_root):
    wanted = slots(engine, expected, expected_root)
    actual = slots(engine, extracted, extracted_root)
    if wanted.keys() != actual.keys():
        raise AssertionError(f"resource identities differ: missing={sorted(wanted.keys()-actual.keys())}, unexpected={sorted(actual.keys()-wanted.keys())}")
    normalized_dialogues = 0
    for key, entry in wanted.items():
        translated, source = entry["translation"], actual[key]["source"]
        if entry.get("tags") != actual[key].get("tags"):
            raise AssertionError("reextraction slot tags differ")
        written = expected_layout(engine, entry, source)
        if source != written:
            raise AssertionError(f"translation differs at resource {key}: {source!r} != {written!r}")
        normalized_dialogues += source != translated
    return {"resources_verified": len(wanted), "normalized_rpg_dialogues": normalized_dialogues}
