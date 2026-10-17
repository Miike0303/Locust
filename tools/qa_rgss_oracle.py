"""Independent, bounded Ace fixture-wire oracle. Not a Marshal runtime/serializer."""
import argparse
from pathlib import Path
import sys


class Refused(ValueError):
    pass


def need(ok, message):
    if not ok:
        raise Refused(message)


class Reader:
    """Exact ordered graph; link targets are stable structural paths, not wire IDs."""
    def __init__(self, data):
        need(len(data) <= 65536 and data[:2] == b"\x04\x08", "header/size refused")
        self.data, self.pos = data, 2
        self.symbols, self.objects = [], []
        self.nodes = 0

    def take(self, count):
        need(0 <= count <= len(self.data) - self.pos, "truncated input")
        start = self.pos
        self.pos += count
        return self.data[start:self.pos]

    def number(self):
        n = int.from_bytes(self.take(1), "little", signed=True)
        if n == 0:
            return 0
        if n > 4:
            return n - 5
        if n < -4:
            return n + 5
        raw = self.take(abs(n))
        return int.from_bytes(raw, "little") - (1 << (8 * -n) if n < 0 else 0)

    def count(self):
        n = self.number()
        need(0 <= n <= 4096, "length refused")
        return n

    def blob(self):
        return self.take(self.count())

    def symbol(self):
        tag = self.take(1)
        if tag == b";":
            index = self.number()
            need(0 <= index < len(self.symbols), "symbol link refused")
            return self.symbols[index]
        need(tag == b":", "symbol tag refused")
        raw = self.blob()
        need(0 < len(raw) <= 64 and raw.isascii(), "symbol encoding refused")
        name = raw.decode("ascii")
        need(name not in self.symbols, "duplicate symbol definition refused")
        self.symbols.append(name)
        return name

    def value(self, path=(), depth=0):
        self.nodes += 1
        need(depth <= 32 and self.nodes <= 2048, "graph limit refused")
        tag = self.take(1)
        if tag in (b"0", b"T", b"F"):
            return ({b"0": "nil", b"T": "true", b"F": "false"}[tag],)
        if tag == b"i":
            return ("int", self.number())
        if tag == b"@":
            index = self.number()
            need(0 <= index < len(self.objects), "object link refused")
            target, kind = self.objects[index]
            need(kind == "string", "container/cyclic link refused")
            return ("link", target)
        if tag == b"I":
            need(self.take(1) == b'"', "encoding wrapper refused")
            self.objects.append((path, "string"))
            raw = self.blob()
            need(self.count() == 1 and self.symbol() == "E", "encoding metadata refused")
            flag = self.take(1)
            need(flag in (b"T", b"F"), "encoding flag refused")
            try:
                raw.decode("utf-8" if flag == b"T" else "ascii")
            except UnicodeError as error:
                raise Refused("string encoding refused") from error
            return ("string", raw, flag == b"T")
        if tag == b'"':
            self.objects.append((path, "string"))
            return ("string", self.blob(), None)
        need(tag in (b"[", b"{", b"o"), "unsupported tag")
        self.objects.append((path, "container"))
        child = lambda p: self.value(path + (p,), depth + 1)
        if tag == b"[":
            return ("array", tuple(child(i) for i in range(self.count())))
        if tag == b"{":
            return ("hash", tuple((child((i, "key")), child((i, "value")))
                                  for i in range(self.count())))
        name = self.symbol()
        fields = []
        for _ in range(self.count()):
            key = self.symbol()
            need(key not in [k for k, _ in fields], "duplicate ivar refused")
            fields.append((key, child(key)))
        return ("object", name, tuple(fields))


def decode(data):
    reader = Reader(data)
    result = reader.value()
    need(reader.pos == len(data), "trailing input refused")
    return result


# Tiny fixture construction vocabulary, independent of all production modules.
# No arbitrary graph serialization: only these explicit shapes and backward string links.
def packed(n):
    need(0 <= n <= 65535, "fixture integer refused")
    return bytes([0 if n == 0 else n + 5]) if n < 123 else bytes([1 if n < 256 else 2]) + n.to_bytes(1 if n < 256 else 2, "little")


class Wire:
    def __init__(self):
        self.symbols = []
        self.objects = 0

    def register(self):
        self.objects += 1

    def sym(self, name):
        if name in self.symbols:
            return b";" + packed(self.symbols.index(name))
        self.symbols.append(name)
        raw = name.encode("ascii")
        return b":" + packed(len(raw)) + raw

    def text(self, text, encoding=True):
        self.register()
        raw = text if isinstance(text, bytes) else text.encode("utf-8")
        body = b'"' + packed(len(raw)) + raw
        return body if encoding is None else b"I" + body + b"\x06" + self.sym("E") + (b"T" if encoding else b"F")

    def array(self, builders):
        self.register()
        return b"[" + packed(len(builders)) + b"".join(f() for f in builders)

    def obj(self, name, fields):
        self.register()
        return b"o" + self.sym(name) + packed(len(fields)) + b"".join(self.sym(k) + f() for k, f in fields)

    def command(self, code, parameters):
        return self.obj("RPG::EventCommand", [("@code", lambda: b"i" + packed(code)),
            ("@indent", lambda: b"i\x00"), ("@parameters", lambda: self.array(parameters))])


TEXT = {
    "ja": ("案内人", "静かな案内人です。仲間と旅をしています。\n町の人々を助けながら道を探しています。", ["こんにちは。"], "また会いましょう。", "進む", "待つ"),
    "en": ("Guide", "Calm guide.\nKind ally.", ["The guide offers a map to every visitor", "and explains the route through the town", "before the quiet journey begins."], "See you later.", "Continue", "Wait"),
    "es": ("Guía", "Guía serena.\nBuena aliada.", ["La guía ofrece un mapa a cada visitante", "y explica la ruta por el pueblo antes de", "comenzar un tranquilo viaje."], "Hasta luego.", "Seguir", "Esperar"),
}
FILES = ("Data/Actors.rvdata2", "Data/Map001.rvdata2", "Data/System.rvdata2", "Graphics/sentinel.bin")
BASE = Path(__file__).resolve().parents[1] / "tmp/engine-pipeline-expansion/rgss"


def fixture(phase):
    name, description, lines, later, yes, no = TEXT[phase]
    w = Wire()
    # IDs: array=0, actor=1, name=2, description=3, opaque=4.
    actor = w.array([lambda: b"0", lambda: w.obj("RPG::Actor", [
        ("@name", lambda: w.text(name)), ("@description", lambda: w.text(description)),
        ("@opaque", lambda: w.text(b"\x00\xff\x80physical", None)),
        ("@alias", lambda: b"@\x09"), ("@ascii", lambda: w.text("ASCII sentinel", False))])])
    w = Wire()
    def commands():
        builders = [lambda: w.command(101, [lambda: w.text("", False), lambda: b"i\x00", lambda: b"i\x00", lambda: b"i\x07"])]
        builders += [lambda line=line: w.command(401, [lambda: w.text(line)]) for line in lines]
        builders += [lambda: w.command(101, [lambda: w.text("", False), lambda: b"i\x00", lambda: b"i\x00", lambda: b"i\x07"]),
                     lambda: w.command(401, [lambda: w.text(later)]),
                     lambda: w.command(102, [lambda: w.array([lambda: w.text(yes), lambda: w.text(no)]), lambda: b"i\x00"]),
                     lambda: w.command(0, [])]
        return w.array(builders)
    def events():
        w.register()
        return b"{\x06i\x06" + w.obj("RPG::Event", [("@pages", lambda: w.array([
            lambda: w.obj("RPG::Event::Page", [("@list", commands)])]))])
    map_data = w.obj("RPG::Map", [("@events", events), ("@opaque", lambda: w.text(b"\x00\xffmap", None))])
    return dict(zip(FILES, [b"\x04\x08" + actor, b"\x04\x08" + map_data,
                           b'\x04\x08[\x07"\x0bSYSTEMi\x16', b"\x00\xffRESOURCE\x80\x00"]))


def owned(path):
    path = Path(path).absolute()
    need(".." not in path.parts and path.is_relative_to(BASE) and path != BASE, "owned path refused")
    for part in (*reversed(path.parents), path):
        if part.exists() or part.is_symlink():
            info = part.lstat()
            need(not part.is_symlink() and not getattr(info, "st_file_attributes", 0) & 0x400, "linked path refused")
    return path


def create(root):
    root = owned(root)
    need(not root.exists(), "existing fixture refused")
    for name, data in fixture("ja").items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


def check(root, phase):
    root = owned(root)
    for name, expected in fixture(phase).items():
        path = owned(root / name)
        need(path.is_file() and path.stat().st_size <= 65536, "fixture file/size refused")
        actual = path.read_bytes()
        if name in FILES[:2]:
            need(decode(actual) == decode(expected), "artifact graph differs: " + name)
            need(actual == expected, "artifact wire differs: " + name)
        else:
            need(actual == expected, "unchanged resource differs: " + name)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("fixture", help="Create a new owned Japanese fixture").add_argument("root")
    check_parser = sub.add_parser("check", help="Read-only exact bounded graph and wire/resource proof")
    check_parser.add_argument("root")
    check_parser.add_argument("--phase", choices=TEXT, required=True, help="Fixed expected physical language")
    args = parser.parse_args(argv)
    try:
        if args.command == "fixture":
            create(args.root)
        else:
            check(args.root, args.phase)
        print("RGSS " + args.command + " OK")
        return 0
    except (Refused, OSError) as error:
        print("RGSS refused: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
