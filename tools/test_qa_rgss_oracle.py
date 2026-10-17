"""Focused wire tests; never read Git history or run a build/provider."""
import contextlib
import io
from pathlib import Path
import unittest
import tempfile
import json
import hashlib
import os
import shutil
import subprocess
import xml.etree.ElementTree as ET

from ja_en_es_pipeline import read_database

import qa_rgss_oracle as q


class WireTests(unittest.TestCase):
    def test_literal_encoding_and_shared_string(self):
        wire = b'\x04\x08[\x07I"\x08abc\x06:\x06ET@\x06'
        self.assertEqual(q.decode(wire), ("array", (
            ("string", b"abc", True), ("link", (0,)))))

    def test_reject_unsupported_and_malformed(self):
        bad = [b"", b"\x04\x09T", b"\x04\x08T0", b"\x04\x08u",
               b"\x04\x08@\x00", b"\x04\x08;\x00", b"\x04\x08[\x06@\x00",
               b"\x04\x08o;\x00\x00",  # missing class symbol reference
               b"\x04\x08o:\x06X\x06:\x06X0",  # duplicate symbol definition
               b"\x04\x08o:\x06X\x07:\x07@a0;\x060",  # duplicate ivar
               b"\x04\x08[\x07[\x00@\x06",  # backward container link
               b'\x04\x08I"\x06a\x00',  # missing E metadata
               b'\x04\x08I"\x06a\x06:\x06Ei\x00',  # nonboolean E

               b'\x04\x08I"\x06a\x06:\x06EFX',
               b'\x04\x08I"\x06\xff\x06:\x06ET',
               b'\x04\x08I"\x06\xff\x06:\x06EF',
               b'\x04\x08I"\x06a\x06:\x0dencoding"\x0aUTF-8',
               b'\x04\x08"\x02\xff\xff', b"\x04\x08[\xfa",
               b"\x04\x08" + b"[\x06" * 34 + b"0",
               b"\x04\x08" + b"0" * 65536]
        q.BASE.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=q.BASE, prefix="unit-") as scratch:
            root = Path(scratch) / "game"
            q.create(root)
            for wire in bad:
                with self.subTest(wire=wire[:24]), self.assertRaises(q.Refused) as caught:
                    q.decode(wire)
                (root / q.FILES[0]).write_bytes(wire)
                before = {name: (root / name).read_bytes() for name in q.FILES}
                reason = "fixture file/size refused" if len(wire) > 65536 else str(caught.exception)
                self.assertEqual(self.invoke(["check", str(root), "--phase", "ja"]),
                                 (1, "", "RGSS refused: " + reason + "\n"))
                self.assertEqual(before, {name: (root / name).read_bytes() for name in q.FILES})

    def test_every_fixture_prefix_is_truncated(self):
        for name, wire in q.fixture("ja").items():
            if name.endswith("rvdata2"):
                q.decode(wire)
                for end in range(len(wire)):
                    with self.subTest(name=name, end=end), self.assertRaises(q.Refused):
                        q.decode(wire[:end])

    def test_fixed_predictions_and_non_display(self):
        ja, en, es = (q.fixture(p) for p in q.TEXT)
        for name in q.FILES[2:]:
            self.assertEqual(ja[name], en[name])
            self.assertEqual(ja[name], es[name])
        for phase in ("en", "es"):
            lines = q.TEXT[phase][2]
            self.assertEqual(len(lines), 3)
            self.assertTrue(all(len(line) <= 40 for line in lines))
            for left, right in zip(lines, lines[1:]):
                self.assertGreater(len(left) + 1 + len(right.split()[0]), 40)
        graph = q.decode(ja[q.FILES[0]])
        self.assertEqual(graph[1][1][2][3], ("@alias", ("link", (1, "@opaque"))))
        self.assertEqual(graph[1][1][2][4][1], ("string", b"ASCII sentinel", False))

    def invoke(self, args):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = q.main(args)
        return code, out.getvalue(), err.getvalue()

    def test_public_fixture_check_and_rejection_effects(self):
        q.BASE.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=q.BASE, prefix="unit-") as scratch:
            root = Path(scratch) / "game"
            self.assertEqual(self.invoke(["fixture", str(root)]), (0, "RGSS fixture OK\n", ""))
            original = {name: (root / name).read_bytes() for name in q.FILES}
            self.assertEqual(self.invoke(["check", str(root), "--phase", "ja"]), (0, "RGSS check OK\n", ""))
            self.assertEqual(self.invoke(["fixture", str(root)]), (1, "", "RGSS refused: existing fixture refused\n"))
            self.assertEqual(self.invoke(["check", str(root), "--phase", "es"]),
                             (1, "", "RGSS refused: artifact graph differs: Data/Actors.rvdata2\n"))
            self.assertEqual(original, {name: (root / name).read_bytes() for name in q.FILES})
            actor = root / q.FILES[0]
            for bad in (b"\x04\x08u", original[q.FILES[0]][:-1],
                        original[q.FILES[0]].replace(b"physical", b"PHYSICAL"),
                        original[q.FILES[0]].replace(b"@\x09", b"@\x07"),
                        original[q.FILES[0]].replace(b"ASCII sentinel", b"ASCII SENTINEL"),
                        original[q.FILES[0]][:-1] + b"T"):
                self.assertNotEqual(bad, original[q.FILES[0]])
                actor.write_bytes(bad)
                before = {name: (root / name).read_bytes() for name in q.FILES}
                code, out, err = self.invoke(["check", str(root), "--phase", "ja"])
                self.assertEqual((code, out), (1, ""))
                try:
                    q.decode(bad)
                    reason = "artifact graph differs: Data/Actors.rvdata2"
                except q.Refused as error:
                    reason = str(error)
                self.assertEqual(err, "RGSS refused: " + reason + "\n")
                self.assertEqual(before, {name: (root / name).read_bytes() for name in q.FILES})

    def test_graph_equivalent_wire_changes_are_not_silently_accepted(self):
        q.BASE.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=q.BASE, prefix="unit-") as scratch:
            root = Path(scratch) / "game"
            q.create(root)
            actor = root / q.FILES[0]
            wire = actor.read_bytes().replace(b"[\x07", b"[\x01\x02", 1)
            self.assertEqual(q.decode(wire), q.decode(actor.read_bytes()))
            actor.write_bytes(wire)
            self.assertEqual(self.invoke(["check", str(root), "--phase", "ja"]),
                             (1, "", "RGSS refused: artifact wire differs: Data/Actors.rvdata2\n"))
            self.assertEqual(actor.read_bytes(), wire)

    def test_help_and_outside_refusal(self):
        for args in (["--help"], ["fixture", "--help"], ["check", "--help"]):
            with contextlib.redirect_stdout(io.StringIO()) as out, self.assertRaises(SystemExit) as exit:
                q.main(args)
            self.assertEqual(exit.exception.code, 0)
            self.assertIn("usage:", out.getvalue())
        self.assertEqual(self.invoke(["fixture", "tools"]), (1, "", "RGSS refused: owned path refused\n"))
        for args in (["check", str(q.BASE), "--phase", "xx"], ["check", str(q.BASE)], ["unknown"]):
            with contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()) as err:
                with self.assertRaises(SystemExit) as exit:
                    q.main(args)
            self.assertEqual(exit.exception.code, 2)
            self.assertEqual(out.getvalue(), "")
            self.assertIn("error:", err.getvalue())


class PublicCliTests(unittest.TestCase):
    def test_owned_public_lifecycle(self):
        cli = q.BASE.parents[2] / "target/debug/locust.exe"
        if not cli.is_file():
            self.skipTest("existing checkout target/debug/locust.exe unavailable; no build allowed")
        q.BASE.mkdir(parents=True, exist_ok=True)
        root = Path(tempfile.mkdtemp(dir=q.BASE, prefix="public-"))
        print("Retained public CLI evidence:", root)
        source, install = root / "source", root / "install"
        q.create(source)
        shutil.copytree(source, install)
        baseline = {name: (source / name).read_bytes() for name in q.FILES}
        config = root / "config.json"
        config.write_text("{}", encoding="utf-8")
        env = {k: v for k, v in os.environ.items() if k.upper() in {
            "PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "SYSTEMDRIVE"}}
        for key in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
                    "XDG_DATA_HOME", "XDG_CACHE_HOME", "LOCUST_DATA_DIR", "LOCUST_BACKUP_ROOT", "TMP", "TEMP", "TMPDIR"):
            relative = {"APPDATA": "profile/AppData/Roaming", "LOCALAPPDATA": "profile/AppData/Local",
                        "HOME": "profile", "USERPROFILE": "profile"}.get(key, key)
            env[key] = str(root / relative)
            Path(env[key]).mkdir(parents=True, exist_ok=True)
        records = []
        def run(*args, accepted=0):
            command = [str(cli), "--config", str(config), *map(str, args)]
            result = subprocess.run(command, env=env, cwd=root, capture_output=True, timeout=30)
            record = {"command": command, "exit": result.returncode,
                      "stdout": result.stdout.decode("utf-8", errors="replace"),
                      "stderr": result.stderr.decode("utf-8", errors="replace")}
            records.append(record)
            (root / "commands.json").write_text(json.dumps(records, ensure_ascii=False, indent=2), encoding="utf-8")
            self.assertEqual(result.returncode, accepted, record)
            return record["stdout"]
        def targets(phase):
            name, desc, lines, later, yes, no = q.TEXT[phase]
            return dict(zip(["Actors.rvdata2#1#@name", "Actors.rvdata2#1#@description",
                "Map001.rvdata2#0#event_1#page_0#cmd_1#msg", "Map001.rvdata2#0#event_1#page_0#cmd_3#msg",
                "Map001.rvdata2#0#event_1#page_0#cmd_4#choice_0", "Map001.rvdata2#0#event_1#page_0#cmd_4#choice_1"],
                [name, desc, " ".join(lines), later, yes, no]))
        def rows(db):
            result, _ = read_database(db)
            return {row["id"]: row for row in result}
        def seed(db, phase):
            catalog = root / (phase + ".xlf")
            run("export", db, "-f", "xliff", "-l", phase, "-o", catalog)
            tree = ET.parse(catalog)
            units = [node for node in tree.iter() if node.tag.split("}")[-1] == "trans-unit"]
            self.assertEqual({node.attrib["id"] for node in units}, set(targets(phase)))
            for unit in units:
                old = next((node for node in unit if node.tag.split("}")[-1] == "target"), None)
                if old is not None:
                    unit.remove(old)
                prefix = unit.tag[:-len("trans-unit")]
                ET.SubElement(unit, prefix + "target").text = targets(phase)[unit.attrib["id"]]
            tree.write(catalog, encoding="utf-8", xml_declaration=True)
            run("import", db, "-f", "xliff", "-l", phase, "-i", catalog)
            run("validate", db, "--json")
            self.assertEqual({key: row["translation"] for key, row in rows(db).items()}, targets(phase))
        english, spanish = root / "en.db", root / "es.db"
        run("extract", source, "-f", "rpgmaker-vxa", "-o", english)
        original_rows = rows(english)
        self.assertEqual({key: row["source"] for key, row in original_rows.items()}, targets("ja"))
        rejected = root / "rejected-pivot.db"
        before = english.read_bytes()
        run("pivot", english, "-o", rejected, "--require-complete", accepted=1)
        self.assertFalse(rejected.exists())
        self.assertEqual(before, english.read_bytes())
        self.assertEqual(original_rows, rows(english))
        self.assertEqual(records[-1]["stdout"], "")
        self.assertIn("incomplete", records[-1]["stderr"].lower())
        seed(english, "en")
        en_copy = root / "english-copy"
        english_report = json.loads(run("inject", source, "-P", english, "-l", "en", "-m", "replace", "-o", en_copy, "--json"))
        en_copy = q.owned(english_report["injected_roots"]["en"])
        q.check(en_copy, "en")
        run("pivot", english, "-o", spanish, "--require-complete")
        pivot_rows = rows(spanish)
        self.assertEqual({key: row["source"] for key, row in pivot_rows.items()}, targets("en"))
        self.assertEqual({key: row["metadata"]["locust_injection_source"] for key, row in pivot_rows.items()}, targets("ja"))
        seed(spanish, "es")
        es_copy = root / "spanish-copy"
        report = json.loads(run("inject", source, "-P", spanish, "-l", "es", "-m", "replace", "-o", es_copy, "--json"))
        es_copy = q.owned(report["injected_roots"]["es"])
        self.assertEqual(report["reports"]["es"]["strings_written"], 6)
        self.assertEqual(report["reports"]["es"]["strings_skipped"], 0)
        q.check(es_copy, "es")
        # Re-extraction is corroboration only; graph proof above is independent.
        observed = root / "reextract.db"
        run("extract", es_copy, "-f", "rpgmaker-vxa", "-o", observed)
        physical = rows(observed)
        self.assertIn("Map001.rvdata2#0#event_1#page_0#cmd_5#msg", physical)
        self.assertIn("Map001.rvdata2#0#event_1#page_0#cmd_6#choice_0", physical)
        patch = root / "patch.zip"
        run("patch", es_copy, "-P", spanish, "-l", "es", "-o", patch, "--pristine", source, "--no-detect-id")
        run("apply", install, patch)
        q.check(install, "es")
        run("patch-rollback", install)
        q.check(install, "ja")
        run("apply", install, patch)
        q.check(install, "es")
        self.assertEqual(baseline, {name: (source / name).read_bytes() for name in q.FILES})
        wire_equal = {}
        for phase, actual_root in (("en", en_copy), ("es", es_copy)):
            expected_root = root / ("expected-" + phase)
            for name, wire in q.fixture(phase).items():
                path = expected_root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(wire)
                wire_equal[phase + "/" + name] = wire == (actual_root / name).read_bytes()
        with cli.open("rb") as stream:
            cli_sha256 = hashlib.file_digest(stream, "sha256").hexdigest()
        (root / "proof.json").write_text(json.dumps({"result": "bounded graph/lifecycle pass", "selected": 6,
            "cli_sha256": cli_sha256, "wire_equal": wire_equal,
            "contract": "exact independent full document bytes plus bounded ordered graph",
            "original_source_unchanged": True,
            "original": original_rows, "pivot": pivot_rows, "reextracted": physical,
            "injection": report}, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    unittest.main()
