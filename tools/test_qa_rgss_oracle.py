"""Focused wire tests; never read Git history or run a build/provider."""
import argparse
import contextlib
import io
from pathlib import Path
import unittest
from unittest import mock
import tempfile
import json
import hashlib
import os
import shutil
import subprocess
import sys
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

    def test_shrink_profile_wire_and_readonly_rejections(self):
        profile = "ace-common-shrink"
        documents = {phase: q.fixture(phase, profile) for phase in q.TEXT}
        common = "Data/CommonEvents.rvdata2"
        self.assertEqual(set(documents["ja"]), {common, *q.FILES[2:]})
        for phase, files in documents.items():
            event = q.decode(files[common])[1][1]
            fields = dict(event[2])
            commands = fields["@list"][1]
            self.assertEqual(len(commands), 7 if phase == "ja" else 5)
            self.assertEqual(fields["@opaque"], ("string", b"\x00\xff\x80physical", None))
            self.assertEqual(fields["@ascii"], ("string", b"ASCII sentinel", False))
            for name in q.FILES[2:]:
                self.assertEqual(files[name], documents["ja"][name])
        q.BASE.mkdir(parents=True, exist_ok=True)
        root = Path(tempfile.mkdtemp(dir=q.BASE, prefix="f4-shrink-unit-")) / "game"
        args = ["--profile", profile]
        self.assertEqual(self.invoke(["fixture", str(root), *args]), (0, "RGSS fixture OK\n", ""))
        self.assertEqual(self.invoke(["check", str(root), "--phase", "ja", *args]), (0, "RGSS check OK\n", ""))
        path = root / common
        original = path.read_bytes()
        variants = [
            (original.replace(b"physical", b"PHYSICAL"), "graph"),
            (original.replace(b"i\x02\x91\x01", b"i\x02\x92\x01", 1), "graph"),
            (original.replace(b"@indenti\x00", b"@indenti\x06", 1), "graph"),
            (original[:-1] + b"T", "graph"),
            (original.replace(b"[\x07", b"[\x01\x02", 1), "wire"),
        ]
        for wire, kind in variants:
            with self.subTest(kind=kind, wire=wire[:32]):
                self.assertNotEqual(wire, original)
                path.write_bytes(wire)
                before = {name: (root / name).read_bytes() for name in documents["ja"]}
                self.assertEqual(self.invoke(["check", str(root), "--phase", "ja", *args]),
                    (1, "", f"RGSS refused: artifact {kind} differs: {common}\n"))
                self.assertEqual(before, {name: (root / name).read_bytes() for name in before})
        path.write_bytes(original)

    def test_shrink_profile_paths_and_invalid_profile_do_not_create(self):
        q.BASE.mkdir(parents=True, exist_ok=True)
        parent = Path(tempfile.mkdtemp(dir=q.BASE, prefix="f4-paths-"))
        root = parent / "game"
        args = ["--profile", "ace-common-shrink"]
        self.assertEqual(self.invoke(["fixture", str(parent), *args]),
                         (1, "", "RGSS refused: existing fixture refused\n"))
        self.assertEqual(self.invoke(["fixture", "tools", *args]),
                         (1, "", "RGSS refused: owned path refused\n"))
        self.assertEqual(list(parent.iterdir()), [])
        for command in (["fixture", str(root)], ["check", str(root), "--phase", "ja"]):
            with mock.patch.object(sys, "argv", ["qa_rgss_oracle.py"]), \
                    contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()) as err:
                with self.assertRaises(SystemExit) as caught:
                    q.main([*command, "--profile", "bad"])
            self.assertEqual(caught.exception.code, 2)
            self.assertEqual(out.getvalue(), "")
            usage = "[-h] root" if command[0] == "fixture" else "[-h] --phase {ja,en,es} root"
            # argparse changed choice quoting between supported Python versions.
            # Compare exact streams against an independent, fixed-choice parser.
            reference = argparse.ArgumentParser(prog=f"qa_rgss_oracle.py {command[0]}",
                                                usage=f"%(prog)s {usage}")
            reference.add_argument("--profile", choices=["ace-v1", "ace-common-shrink"])
            expected_error = io.StringIO()
            with contextlib.redirect_stderr(expected_error), self.assertRaises(SystemExit) as expected_exit:
                reference.parse_args(["--profile", "bad"])
            self.assertEqual(expected_exit.exception.code, 2)
            self.assertEqual(err.getvalue(), expected_error.getvalue())
            self.assertEqual(list(parent.iterdir()), [])
        # The test never falls back to a copy when symlinks are unavailable.
        linked = parent / "link"
        try:
            linked.symlink_to(parent, target_is_directory=True)
        except OSError as error:
            self.fail("linked-root proof unavailable: " + str(error))
        self.assertEqual(self.invoke(["fixture", str(linked / "game"), *args]),
                         (1, "", "RGSS refused: linked path refused\n"))
        self.assertEqual(self.invoke(["check", str(linked), "--phase", "ja", *args]),
                         (1, "", "RGSS refused: linked path refused\n"))
        self.assertEqual(set(p.name for p in parent.iterdir()), {"link"})

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


def checkout_cli():
    explicit = os.environ.get("LOCUST_QA_RGSS_CLI")
    if explicit is not None:
        cli = Path(explicit).absolute()
        if not explicit or not cli.is_file():
            raise AssertionError("explicit QA CLI unavailable: " + explicit)
        return cli
    name = "locust.exe" if sys.platform == "win32" else "locust"
    cli = Path(__file__).resolve().parents[1] / "target/debug" / name
    if not cli.is_file():
        if os.environ.get("CI"):
            raise AssertionError("required CI CLI unavailable: " + str(cli))
        raise unittest.SkipTest("existing checkout " + str(cli) + " unavailable; no build allowed")
    return cli


class CliSelectionTests(unittest.TestCase):
    def test_explicit_candidate_selected_and_missing_is_failure_not_skip(self):
        candidate = str(Path(__file__).resolve())
        with mock.patch.dict(os.environ, {"LOCUST_QA_RGSS_CLI": candidate}, clear=True), \
                mock.patch.object(Path, "is_file", return_value=True):
            self.assertEqual(checkout_cli(), Path(candidate))
        for explicit in (candidate + ".missing", ""):
            for ci in ("", "true"):
                with self.subTest(explicit=explicit, ci=ci), \
                        mock.patch.dict(os.environ, {"LOCUST_QA_RGSS_CLI": explicit, "CI": ci}, clear=True):
                    with self.assertRaisesRegex(AssertionError, "explicit QA CLI unavailable"):
                        checkout_cli()

    @mock.patch.dict(os.environ, {}, clear=True)
    def test_platform_binary_in_normal_checkout_target(self):
        root = Path(__file__).resolve().parents[1]
        for platform, name in (("win32", "locust.exe"), ("linux", "locust"), ("darwin", "locust")):
            with self.subTest(platform=platform), mock.patch("sys.platform", platform), \
                    mock.patch.object(Path, "is_file", return_value=True):
                self.assertEqual(checkout_cli(), root / "target/debug" / name)

    @mock.patch.dict(os.environ, {}, clear=True)
    def test_ci_missing_binary_fails_local_missing_explicitly_skips(self):
        with mock.patch.object(Path, "is_file", return_value=False):
            with mock.patch.dict(os.environ, {"CI": "true"}):
                with self.assertRaisesRegex(AssertionError, "required CI CLI unavailable"):
                    checkout_cli()
            with mock.patch.dict(os.environ, {}, clear=True):
                with self.assertRaisesRegex(unittest.SkipTest, "existing checkout.*unavailable"):
                    checkout_cli()


class PublicCliTests(unittest.TestCase):
    def test_shrink_public_lifecycle(self):
        self.lifecycle("ace-common-shrink")

    def test_owned_public_lifecycle(self):
        self.lifecycle()

    def lifecycle(self, profile="ace-v1"):
        cli = checkout_cli()
        q.BASE.mkdir(parents=True, exist_ok=True)
        root = Path(tempfile.mkdtemp(dir=q.BASE, prefix="public-"))
        print("Retained public CLI evidence:", root)
        source, install = root / "source", root / "install"
        q.create(source, profile)
        shutil.copytree(source, install)
        files = q.fixture("ja", profile)
        baseline = {path.relative_to(source).as_posix(): path.read_bytes()
                    for path in source.rglob("*") if path.is_file()}
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
            raw = {}
            for stream in ("stdout", "stderr"):
                path = root / f"cli-{len(records):02d}.{stream}"
                path.write_bytes(getattr(result, stream))
                raw[stream + "_path"] = str(path)
            record = {"command": command, "exit": result.returncode, **raw,
                      "stdout": result.stdout.decode("utf-8"),
                      "stderr": result.stderr.decode("utf-8")}
            records.append(record)
            (root / "commands.json").write_text(json.dumps(records, ensure_ascii=False, indent=2), encoding="utf-8")
            self.assertEqual(result.returncode, accepted, record)
            if accepted == 0:
                self.assertEqual(record["stderr"], "", record)
            return record["stdout"]
        def targets(phase):
            if profile == "ace-common-shrink":
                lines, later = q.SHRINK_TEXT[phase]
                return dict(zip(["CommonEvents.rvdata2#1#cmd_1#msg",
                                 "CommonEvents.rvdata2#1#cmd_5#msg"], ["\n".join(lines), later]))
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
            self.assertEqual({unit.attrib["id"]: next(node.text for node in unit
                if node.tag.split("}")[-1] == "source") for unit in units},
                targets("ja" if phase == "en" else "en"))
            for unit in units:
                old = next((node for node in unit if node.tag.split("}")[-1] == "target"), None)
                if old is not None:
                    unit.remove(old)
                prefix = unit.tag[:-len("trans-unit")]
                ET.SubElement(unit, prefix + "target").text = targets(phase)[unit.attrib["id"]]
            tree.write(catalog, encoding="utf-8", xml_declaration=True)
            run("import", db, "-f", "xliff", "-l", phase, "-i", catalog)
            self.assertEqual(run("validate", db, "--json"), '{"count":0,"issues":[]}\n')
            self.assertEqual({key: row["translation"] for key, row in rows(db).items()}, targets(phase))
        english, spanish = root / "en.db", root / "es.db"
        run("extract", source, "-f", "rpgmaker-vxa", "-o", english)
        original_rows = rows(english)
        self.assertEqual({key: row["source"] for key, row in original_rows.items()}, targets("ja"))
        rejected = root / "rejected-pivot.db"
        before = english.read_bytes()
        run("pivot", english, "-o", rejected, "--require-complete", accepted=1)
        self.assertEqual(list(root.glob(rejected.name + "*")), [])
        # A read-only SQLite connection can retire transient SHM/WAL files.
        # Exact persistent DB bytes include counters; do not equate that housekeeping with a write.
        self.assertEqual(before, english.read_bytes())
        self.assertEqual(original_rows, rows(english))
        self.assertEqual(records[-1]["stdout"], "")
        self.assertEqual(records[-1]["stderr"], "Error: incomplete pivot: entry '" +
                         sorted(targets("ja"))[0] + "' has a missing or blank translation\n")
        seed(english, "en")
        en_copy = root / "english-copy"
        english_report = json.loads(run("inject", source, "-P", english, "-l", "en", "-m", "replace", "-o", en_copy, "--json"))
        en_copy = q.owned(english_report["injected_roots"]["en"])
        q.check(en_copy, "en", profile)
        self.assertEqual(english_report["reports"]["en"]["strings_written"], len(targets("en")))
        self.assertEqual(english_report["reports"]["en"]["strings_skipped"], 0)
        run("pivot", english, "-o", spanish, "--require-complete")
        pivot_rows = rows(spanish)
        self.assertEqual({key: row["source"] for key, row in pivot_rows.items()}, targets("en"))
        self.assertEqual({key: row["metadata"]["locust_injection_source"] for key, row in pivot_rows.items()}, targets("ja"))
        seed(spanish, "es")
        es_copy = root / "spanish-copy"
        report = json.loads(run("inject", source, "-P", spanish, "-l", "es", "-m", "replace", "-o", es_copy, "--json"))
        es_copy = q.owned(report["injected_roots"]["es"])
        self.assertEqual(report["reports"]["es"]["strings_written"], len(targets("es")))
        self.assertEqual(report["reports"]["es"]["strings_skipped"], 0)
        q.check(es_copy, "es", profile)
        # Re-extraction is corroboration only; graph proof above is independent.
        observed = root / "reextract.db"
        run("extract", es_copy, "-f", "rpgmaker-vxa", "-o", observed)
        physical = rows(observed)
        if profile == "ace-common-shrink":
            expected_physical = {key.replace("cmd_5", "cmd_3"): text for key, text in targets("es").items()}
            self.assertEqual({key: row["source"] for key, row in physical.items()}, expected_physical)
        else:
            self.assertIn("Map001.rvdata2#0#event_1#page_0#cmd_5#msg", physical)
            self.assertIn("Map001.rvdata2#0#event_1#page_0#cmd_6#choice_0", physical)
        patch = root / "patch.zip"
        run("patch", es_copy, "-P", spanish, "-l", "es", "-o", patch, "--pristine", source, "--no-detect-id")
        run("apply", install, patch)
        q.check(install, "es", profile)
        run("patch-rollback", install)
        q.check(install, "ja", profile)
        run("apply", install, patch)
        q.check(install, "es", profile)
        self.assertEqual(baseline, {path.relative_to(source).as_posix(): path.read_bytes()
                                   for path in source.rglob("*") if path.is_file()})
        wire_equal = {}
        for phase, actual_root in (("en", en_copy), ("es", es_copy)):
            expected_root = root / ("expected-" + phase)
            for name, wire in q.fixture(phase, profile).items():
                path = expected_root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(wire)
                wire_equal[phase + "/" + name] = wire == (actual_root / name).read_bytes()
        with cli.open("rb") as stream:
            cli_sha256 = hashlib.file_digest(stream, "sha256").hexdigest()
        (root / "proof.json").write_text(json.dumps({"result": "bounded graph/lifecycle pass", "profile": profile, "selected": len(targets("ja")),
            "cli_sha256": cli_sha256, "wire_equal": wire_equal,
            "contract": "exact independent full document bytes plus bounded ordered graph",
            "original_source_unchanged": True,
            "original": original_rows, "pivot": pivot_rows, "reextracted": physical,
            "injection": report, "english_injection": english_report,
            "rejection_db_bytes_counters_unchanged": True}, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    unittest.main()
