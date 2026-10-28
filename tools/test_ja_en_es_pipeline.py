"""Deterministic safety and release gates; production transport tests below."""
import copy
from dataclasses import replace
import errno
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from io import StringIO
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import patch

SCRATCH = Path(__file__).resolve().parents[1] / ".test-tmp"
CLI = Path(__file__).resolve().parents[1] / "target/debug" / ("locust.exe" if os.name == "nt" else "locust")

import ja_en_es_pipeline as pipeline
from test_qa_replay_oracle import LAYOUT_CASES, layout_rejections, layout_row


def row(root, **updates):
    value = {"id": "script.rpy#1", "file_path": str(root / "game/script.rpy"),
             "source": "こんにちは", "translation": "Hello", "status": "translated",
             "context": None, "tags": [], "metadata": {"format_id": "renpy"},
             "char_limit": None, "provider_used": "pipeline-scripted",
             "created_at": "2026-01-01", "translated_at": "2026-01-01", "reviewed_at": None}
    value.update(updates)
    return value


class PipelineCase(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="pipeline-test-", dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.game = self.root / "source"
        (self.game / "game").mkdir(parents=True)
        (self.game / "game/script.rpy").write_text('label start:\n    "こんにちは"\n', encoding="utf-8")
        self.cli = self.root / "locust.exe"
        self.cli.write_bytes(b"mock executable")
        self.cli.chmod(0o700)

    def options(self, **updates):
        values = dict(cli=str(self.cli), game=str(self.game), output=str(self.root / "run"),
                      format="renpy", provider="google")
        values.update(updates)
        return pipeline.Options(**values)


class GlossaryRules(PipelineCase):
    def glossary(self, value=None):
        path = self.root / "terms.json"
        path.write_text(json.dumps(value if value is not None else {
            "version": 1, "ja-en": [{"term": "こんにちは", "translation": "Hello"}],
            "en-es": [{"term": "Hello", "translation": "Hola"}]}), encoding="utf-8")
        return path

    def test_closed_public_input_rejects_before_output_or_child(self):
        valid = {"version": 1, "ja-en": [], "en-es": []}
        invalid = ["[]", '{"version":1,"version":1,"ja-en":[],"en-es":[]}',
                   json.dumps(dict(valid, unknown=True)), json.dumps(dict(valid, version=True)),
                   json.dumps(dict(valid, **{"ja-en": {}}))]
        for entry in ({"term": " ", "translation": "Hello"},
                      {"term": "Hi", "translation": ""},
                      {"term": 1, "translation": "Hello"},
                      {"term": "Hi", "translation": "Hola", "case_sensitive": True},
                      {"term": "Hi", "translation": "Bad\\\\C[2]"},
                      {"term": " Hi", "translation": "Hello"}):
            invalid.append(json.dumps(dict(valid, **{"ja-en": [entry]})))
        invalid.append(json.dumps(dict(valid, **{"en-es": [
            {"term": "Hello", "translation": "Hola"}, {"term": "HELLO", "translation": "Otro"}]})))
        path = self.glossary(valid)
        for text in invalid:
            with self.subTest(text=text):
                path.write_text(text, encoding="utf-8")
                before = pipeline.scan(self.game)
                stdout, stderr = StringIO(), StringIO()
                args = ["--cli", str(self.cli), "--game", str(self.game), "--output", str(self.root / "run"),
                        "--format", "renpy", "--provider", "google", "--glossary-json", str(path)]
                with patch("sys.stdout", stdout), patch("sys.stderr", stderr), patch.object(pipeline.subprocess, "run") as child:
                    self.assertEqual(pipeline.main(args), 1)
                self.assertEqual(stderr.getvalue(), "")
                result = json.loads(stdout.getvalue())
                self.assertEqual(set(result), {"status", "diagnostic"})
                self.assertEqual(result["status"], "failed")
                self.assertIn(result["diagnostic"], {
                    "Malformed/unknown resume manifest fields", "Duplicate JSON key", "Unsupported glossary version",
                    "Glossary phase must be an array of at most 200 terms", "Duplicate/ambiguous glossary term",
                    "Glossary requires bounded plain visible text without controls or edge whitespace"})
                self.assertEqual(stdout.getvalue(), json.dumps(result, sort_keys=True) + "\n")
                self.assertEqual(path.read_text(encoding="utf-8"), text)
                self.assertEqual(pipeline.scan(self.game), before)
                self.assertFalse((self.root / "run").exists())
                child.assert_not_called()

    def test_canonical_policy_content_not_filename_or_array_order(self):
        path = self.glossary({"version": 1, "ja-en": [
            {"term": "進む", "translation": "Proceed"}, {"term": "こんにちは", "translation": "Hello"}], "en-es": []})
        first = pipeline.Pipeline(self.options(glossary_json=str(path)))
        value = json.loads(path.read_text(encoding="utf-8"))
        value["ja-en"].reverse()
        alias = self.root / "same-terms.json"
        alias.write_text(json.dumps(value, indent=2), encoding="utf-8")
        second = pipeline.Pipeline(self.options(glossary_json=str(alias)))
        self.assertEqual(first.settings, second.settings)
        self.assertIn("glossary", first.settings)
        self.assertNotIn("glossary_json", first.settings)
        self.assertNotIn("glossary", pipeline.Pipeline(self.options()).settings)


class SafetyTests(PipelineCase):
    def test_overlap_both_directions_and_occupied_root(self):
        for output in (self.game, self.game / "nested", self.root):
            with self.subTest(output=output), self.assertRaises(pipeline.PipelineError):
                pipeline.preflight(self.options(output=str(output)))
        occupied = self.root / "occupied"
        occupied.mkdir()
        with self.assertRaises(pipeline.PipelineError):
            pipeline.preflight(self.options(output=str(occupied)))
        self.assertEqual(list(occupied.iterdir()), [])

    def test_link_components_and_tree_are_refused(self):
        link = self.root / "alias"
        try:
            link.symlink_to(self.game, target_is_directory=True)
        except OSError as error:
            self.skipTest(f"symlinks unavailable: {error}")
        with self.assertRaises(pipeline.PipelineError):
            pipeline.preflight(self.options(game=str(link)))
        with self.assertRaises(pipeline.PipelineError):
            pipeline.preflight(self.options(output=str(link / "out")))
        (self.game / "linked").symlink_to(self.cli)
        with self.assertRaises(pipeline.PipelineError):
            pipeline.preflight(self.options())

    def test_installed_state_and_archives_refused(self):
        for name in (".locust", ".locust-injections", "game/archive.rpa", "scenario/data.xp3"):
            game = self.root / str(len(list(self.root.iterdir())))
            (game / Path(name).parent).mkdir(parents=True, exist_ok=True)
            (game / name).write_bytes(b"do not inspect archive")
            with self.subTest(name=name), self.assertRaises(pipeline.PipelineError):
                pipeline.preflight(self.options(game=str(game)))

    def test_endpoint_literal_loopback_only(self):
        for endpoint in ("https://example.com", "http://localhost:7", "http://127.0.0.1.evil:7",
                         "http://user:pass@127.0.0.1:7", "http://127.0.0.1:7/?key=secret",
                         "http://127.0.0.1:0", "http://127.0.0.1:7\n"):
            with self.subTest(endpoint=endpoint), self.assertRaises(pipeline.PipelineError):
                pipeline.preflight(self.options(test_endpoint=endpoint))

    def test_blank_stale_malformed_and_shrunken_rows_refused(self):
        valid = [row(self.game)]
        baseline = pipeline.inventory(valid, self.game, "renpy")
        variants = [[], [row(self.game, translation=" ")], [row(self.game, status="pending")],
                    [row(self.game, metadata={"format_id": "renpy", "locust_stale_translation": None})],
                    [row(self.game, metadata=[])], [row(self.game, source="changed")]]
        for rows in variants:
            with self.subTest(rows=rows), self.assertRaises(pipeline.PipelineError):
                pipeline.gate_rows(rows, baseline, self.game, "renpy", ready=True)

    def test_malformed_warning_skip_and_outcome_reports_refused(self):
        report = {"mode": "direct", "languages_processed": ["es"], "languages_failed": [],
                  "backup_id": "owned", "pristine_backup_id": "owned",
                  "backup_path": str(self.root / "run/backups/owned"),
                  "files_modified": 1, "strings_written": 1, "strings_skipped": 0,
                  "warnings": [], "files_written": [str(self.game / "game/script.rpy")],
                  "reports": {"es": {"skip_reasons": {}, "files_modified": 1, "strings_written": 1,
                                     "strings_skipped": 0, "warnings": [],
                                     "files_written": [str(self.game / "game/script.rpy")]}},
                  "outcomes": [["es", {"Recorded": {"files": 1}}]]}
        variants = []
        for key, value in (("strings_written", True), ("strings_skipped", -1), ("warnings", ["partial"]),
                           ("languages_failed", [["es", "failed"]]), ("outcomes", [["es", "NothingRecorded"]])):
            broken = copy.deepcopy(report)
            broken[key] = value
            variants.append(broken)
        broken = copy.deepcopy(report)
        broken["reports"]["es"]["skip_reasons"] = {"unclassified": 1}
        variants.append(broken)
        for edit in (lambda value: value["reports"]["es"].update(files_modified=True),
                     lambda value: value.update(outcomes=[["es", {"Recorded": {"files": True}}]])):
            broken = copy.deepcopy(report)
            edit(broken)
            variants.append(broken)
        for value in variants:
            with self.subTest(value=value), self.assertRaises(pipeline.PipelineError):
                pipeline.gate_injection(value, [row(self.game)], self.game, self.root / "run/backups")


class AdditionalGates(PipelineCase):
    def test_filesystem_directory_alias_cannot_hide_source_output_overlap(self):
        alias_parent = self.root / "alias-parent"
        alias_parent.mkdir()
        real_identity = pipeline.directory_identity
        source_identity = real_identity(self.game / "game")
        def identity(path):
            return source_identity if Path(path) == alias_parent else real_identity(path)
        with patch.object(pipeline, "directory_identity", side_effect=identity):
            with self.assertRaises(pipeline.PipelineError):
                pipeline.preflight(self.options(output=str(alias_parent / "fresh")))
        self.assertFalse((alias_parent / "fresh").exists())

    def test_parent_traversal_does_not_hide_link_components(self):
        with self.assertRaises(pipeline.PipelineError):
            pipeline.preflight(self.options(game=str(self.root / "alias/../source")))

    def test_sensitive_names_hardlinks_and_special_files(self):
        # Never read a sensitive member: the name is rejected before hashing.
        (self.game / ".env").write_bytes(b"synthetic, not a credential")
        with patch.object(pipeline, "file_hash", wraps=pipeline.file_hash) as hashed:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.preflight(self.options())
            self.assertFalse(any(Path(call.args[0]).name == ".env" for call in hashed.call_args_list))
        other = self.root / "hardlinked"
        other.mkdir()
        try:
            os.link(self.cli, other / "same.exe")
        except OSError as error:
            self.skipTest(f"hardlinks unavailable: {error}")
        with self.assertRaises(pipeline.PipelineError):
            pipeline.scan(other)

    @unittest.skipUnless(hasattr(os, "mkfifo"), "POSIX FIFO branch; Windows reparse branch tested separately")
    def test_fifo_refused_without_open(self):
        os.mkfifo(self.game / "pipe")
        with self.assertRaises(pipeline.PipelineError):
            pipeline.preflight(self.options())

    def test_reparse_attribute_detected_even_without_symlink_mode(self):
        class Junction:
            st_mode = 0o040755
            st_file_attributes = 0x400
        self.assertTrue(pipeline.linked(Junction()))

    def test_nonsecret_environment_and_subscription_discovery(self):
        inherited = {"LOCUST_CONFIG": "owner-secret-config", "LOCUST_DATA_DIR": "owner-data",
                     "HTTP_PROXY": "owner-proxy", "https_proxy": "owner-proxy", "ALL_PROXY": "owner-proxy",
                     "OPENAI_API_KEY": "not-real", "PATH": "agy-path", "HOME": "owner-profile",
                     "USERPROFILE": "owner-profile", "APPDATA": "owner-roaming", "LOCALAPPDATA": "owner-local"}
        with patch.dict(os.environ, inherited, clear=True):
            env = pipeline.child_environment(self.root / "run", self.options(test_endpoint="http://127.0.0.1:7"))
            self.assertNotIn("LOCUST_CONFIG", env)
            self.assertFalse(any("proxy" in key.lower() and key != "NO_PROXY" for key in env))
            self.assertNotIn("OPENAI_API_KEY", env)
            self.assertEqual(env["PATH"], "agy-path")
            self.assertEqual(env["HOME"], str(self.root / "run/profile"))
            subscription = pipeline.child_environment(self.root / "run", self.options(provider="antigravity"))
            self.assertEqual(subscription["HOME"], "owner-profile")
            self.assertEqual(subscription["USERPROFILE"], "owner-profile")
            self.assertEqual(subscription["LOCUST_DATA_DIR"], str(self.root / "run/profile"))
            self.assertNotIn("LOCUST_CONFIG", subscription)
            self.assertNotIn("OPENAI_API_KEY", subscription)
            ordinary = pipeline.child_environment(self.root / "run", self.options(provider="antigravity"), subscription_login=False)
            self.assertEqual(ordinary["HOME"], str(self.root / "run/profile"))
            self.assertEqual(ordinary["USERPROFILE"], str(self.root / "run/profile"))

    def test_preflight_io_failure_returns_nonsecret_json(self):
        output = StringIO()
        arguments = ["--cli", str(self.cli), "--game", str(self.game), "--output", str(self.root / "run"),
                     "--format", "renpy", "--provider", "google"]
        with patch.object(pipeline, "preflight", side_effect=OSError("synthetic private diagnostic")), patch("sys.stdout", output):
            self.assertEqual(pipeline.main(arguments), 1)
        summary = json.loads(output.getvalue())
        self.assertEqual(summary["status"], "failed")
        self.assertNotIn("private", summary["diagnostic"])

    def test_json_counts_duplicates_nan_and_controls(self):
        for text in ('{"count":0,"count":0,"issues":[]}', '{"count":NaN}', 'no json'):
            with self.assertRaises(pipeline.PipelineError):
                pipeline.strict_json(text)
        for report in ({"count": False, "issues": []}, {"count": 1, "issues": []}, {"count": 0, "issues": ["warning"]}):
            with self.assertRaises(pipeline.PipelineError):
                pipeline.gate_validation(report)
        good = row(self.game, source='こんにちは [emb exp="f.player"][l]', translation='Hello [emb exp="f.player"][l]',
                   metadata={"format_id": "kirikiri"})
        baseline = pipeline.inventory([good], self.game, "kirikiri")
        pipeline.gate_rows([good], baseline, self.game, "kirikiri", ready=True)
        bad = copy.deepcopy(good)
        bad["translation"] = 'Hello [emb exp="f.other"][l]'
        with self.assertRaises(pipeline.PipelineError):
            pipeline.gate_rows([bad], baseline, self.game, "kirikiri", ready=True)

    def test_resource_identity_duplicates_and_bounded_wrap(self):
        original = row(self.game, id="Map#event_1#page_0#cmd_1#msg", source="森へ\n進む", translation="Continuar hacia el bosque", tags=["dialogue"],
                       metadata={"format_id": "rpgmaker-mv"})
        actual = dict(original, id="Map#event_1#page_0#cmd_3#msg", source="Continuar hacia el bosque")
        evidence = pipeline.compare_reextraction([original], self.game, [actual], self.game, "rpgmaker-mv")
        self.assertEqual(evidence["normalized_rpg_dialogues"], 0)
        for invalid in ([], [actual, actual], [dict(actual, source="Continuar el bosque")],
                        [dict(actual, id="Map#event_2#page_0#cmd_3#msg")]):
            with self.assertRaises(pipeline.PipelineError):
                pipeline.compare_reextraction([original], self.game, invalid, self.game, "rpgmaker-mv")

    def test_exact_neutral_writer_layouts_and_whitespace_corruption(self):
        for label, kind, physical, literal, written in LAYOUT_CASES:
            with self.subTest(label=label):
                original = layout_row(kind, physical, literal)
                actual = dict(original, source=written)
                root = Path("C:/fixture")
                self.assertEqual(pipeline.compare_reextraction([original], root, [actual], root, "rpgmaker-mv")["resources_verified"], 1)
                for corrupt in (written + " ", written + "!", written.replace(" ", "\n", 1)):
                    if corrupt != written:
                        with self.assertRaises(pipeline.PipelineError):
                            pipeline.compare_reextraction([original], root, [dict(actual, source=corrupt)], root, "rpgmaker-mv")

    def test_layout_cannot_accept_controls_tags_lists_blanks_or_position_corruption(self):
        for original, actual in layout_rejections():
            with self.subTest(actual=actual), self.assertRaises(pipeline.PipelineError):
                pipeline.compare_reextraction([original], Path("C:/fixture"), [actual], Path("C:/fixture"), "rpgmaker-mv")

    def test_readonly_sqlite_wal_not_immutable(self):
        # Metadata-only fixture. No translations are seeded by SQL.
        database = self.root / "wal.db"
        connection = sqlite3.connect(database)
        self.addCleanup(connection.close)
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute("PRAGMA wal_autocheckpoint=0")
        connection.execute("CREATE TABLE strings (" + ",".join(key + " TEXT" for key in pipeline.COLUMNS) + ")")
        connection.execute("CREATE TABLE project_metadata (key TEXT, value TEXT)")
        connection.execute("CREATE TABLE translation_memory (source TEXT)")
        connection.execute("INSERT INTO project_metadata VALUES ('extraction_warnings','[]')")
        connection.commit()
        before = pipeline.file_hash(database)
        rows, metadata = pipeline.read_database(database)
        self.assertEqual(rows, [])
        self.assertEqual(metadata["extraction_warnings"], [])
        self.assertEqual(before, pipeline.file_hash(database))
        connection.execute("UPDATE project_metadata SET value='[\"partial\"]'")
        connection.commit()
        with self.assertRaises(pipeline.PipelineError):
            pipeline.read_database(database)


class MockCli:
    """Virtual rows only: Python never writes SQL translations, even in mocks."""
    def __init__(self, case):
        self.case, self.rows, self.calls = case, {}, []
        self.missing = True
        self.timeout_stage = None
        self.inject_called_after_marker = False

    def database(self, path):
        # Metadata-only SQLite fixture for the independent glossary reader.
        # Resource/translation rows remain virtual; Python never seeds them.
        connection = sqlite3.connect(path)
        try:
            connection.execute("CREATE TABLE glossary (term TEXT, translation TEXT, lang_pair TEXT, context TEXT, case_sensitive INTEGER)")
            connection.commit()
        finally:
            connection.close()

    def read(self, path):
        return copy.deepcopy(self.rows[Path(path)]), {"extraction_warnings": [], "game_root": str(self.case.root / "run/work-game")}

    def run(self, argv, **kwargs):
        self.case.assertEqual(argv[:3], [str(self.case.cli), "--config", str(self.case.root / "run/profile/config.json")])
        args = argv[3:]
        self.calls.append(args)
        self.case.assertEqual(kwargs["cwd"], self.case.root / "run")
        self.case.assertEqual(kwargs["timeout"], 900)
        self.case.assertNotIn(str(self.case.game), list(map(str, args)))
        self.case.assertNotIn("LOCUST_CONFIG", kwargs["env"])
        if args[0] == self.timeout_stage:
            raise subprocess.TimeoutExpired(argv, kwargs["timeout"])
        stdout, returncode = b"", 0
        if args[0] == "extract":
            db = Path(args[-1])
            self.database(db)
            self.rows[db] = [row(Path(args[1]), translation=None, status="pending", provider_used=None, translated_at=None)]
        elif args[0] == "translate":
            db = Path(args[1])
            if self.missing:
                self.missing = False
            else:
                for value in self.rows[db]:
                    if value["status"] == "pending":
                        value.update(translation="Hello" if "ja" in args else "Hola", status="translated",
                                     provider_used="google", translated_at="2026-01-02")
        elif args[0] == "validate":
            stdout = b'{"count":0,"issues":[]}'
        elif args[0] == "pivot":
            source, target = Path(args[1]), Path(args[3])
            self.rows[target] = copy.deepcopy(self.rows[source])
            self.database(target)
            for value in self.rows[target]:
                value["metadata"]["locust_injection_source"] = value["source"]
                value.update(source=value["translation"], translation=None, status="pending", provider_used=None, translated_at=None)
        elif args[0] == "inject" and "--help" not in args:
            manifest = pipeline.read_json_file(self.case.root / "run/pipeline-manifest.json")
            self.inject_called_after_marker = manifest["injection_started"] is True
            returncode = 1
        return subprocess.CompletedProcess(argv, returncode, stdout, b"do not log source/credentials")


class GlossaryOrchestration(GlossaryRules):
    def session(self, options, fault=None):
        cli = MockCli(self)
        cli.missing = False
        glossaries = {}
        actual = cli.run
        def run(argv, **kwargs):
            args = argv[3:]
            result = actual(argv, **kwargs)
            if args[0] in {"extract", "pivot"}:
                glossaries[Path(args[-1] if args[0] == "extract" else args[3])] = []
            if args[:2] == ["glossary", "add"]:
                entry = tuple(a.split("=", 1)[1] for a in args if a.startswith(("--term=", "--translation=")))
                glossaries[Path(args[2])].append((*entry, args[-1], None, 0))
                glossaries[Path(args[2])].sort()
                if fault == "wrong-case":
                    value = glossaries[Path(args[2])][-1]
                    glossaries[Path(args[2])][-1] = (*value[:-1], 1)
            if args[0] == "translate":
                db = Path(args[1])
                for r in cli.rows[db]:
                    if r["source"] == ("こんにちは" if db.name == "english.db" else "Hello"):
                        r["provider_used"] = "glossary"
                        if fault == "unbound-target":
                            r["translation"] = "Unexpected"
            return result
        cli.glossary_run = run
        return cli, glossaries, patch.object(pipeline, "read_database", side_effect=cli.read), patch.object(
            pipeline, "read_glossary", side_effect=lambda path: list(glossaries[Path(path)])), patch.object(
            pipeline.subprocess, "run", side_effect=run)

    def test_phase_isolation_and_exact_glossary_provider_proofs(self):
        path = self.glossary()
        options = self.options(glossary_json=str(path))
        cli, glossaries, reader, glossary_reader, runner = self.session(options)
        with reader, glossary_reader, runner:
            with self.assertRaisesRegex(pipeline.PipelineError, "inject-spanish"):
                pipeline.Pipeline(options).run()
        self.assertEqual(glossaries[self.root / "run/english.db"], [("こんにちは", "Hello", "ja-en", None, 0)])
        self.assertEqual(glossaries[self.root / "run/spanish.db"], [("Hello", "Hola", "en-es", None, 0)])
        manifest = pipeline.read_json_file(self.root / "run/pipeline-manifest.json")
        for database in ("english.db", "spanish.db"):
            self.assertEqual({v["provider"] for v in manifest["accepted_rows"][database].values()}, {"glossary"})
        self.assertTrue(all("policy_sha256" in stage for stage in manifest["stages"]))
        self.assertEqual(sum(c[:2] == ["glossary", "add"] for c in cli.calls), 2)

    def test_wrong_db_case_policy_and_forged_glossary_attribution_refuse(self):
        original_root = self.root
        for fault, message in (("wrong-case", "phase glossary drift"), ("unbound-target", "Unbound exact glossary")):
            with self.subTest(fault=fault):
                self.root = original_root / fault
                self.root.mkdir()
                options = self.options(glossary_json=str(self.glossary()))
                cli, _, reader, glossary_reader, runner = self.session(options, fault)
                with reader, glossary_reader, runner:
                    with self.assertRaisesRegex(pipeline.PipelineError, message):
                        pipeline.Pipeline(options).run()
                self.assertFalse(any(c[0] == "pivot" for c in cli.calls))
        self.root = original_root

    def test_absent_glossary_still_requires_independent_empty_db_table(self):
        cli = MockCli(self)
        options = self.options()
        with patch.object(pipeline, "read_database", side_effect=cli.read), patch.object(pipeline.subprocess, "run", side_effect=cli.run):
            with self.assertRaisesRegex(pipeline.PipelineError, "Missing/blank/pending"):
                pipeline.Pipeline(options).run()
            before = pipeline.scan(self.root / "run")
            with patch.object(pipeline, "read_glossary", return_value=[("Unexpected", "Otro", "en-es", None, 0)]), patch.object(pipeline.subprocess, "run") as child:
                with self.assertRaisesRegex(pipeline.PipelineError, "phase glossary drift"):
                    pipeline.Pipeline(replace(options, resume=True)).run()
                child.assert_not_called()
            self.assertEqual(pipeline.scan(self.root / "run"), before)

    def test_coordinated_db_manifest_policy_forgery_refuses_without_publication(self):
        options = self.options(glossary_json=str(self.glossary()))
        cli, glossaries, reader, glossary_reader, runner = self.session(options)
        # Leave a resumable pending checkpoint, with complete CLI glossary setup.
        def pending(argv, **kwargs):
            if argv[3] == "translate":
                return subprocess.CompletedProcess(argv, 0, b"", b"")
            return cli.glossary_run(argv, **kwargs)
        with reader, glossary_reader, patch.object(pipeline.subprocess, "run", side_effect=pending):
            with self.assertRaisesRegex(pipeline.PipelineError, "Missing/blank/pending"):
                pipeline.Pipeline(options).run()
            path = self.root / "run/pipeline-manifest.json"
            original = pipeline.read_json_file(path)
            pipeline.validate_resume_manifest(original)
            for fault in ("db-target", "db-case", "db-context", "db-pair", "db-extra", "db-missing", "policy", "stage", "config"):
                changed = copy.deepcopy(original)
                glossaries[self.root / "run/english.db"] = [("こんにちは", "Hello", "ja-en", None, 0)]
                if fault.startswith("db-"):
                    variants = {
                        "db-target": [("こんにちは", "Untrusted", "ja-en", None, 0)],
                        "db-case": [("こんにちは", "Hello", "ja-en", None, 1)],
                        "db-context": [("こんにちは", "Hello", "ja-en", "unexpected", 0)],
                        "db-pair": [("こんにちは", "Hello", "en-es", None, 0)],
                        "db-extra": [("こんにちは", "Hello", "ja-en", None, 0), ("Other", "Otro", "en-es", None, 0)],
                        "db-missing": [],
                    }
                    glossaries[self.root / "run/english.db"] = variants[fault]
                    probe = pipeline.Pipeline(replace(options, resume=True))
                    changed["databases"]["english.db"] = probe.snapshot(probe.en)
                elif fault == "policy":
                    changed["settings"]["glossary"]["ja-en"][0]["translation"] = "Untrusted"
                    fingerprint = pipeline.policy_fingerprint(changed["settings"], pipeline.permitted_config(options))
                    for stage in changed["stages"]:
                        stage["policy_sha256"] = fingerprint
                elif fault == "stage":
                    changed["stages"][-1]["policy_sha256"] = "0" * 64
                else:
                    config = self.root / "run/profile/config.json"
                    config.write_bytes(pipeline.encoded({"providers": {}, "unknown": True}))
                    changed["config"] = pipeline.file_hash(config)
                path.write_bytes(pipeline.encoded(changed))
                before = pipeline.scan(self.root / "run")
                with patch.object(pipeline.subprocess, "run") as child:
                    with self.assertRaises(pipeline.PipelineError):
                        pipeline.Pipeline(replace(options, resume=True)).run()
                    child.assert_not_called()
                self.assertEqual(pipeline.scan(self.root / "run"), before)
                (self.root / "run/profile/config.json").write_bytes(pipeline.encoded(pipeline.permitted_config(options)))


class FallbackTests(PipelineCase):
    def test_explicit_fallback_preserves_primary_rows_and_completes_both_stages(self):
        options = self.options(provider="antigravity", fallback_provider="google")
        cli = MockCli(self)
        cli.missing = False
        actual = cli.run
        ready_before = {}
        def run(argv, **kwargs):
            args = argv[3:]
            result = actual(argv, **kwargs)
            if args[0] == "extract":
                db = Path(args[-1])
                cli.rows[db].append(dict(copy.deepcopy(cli.rows[db][0]), id="script.rpy#2", source="進む"))
            if args[0] == "translate":
                db, provider = Path(args[1]), args[args.index("-p") + 1]
                if provider == "antigravity":
                    cli.rows[db][0]["provider_used"] = provider
                    ready_before[db] = copy.deepcopy(cli.rows[db][0])
                    cli.rows[db][1].update(translation=None, status="pending", provider_used=None, translated_at=None)
                else:
                    self.assertEqual(kwargs["env"]["HOME"], str(self.root / "run/profile"))
                    self.assertEqual(cli.rows[db][0], ready_before[db])
            return result
        with patch.object(pipeline, "read_database", side_effect=cli.read), patch.object(pipeline.subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(pipeline.PipelineError, "inject-spanish"):
                pipeline.Pipeline(options).run()
        manifest = pipeline.read_json_file(self.root / "run/pipeline-manifest.json")
        self.assertEqual([s["stage"] for s in manifest["stages"] if "fallback" in s["stage"]],
                         ["ja-en-fallback-google", "en-es-fallback-google"])
        for database in ("english.db", "spanish.db"):
            rows = cli.rows[self.root / "run" / database]
            self.assertEqual([r["provider_used"] for r in rows], ["antigravity", "google"])
            self.assertEqual(len(rows), 2)

    def test_complete_primary_never_invokes_optional_fallback(self):
        cli = MockCli(self)
        cli.missing = False
        actual = cli.run
        def run(argv, **kwargs):
            result = actual(argv, **kwargs)
            if argv[3] == "translate":
                self.assertIn("antigravity", argv)
                for r in cli.rows[Path(argv[4])]:
                    r["provider_used"] = "antigravity"
            return result
        with patch.object(pipeline, "read_database", side_effect=cli.read), patch.object(pipeline.subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(pipeline.PipelineError, "inject-spanish"):
                pipeline.Pipeline(self.options(provider="antigravity", fallback_provider="google")).run()
        self.assertEqual(sum(c[0] == "translate" for c in cli.calls), 2)
        self.assertFalse(any("google" in c for c in cli.calls))

    def test_fallback_settings_are_explicit_and_test_routes_cannot_conflict(self):
        for changes in ({"fallback_provider": "openai"}, {"fallback_provider": "google"},
                        {"provider": "antigravity", "fallback_provider": "google", "test_endpoint": "http://127.0.0.1:7"},
                        {"provider": "antigravity", "fallback_provider": "google", "test_endpoint": "http://127.0.0.1:7",
                         "test_fallback_endpoint": "http://127.0.0.1:7"}):
            with self.subTest(changes=changes), self.assertRaises(pipeline.PipelineError):
                pipeline.preflight(self.options(**changes))


    def test_failed_timed_out_or_unpublished_primary_never_falls_back(self):
        parent = self.root
        for fault in ("failure", "timeout", "publication"):
            with self.subTest(fault=fault):
                self.root = parent / fault
                self.root.mkdir()
                cli = MockCli(self)
                actual_run, actual_replace = cli.run, pipeline.os.replace
                def run(argv, **kwargs):
                    if argv[3] == "translate":
                        if fault == "timeout":
                            raise subprocess.TimeoutExpired(argv, kwargs["timeout"])
                        if fault == "failure":
                            return subprocess.CompletedProcess(argv, 1, b"", b"synthetic")
                    return actual_run(argv, **kwargs)
                def publish(source, target):
                    manifest = pipeline.read_json_file(Path(source))
                    if fault == "publication" and manifest["stages"] and manifest["stages"][-1]["stage"] == "ja-en":
                        raise OSError(errno.EIO, "synthetic publication failure")
                    return actual_replace(source, target)
                with patch.object(pipeline, "read_database", side_effect=cli.read), patch.object(pipeline.subprocess, "run", side_effect=run) as child, patch.object(pipeline.os, "replace", side_effect=publish):
                    with self.assertRaises(pipeline.PipelineError):
                        pipeline.Pipeline(self.options(provider="antigravity", fallback_provider="google")).run()
                self.assertEqual(sum(c.args[0][3] == "translate" for c in child.call_args_list), 1)
                self.assertFalse((self.root / "run/spanish.db").exists())
        self.root = parent

    def test_fallback_failure_timeout_and_unproven_rows_cannot_advance(self):
        parent = self.root
        for fault in ("failure", "timeout", "provider", "static", "ready-columns", "unknown-control"):
            with self.subTest(fault=fault):
                self.root = parent / fault
                self.root.mkdir()
                cli = MockCli(self)
                cli.missing = False
                actual = cli.run
                def run(argv, **kwargs):
                    args = argv[3:]
                    fallback = args[0] == "translate" and "google" in args
                    if fallback and fault == "failure":
                        return subprocess.CompletedProcess(argv, 1, b"", b"synthetic")
                    if fallback and fault == "timeout":
                        raise subprocess.TimeoutExpired(argv, kwargs["timeout"])
                    result = actual(argv, **kwargs)
                    if args[0] == "extract":
                        db = Path(args[-1])
                        cli.rows[db].append(dict(copy.deepcopy(cli.rows[db][0]), id="script.rpy#2", source="進む"))
                    if args[0] == "translate":
                        db = Path(args[1])
                        if not fallback:
                            cli.rows[db][0]["provider_used"] = "antigravity"
                            cli.rows[db][1].update(translation=None, status="pending", provider_used=None, translated_at=None)
                        elif fault == "provider":
                            cli.rows[db][1]["provider_used"] = "unproven"
                        elif fault == "static":
                            cli.rows[db][1]["context"] = "changed"
                        elif fault == "ready-columns":
                            cli.rows[db][0]["reviewed_at"] = "changed"
                    if args[0] == "validate" and fault == "unknown-control":
                        report = {"count": 1, "issues": [{"entry_id": "script.rpy#1", "kind": "UnknownControl", "message": "unclassified"}]}
                        return subprocess.CompletedProcess(argv, 1, json.dumps(report).encode(), b"")
                    return result
                with patch.object(pipeline, "read_database", side_effect=cli.read), patch.object(pipeline.subprocess, "run", side_effect=run):
                    with self.assertRaises(pipeline.PipelineError):
                        pipeline.Pipeline(self.options(provider="antigravity", fallback_provider="google")).run()
                manifest = pipeline.read_json_file(self.root / "run/pipeline-manifest.json")
                self.assertFalse(manifest["injection_started"])
                self.assertFalse((self.root / "run/spanish.db").exists())
                self.assertEqual(manifest["phase"], "english-pending")
                if fault == "timeout":
                    self.assertTrue(manifest["uncertain_child"])
        self.root = parent

    def test_version_defaults_and_forged_fallback_stage_refuse_resume(self):
        cli = MockCli(self)
        options = self.options(provider="antigravity", fallback_provider="google")
        # Both transports leave pending; fallback finishes orderly but readiness
        # refuses. This is the exact safe pre-injection retry checkpoint.
        actual = cli.run
        def run(argv, **kwargs):
            result = actual(argv, **kwargs)
            if argv[3] == "translate":
                cli.rows[Path(argv[4])][0].update(translation=None, status="pending", provider_used=None, translated_at=None)
            return result
        with patch.object(pipeline, "read_database", side_effect=cli.read), patch.object(pipeline.subprocess, "run", side_effect=run):
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(options).run()
        path = self.root / "run/pipeline-manifest.json"
        original = pipeline.read_json_file(path)
        pipeline.validate_resume_manifest(original)
        with patch.object(pipeline, "read_database", side_effect=cli.read), patch.object(pipeline.subprocess, "run") as child:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(replace(options, fallback_provider=None, resume=True)).run()
            child.assert_not_called()
        for mutate in (lambda m: m.update(version=1),
                       lambda m: m["stages"][-2].pop("classified_identical_resources"),
                       lambda m: m["settings"].update(fallback_provider=None),
                       lambda m: m["accepted_rows"]["english.db"].update({"script.rpy#1": {"row_sha256": "0" * 64, "stage": "forged", "provider": "google"}})):
            changed = copy.deepcopy(original)
            mutate(changed)
            with self.assertRaises(pipeline.PipelineError):
                pipeline.validate_resume_manifest(changed)


class OrchestratorTests(PipelineCase):
    def mock_session(self):
        cli = MockCli(self)
        return cli, patch.object(pipeline, "read_database", side_effect=cli.read), patch.object(pipeline.subprocess, "run", side_effect=cli.run)

    def test_pending_translation_resume_and_injection_boundary(self):
        mock_cli, reader, runner = self.mock_session()
        with reader, runner:
            with self.assertRaisesRegex(pipeline.PipelineError, "Missing/blank/pending"):
                pipeline.Pipeline(self.options()).run()
            before = pipeline.read_json_file(self.root / "run/pipeline-manifest.json")
            self.assertEqual(before["phase"], "english-pending")
            self.assertFalse(before["injection_started"])
            self.assertFalse((self.root / "run/spanish.db").exists())
            with self.assertRaisesRegex(pipeline.PipelineError, "Child command failed"):
                pipeline.Pipeline(self.options(resume=True)).run()
            self.assertTrue(mock_cli.inject_called_after_marker)
            self.assertEqual(sum(args[0] == "extract" for args in mock_cli.calls), 1)
            self.assertEqual(sum(args[0] == "pivot" for args in mock_cli.calls), 1)
            self.assertTrue(all("--no-memory" in args for args in mock_cli.calls if args[0] == "translate"))
            self.assertTrue(all("--allow-partial" in args for args in mock_cli.calls if args[0] == "translate"))
            with self.assertRaisesRegex(pipeline.PipelineError, "Resume refused"):
                pipeline.Pipeline(self.options(resume=True)).run()

    def test_resume_rejects_settings_and_database_drift(self):
        mock_cli, reader, runner = self.mock_session()
        with reader, runner:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(self.options()).run()
            total = len(mock_cli.calls)
            for option in (self.options(resume=True, concurrency=2), self.options(resume=True, provider="antigravity")):
                with self.assertRaises(pipeline.PipelineError):
                    pipeline.Pipeline(option).run()
            self.assertEqual(len(mock_cli.calls), total)
            database = self.root / "run/english.db"
            database.write_bytes(b"changed virtual DB")
            with self.assertRaisesRegex(pipeline.PipelineError, "database hashes"):
                pipeline.Pipeline(self.options(resume=True)).run()
            # No recovery or overwrite of a drifted database.
            self.assertEqual(database.read_bytes(), b"changed virtual DB")

    def test_shared_layout_helper_drift_cannot_resume(self):
        mock_cli, reader, runner = self.mock_session()
        with reader, runner:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(self.options()).run()
            count = len(mock_cli.calls)
            actual_hash = pipeline.file_hash
            def fingerprint(path):
                return {"sha256": "0" * 64, "size": 1} if Path(path).name == "qa_replay_oracle.py" else actual_hash(path)
            with patch.object(pipeline, "file_hash", side_effect=fingerprint):
                with self.assertRaisesRegex(pipeline.PipelineError, "tool drift"):
                    pipeline.Pipeline(self.options(resume=True)).run()
            self.assertEqual(len(mock_cli.calls), count)

    def test_resume_rejects_source_binary_work_pristine_config_and_inventory_drift(self):
        mock_cli, reader, runner = self.mock_session()
        with reader, runner:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(self.options()).run()
            count = len(mock_cli.calls)
            paths = [self.game / "game/script.rpy", self.cli,
                     self.root / "run/work-game/game/script.rpy", self.root / "run/pristine/game/script.rpy",
                     self.root / "run/profile/config.json"]
            for path in paths:
                with self.subTest(path=path.name):
                    before, mode = path.read_bytes(), path.stat().st_mode
                    path.chmod(0o600)
                    path.write_bytes(b"synthetic drift")
                    try:
                        with self.assertRaises(pipeline.PipelineError):
                            pipeline.Pipeline(self.options(resume=True)).run()
                        self.assertEqual(len(mock_cli.calls), count)
                    finally:
                        path.write_bytes(before)
                        path.chmod(mode)
            database = self.root / "run/english.db"
            mock_cli.rows[database][0]["source"] = "different Japanese inventory"
            with self.assertRaisesRegex(pipeline.PipelineError, "database hashes/inventory drift"):
                pipeline.Pipeline(self.options(resume=True)).run()
            self.assertEqual(len(mock_cli.calls), count)

    def assert_resume_refused_without_child(self, options):
        # A launch sentinel prevents a vulnerable implementation from changing
        # the virtual DB or contaminating subsequent coordinated mutations.
        with patch.object(pipeline.subprocess, "run", side_effect=AssertionError("Untrusted resume attempted child launch")) as child:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(replace(options, resume=True)).run()
            child.assert_not_called()

    def test_coordinated_config_and_manifest_mutations_never_launch_children(self):
        options = self.options(test_endpoint="http://127.0.0.1:7")
        mock_cli, reader, runner = self.mock_session()
        with reader, runner:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(options).run()
            root = Path(options.output)
            manifest_path, config_path = root / "pipeline-manifest.json", root / "profile/config.json"
            original = pipeline.read_json_file(manifest_path)
            config = pipeline.read_json_file(config_path)
            variants = []
            for endpoint in ("http://192.0.2.1:7000", "http://127.0.0.1:8"):
                altered = copy.deepcopy(config)
                altered["providers"][pipeline.PROVIDER]["base_url"] = endpoint
                variants.append(("endpoint", pipeline.encoded(altered)))
            for provider in ("antigravity", "unexpected"):
                altered = copy.deepcopy(config)
                altered["providers"][provider] = {"extra": {"agy_path": str(root / "profile/untrusted-agy.cmd")}}
                variants.append(("executable", pipeline.encoded(altered)))
            for key in ("unknown", "http_proxy"):
                variants.append(("unknown-config-field", pipeline.encoded(dict(config, **{key: "untrusted"}))))
            altered = copy.deepcopy(config)
            altered["providers"][pipeline.PROVIDER]["extra"]["agy_path"] = "untrusted-agy"
            variants.append(("unknown-provider-key", pipeline.encoded(altered)))
            variants.append(("noncanonical-bytes", json.dumps(config, indent=2).encode()))
            count = len(mock_cli.calls)
            for name, data in variants:
                with self.subTest(mutation=name):
                    config_path.write_bytes(data)
                    altered_manifest = copy.deepcopy(original)
                    # Coordinated forgery: the recorded hash/size agrees with
                    # the attack. Hash agreement must never authorize config.
                    altered_manifest["config"] = pipeline.file_hash(config_path)
                    manifest_path.write_bytes(pipeline.encoded(altered_manifest))
                    self.assert_resume_refused_without_child(options)
                    self.assertEqual(len(mock_cli.calls), count, "Refusal must precede every resumed child")
                    self.assertEqual(manifest_path.read_bytes(), pipeline.encoded(altered_manifest))
            config_path.write_bytes(pipeline.encoded(config))
            manifest_path.write_bytes(pipeline.encoded(original))

    def test_subscription_config_cannot_grant_agy_path_on_resume(self):
        options = self.options(provider="antigravity")
        mock_cli, reader, runner = self.mock_session()
        with reader, runner:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(options).run()
            root = Path(options.output)
            config_path, manifest_path = root / "profile/config.json", root / "pipeline-manifest.json"
            forged = {"providers": {"antigravity": {"extra": {"agy_path": str(root / "profile/agy.cmd")}}}}
            config_path.write_bytes(pipeline.encoded(forged))
            manifest = pipeline.read_json_file(manifest_path)
            manifest["config"] = pipeline.file_hash(config_path)
            manifest_path.write_bytes(pipeline.encoded(manifest))
            count = len(mock_cli.calls)
            self.assert_resume_refused_without_child(options)
            self.assertEqual(len(mock_cli.calls), count)

    def test_resume_manifest_schema_is_closed_typed_and_ordered(self):
        mock_cli, reader, runner = self.mock_session()
        with reader, runner:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(self.options()).run()
            path = self.root / "run/pipeline-manifest.json"
            original = pipeline.read_json_file(path)
            def validation_mutation(manifest, count, code=0):
                manifest["phase"] = "english-ready"
                manifest["stages"].append(dict(manifest["stages"][-1], stage="validate-english", exit_code=code,
                                                classified_identical_resources=count))
            mutations = [
                ("unknown", lambda m: m.update(untrusted="passthrough")),
                ("settings-key", lambda m: m["settings"].update(untrusted="passthrough")),
                ("binary-key", lambda m: m["binary"].update(untrusted="passthrough")),
                ("source-key", lambda m: m["source"].update(untrusted="passthrough")),
                ("file-key", lambda m: next(iter(m["source"]["files"].values())).update(untrusted=True)),
                ("database-key", lambda m: m["databases"]["english.db"].update(untrusted=True)),
                ("database-count", lambda m: m["databases"]["english.db"].update(rows=True)),
                ("unknown-database", lambda m: m["databases"].update(foreign=m["databases"]["english.db"])),
                ("stage-key", lambda m: m["stages"][0].update(untrusted=True)),
                ("stage-name", lambda m: m["stages"][0].update(stage="untrusted")),
                ("stage-order", lambda m: m["stages"].reverse()),
                ("stage-repeat", lambda m: m["stages"].append(m["stages"][0])),
                ("exit-type", lambda m: m["stages"][0].update(exit_code=True)),
                ("exit-range", lambda m: m["stages"][0].update(exit_code=2**64)),
                ("failed-predecessor", lambda m: m["stages"][0].update(exit_code=2)),
                ("duration-type", lambda m: m["stages"][0].update(seconds=True)),
                ("duration-negative", lambda m: m["stages"][0].update(seconds=-1)),
                ("duration-overflow", lambda m: m["stages"][0].update(seconds=10**400)),
                ("timeout-overflow", lambda m: m["settings"].update(timeout=10**400)),
                ("stdout-hash", lambda m: m["stages"][0].update(stdout_sha256="not-a-hash")),
                ("misplaced-count", lambda m: m["stages"][0].update(classified_identical_resources=0)),
                ("negative-count", lambda m: validation_mutation(m, -1)),
                ("boolean-count", lambda m: validation_mutation(m, True)),
                ("excess-count", lambda m: validation_mutation(m, 2)),
                ("count-code-mismatch", lambda m: validation_mutation(m, 0, 1)),
                ("settings-bool", lambda m: m["settings"].update(concurrency=True)),
                ("config-size-bool", lambda m: m["config"].update(size=True)),
                ("logical-hash", lambda m: m["databases"]["english.db"].update(logical_sha256=[])),
                ("db-file-key", lambda m: m["databases"]["english.db"]["files"][""].update(untrusted=True)),
                ("resource-hash", lambda m: m["japanese_inventory"].update({next(iter(m["japanese_inventory"])): False})),
                ("empty-stages", lambda m: m.update(stages=[])),
                ("active-command-type", lambda m: m.update(active_command=False)),
                ("uncertain-type", lambda m: m.update(uncertain_child=0)),
                ("injection-type", lambda m: m.update(injection_started=0)),
                ("status", lambda m: m.update(status="arbitrary")),
                ("completed-status", lambda m: m.update(status="completed")),
                ("diagnostic-type", lambda m: m.update(diagnostic=[])),
                ("run-id", lambda m: m.update(run_id=True)),
                ("missing-field", lambda m: m.pop("stages")),
                ("phase-history", lambda m: m.update(phase="spanish-ready")),
                ("premature-proof", lambda m: m.update(rollback_verified=True)),
            ]
            count = len(mock_cli.calls)
            for name, mutate in mutations:
                with self.subTest(mutation=name):
                    manifest = copy.deepcopy(original)
                    mutate(manifest)
                    path.write_bytes(pipeline.encoded(manifest))
                    self.assert_resume_refused_without_child(self.options())
                    self.assertEqual(len(mock_cli.calls), count, "Invalid manifest must launch no child")
                    self.assertEqual(path.read_bytes(), pipeline.encoded(manifest), "Invalid data must not be republished")
            path.write_bytes(pipeline.encoded(original))

    def test_ready_checkpoint_resumes_preserve_order_and_never_repeat_pivot(self):
        parent = self.root
        for phase, database in (("english-ready", "english.db"), ("spanish-ready", "spanish.db")):
            with self.subTest(phase=phase):
                self.root = parent / phase
                self.root.mkdir()
                mock_cli, reader, _ = self.mock_session()
                mock_cli.missing = False
                validations = 0
                def intercept(argv, **kwargs):
                    nonlocal validations
                    result = mock_cli.run(argv, **kwargs)
                    if argv[3] == "validate" and Path(argv[4]).name == database:
                        validations += 1
                        if validations == 2:
                            report = {"count": 1, "issues": [{"entry_id": "script.rpy#1", "kind": "Bad", "message": "synthetic"}]}
                            return subprocess.CompletedProcess(argv, 1, json.dumps(report).encode(), b"")
                    return result
                with reader, patch.object(pipeline.subprocess, "run", side_effect=intercept):
                    with self.assertRaisesRegex(pipeline.PipelineError, "Validation issue cannot release"):
                        pipeline.Pipeline(self.options()).run()
                    manifest = pipeline.read_json_file(self.root / "run/pipeline-manifest.json")
                    self.assertEqual(manifest["phase"], phase)
                    pipeline.validate_resume_manifest(manifest)
                    with self.assertRaisesRegex(pipeline.PipelineError, "Child command failed: inject-spanish"):
                        pipeline.Pipeline(self.options(resume=True)).run()
                    self.assertTrue(mock_cli.inject_called_after_marker)
                    self.assertEqual(sum(args[0] == "extract" for args in mock_cli.calls), 1)
                    self.assertEqual(sum(args[0] == "pivot" for args in mock_cli.calls), 1)
        self.root = parent

    def test_timeout_is_ambiguous_and_never_resumeable(self):
        mock_cli, reader, runner = self.mock_session()
        mock_cli.timeout_stage = "translate"
        with reader, runner:
            with self.assertRaisesRegex(pipeline.PipelineError, "descendant cleanup is NOT guaranteed"):
                pipeline.Pipeline(self.options()).run()
            manifest = pipeline.read_json_file(self.root / "run/pipeline-manifest.json")
            self.assertTrue(manifest["uncertain_child"])
            self.assertIsNotNone(manifest["active_command"])
            with self.assertRaisesRegex(pipeline.PipelineError, "Resume refused"):
                pipeline.Pipeline(self.options(resume=True)).run()


PUBLICATION_FAILURE = "pipeline-publication-failure.json"


class PublicationTests(PipelineCase):
    def publisher(self, name="run", durable=True):
        program = pipeline.Pipeline(self.options(output=str(self.root / name)))
        program.root.mkdir()
        program.manifest = {"owner": pipeline.OWNER, "version": pipeline.VERSION,
                            "run_id": "01234567-89ab-4cde-8fab-0123456789ab", "status": "running",
                            "phase": "english-pending", "injection_started": False,
                            "active_command": None, "uncertain_child": False, "stages": []}
        if durable:
            program.save()
        program.manifest["active_command"] = "ja-en"
        return program

    def fingerprint(self, path):
        info = path.stat()
        return (info.st_dev, info.st_ino, pipeline.file_hash(path))

    def denied(self):
        error = PermissionError(errno.EACCES, "synthetic primary private diagnostic")
        error.winerror = 5
        return error

    def test_transient_denials_retry_same_next_inode_and_bytes(self):
        actual_replace = pipeline.os.replace
        for denials in (1, 2, 3):
            with self.subTest(denials=denials):
                program = self.publisher("transient-" + str(denials))
                target = program.root / "pipeline-manifest.json"
                before, fingerprints = target.read_bytes(), []
                def intercept(source, destination):
                    self.assertEqual(Path(destination), target)
                    self.assertEqual(target.read_bytes(), before)
                    fingerprints.append(self.fingerprint(Path(source)))
                    self.assertEqual(fingerprints[-1], fingerprints[0])
                    if len(fingerprints) <= denials:
                        raise self.denied()
                    return actual_replace(source, destination)
                with patch.object(pipeline.os, "replace", side_effect=intercept) as moved, patch.object(pipeline.time, "sleep") as slept:
                    program.save()
                self.assertEqual(moved.call_count, denials + 1)
                self.assertEqual([call.args[0] for call in slept.call_args_list], [0.05, 0.1, 0.2][:denials])
                self.assertEqual(self.fingerprint(target), fingerprints[0])
                self.assertEqual(target.read_bytes(), pipeline.encoded(program.manifest) + b"\n")
                self.assertFalse((program.root / "pipeline-manifest.next").exists())
                self.assertFalse((program.root / PUBLICATION_FAILURE).exists())

    def test_permanent_denial_retains_durable_and_next_and_records_ambiguity(self):
        program = pipeline.Pipeline(self.options())
        mock_cli, actual_replace = MockCli(self), pipeline.os.replace
        denied, blocked, old_bytes, next_fingerprints = self.denied(), 0, None, []
        def intercept(source, destination):
            nonlocal blocked, old_bytes
            pending = pipeline.read_json_file(Path(source))
            if pending["stages"] and pending["stages"][-1]["stage"] == "extract-japanese":
                blocked += 1
                if old_bytes is None:
                    old_bytes = Path(destination).read_bytes()
                self.assertEqual(Path(destination).read_bytes(), old_bytes)
                next_fingerprints.append(self.fingerprint(Path(source)))
                self.assertEqual(next_fingerprints[-1], next_fingerprints[0])
                raise denied
            return actual_replace(source, destination)
        with patch.object(pipeline, "read_database", side_effect=mock_cli.read), patch.object(pipeline.subprocess, "run", side_effect=mock_cli.run), patch.object(pipeline.os, "replace", side_effect=intercept), patch.object(pipeline.time, "sleep") as slept:
            with self.assertRaises(pipeline.PipelineError) as failure:
                program.run()
            self.assertIn("Manifest publication failed", str(failure.exception))
            self.assertIs(failure.exception.__cause__, denied)
            self.assertEqual(blocked, 4)
            self.assertEqual([call.args[0] for call in slept.call_args_list], [0.05, 0.1, 0.2])
            self.assertEqual([args[0] for args in mock_cli.calls], ["inject", "extract"], "No next child after unpersisted extraction result")
        target, next_path = program.root / "pipeline-manifest.json", program.root / "pipeline-manifest.next"
        self.assertEqual(target.read_bytes(), old_bytes)
        self.assertEqual(self.fingerprint(next_path), next_fingerprints[0])
        record_path = program.root / PUBLICATION_FAILURE
        self.assertLessEqual(record_path.stat().st_size, 4096)
        record = pipeline.read_json_file(record_path)
        self.assertEqual(record["owner"], pipeline.OWNER)
        self.assertEqual(record["run_id"], program.manifest["run_id"])
        self.assertEqual(record["status"], "failed")
        self.assertIs(record["resume_safe"], False)
        self.assertEqual(record["attempts"], 4)
        self.assertEqual(record["winerror"], 5)
        self.assertEqual(record["durable_state"]["active_command"], "extract-japanese")
        self.assertEqual(record["durable_state"]["stage_count"], 1)
        self.assertEqual(record["attempted_state"]["last_stage"], "extract-japanese")
        self.assertEqual(record["attempted_state"]["last_exit_code"], 0)
        self.assertEqual(record["attempted_state"]["stage_count"], 2)
        self.assertIsNone(record["attempted_state"]["active_command"])
        self.assertNotIn("private", record_path.read_text())
        self.assertTrue((program.root / "pipeline.lock").exists(), "Publication ambiguity retains the owned run marker")
        with patch.object(pipeline.subprocess, "run") as child:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(self.options(resume=True)).run()
            child.assert_not_called()
        with patch.object(pipeline.os, "replace") as moved:
            with self.assertRaises(pipeline.PipelineError) as repeated:
                program.save()
            self.assertIs(repeated.exception, failure.exception)
            moved.assert_not_called()

    def test_nonpermission_replace_failure_is_not_retried(self):
        program = self.publisher()
        error = OSError(errno.EIO, "synthetic nonpermission failure")
        target = program.root / "pipeline-manifest.json"
        before = target.read_bytes()
        with patch.object(pipeline.os, "replace", side_effect=error) as moved, patch.object(pipeline.time, "sleep") as slept:
            with self.assertRaises(pipeline.PipelineError) as failure:
                program.save()
            self.assertIs(failure.exception.__cause__, error)
            self.assertEqual(moved.call_count, 1)
            slept.assert_not_called()
        self.assertEqual(target.read_bytes(), before)
        self.assertEqual(pipeline.read_json_file(program.root / PUBLICATION_FAILURE)["attempts"], 1)

    def test_retry_refuses_changed_or_foreign_source_and_target(self):
        for member in ("source", "target"):
            for mutation in ("bytes", "replacement"):
                with self.subTest(member=member, mutation=mutation):
                    program = self.publisher(member + "-" + mutation)
                    denied, preserved, victim = self.denied(), None, None
                    def intercept(source, target):
                        nonlocal victim
                        victim = Path(source if member == "source" else target)
                        raise denied
                    def during_delay(_):
                        nonlocal preserved
                        if preserved is not None:
                            return
                        old = victim.read_bytes()
                        if mutation == "replacement":
                            # Retain the original fixture file; never delete it.
                            victim.rename(program.root / "retained-test-original")
                            victim.write_bytes(old)  # Same hash, different inode.
                        else:
                            victim.write_bytes(b"foreign changed bytes")
                        preserved = self.fingerprint(victim)
                    with patch.object(pipeline.os, "replace", side_effect=intercept) as moved, patch.object(pipeline.time, "sleep", side_effect=during_delay), patch.object(pipeline.subprocess, "run") as child:
                        with self.assertRaises(pipeline.PipelineError) as failure:
                            program.save()
                        self.assertIs(failure.exception.__cause__, denied)
                        self.assertEqual(moved.call_count, 1, "Guard must refuse before a second replace")
                        child.assert_not_called()
                    path = program.root / ("pipeline-manifest.next" if member == "source" else "pipeline-manifest.json")
                    self.assertEqual(self.fingerprint(path), preserved, "Foreign file must not be modified")
                    record = pipeline.read_json_file(program.root / PUBLICATION_FAILURE)
                    self.assertEqual(record["reason"], "retry-guard-refused")

    def test_retry_refuses_source_target_reparse_or_multiple_links(self):
        actual_lstat = Path.lstat
        class StatView:
            def __init__(self, info, overrides):
                self.info, self.overrides = info, overrides
            def __getattr__(self, key):
                return self.overrides[key] if key in self.overrides else getattr(self.info, key)
        for member in ("source", "target"):
            for mutation in ("reparse", "multiple-links", "special"):
                with self.subTest(member=member, mutation=mutation):
                    program = self.publisher(member + "-" + mutation)
                    armed, victim, fingerprints = False, None, {}
                    denied = self.denied()
                    def intercept(source, target):
                        nonlocal armed, victim
                        victim = Path(source if member == "source" else target)
                        for path in (Path(source), Path(target)):
                            fingerprints[path] = self.fingerprint(path)
                        armed = True
                        raise denied
                    def lstat(path, *args, **kwargs):
                        info = actual_lstat(path, *args, **kwargs)
                        if armed and path == victim:
                            overrides = {"reparse": {"st_file_attributes": 0x400}, "multiple-links": {"st_nlink": 2},
                                         "special": {"st_mode": 0o010600}}
                            return StatView(info, overrides[mutation])
                        return info
                    with patch.object(pipeline.os, "replace", side_effect=intercept) as moved, patch.object(pipeline.time, "sleep"), patch.object(Path, "lstat", autospec=True, side_effect=lstat):
                        with self.assertRaises(pipeline.PipelineError) as failure:
                            program.save()
                        self.assertIs(failure.exception.__cause__, denied)
                        self.assertEqual(moved.call_count, 1)
                    self.assertEqual(pipeline.read_json_file(program.root / PUBLICATION_FAILURE)["reason"], "retry-guard-refused")
                    for path, fingerprint in fingerprints.items():
                        self.assertEqual(self.fingerprint(path), fingerprint)

    def test_retry_refuses_changed_root_or_parent_identity_without_foreign_writes(self):
        actual_identity = pipeline.directory_identity
        for member in ("root", "parent"):
            with self.subTest(member=member):
                program = self.publisher("changed-" + member)
                target = program.root / "pipeline-manifest.json"
                before, armed = target.read_bytes(), False
                denied = self.denied()
                def intercept(*_):
                    nonlocal armed
                    armed = True
                    raise denied
                def identity(path):
                    result = actual_identity(path)
                    victim = program.root if member == "root" else program.root.parent
                    return (result[0], result[1] + 1) if armed and path == victim else result
                with patch.object(pipeline.os, "replace", side_effect=intercept) as moved, patch.object(pipeline.time, "sleep"), patch.object(pipeline, "directory_identity", side_effect=identity):
                    with self.assertRaises(pipeline.PipelineError) as failure:
                        program.save()
                    self.assertIs(failure.exception.__cause__, denied)
                    self.assertEqual(moved.call_count, 1)
                self.assertEqual(target.read_bytes(), before)
                self.assertTrue((program.root / "pipeline-manifest.next").exists())
                self.assertFalse((program.root / PUBLICATION_FAILURE).exists(), "Do not create a record in a foreign root")

    def test_first_publication_refuses_new_foreign_target_before_retry(self):
        program = self.publisher(durable=False)
        target = program.root / "pipeline-manifest.json"
        denied = self.denied()
        def intercept(*_):
            target.write_bytes(b"foreign target appeared")
            raise denied
        with patch.object(pipeline.os, "replace", side_effect=intercept) as moved, patch.object(pipeline.time, "sleep"):
            with self.assertRaises(pipeline.PipelineError) as failure:
                program.save()
            self.assertIs(failure.exception.__cause__, denied)
            self.assertEqual(moved.call_count, 1)
        self.assertEqual(target.read_bytes(), b"foreign target appeared")

    def test_preoccupied_next_is_never_recreated_or_replaced(self):
        program = self.publisher()
        target, next_path = program.root / "pipeline-manifest.json", program.root / "pipeline-manifest.next"
        before = self.fingerprint(target)
        next_path.write_bytes(b"foreign next retained")
        foreign = self.fingerprint(next_path)
        with patch.object(pipeline.os, "replace") as moved, patch.object(pipeline.time, "sleep") as slept:
            with self.assertRaises(pipeline.PipelineError):
                program.save()
            moved.assert_not_called()
            slept.assert_not_called()
        self.assertEqual(self.fingerprint(target), before)
        self.assertEqual(self.fingerprint(next_path), foreign)
        record = pipeline.read_json_file(program.root / PUBLICATION_FAILURE)
        self.assertEqual(record["attempts"], 0)
        self.assertIsNone(record["expected_next"], "Foreign next is not claimed as operation-owned")

    def test_failure_record_denial_or_occupation_never_masks_original_failure(self):
        actual_open = Path.open
        class DeniedWrite:
            def __init__(self, stream):
                self.stream = stream
            def __enter__(self):
                self.stream.__enter__()
                return self
            def __exit__(self, *args):
                return self.stream.__exit__(*args)
            def write(self, _):
                raise PermissionError(errno.EACCES, "secondary private write denial")
        for fault in ("denied", "write-denied", "occupied"):
            with self.subTest(fault=fault):
                program = self.publisher("record-" + fault)
                record_path = program.root / PUBLICATION_FAILURE
                if fault == "occupied":
                    record_path.write_bytes(b"foreign record retained")
                denied, record_attempts = self.denied(), []
                target_before = self.fingerprint(program.root / "pipeline-manifest.json")
                def opened(path, *args, **kwargs):
                    if path == record_path:
                        record_attempts.append(args[0] if args else kwargs.get("mode"))
                        if fault == "denied":
                            raise PermissionError(errno.EACCES, "secondary private record denial")
                        if fault == "write-denied":
                            return DeniedWrite(actual_open(path, *args, **kwargs))
                    return actual_open(path, *args, **kwargs)
                with patch.object(pipeline.os, "replace", side_effect=denied) as moved, patch.object(pipeline.time, "sleep"), patch.object(Path, "open", autospec=True, side_effect=opened):
                    with self.assertRaises(pipeline.PipelineError) as failure:
                        program.save()
                    self.assertIs(failure.exception.__cause__, denied)
                    self.assertEqual(moved.call_count, 4)
                    self.assertNotIn("secondary", str(failure.exception))
                    self.assertEqual(record_attempts, ["xb"], "One exclusive record attempt, no replacement or retry")
                self.assertEqual(self.fingerprint(program.root / "pipeline-manifest.json"), target_before)
                self.assertTrue((program.root / "pipeline-manifest.next").exists())
                if fault == "occupied":
                    self.assertEqual(record_path.read_bytes(), b"foreign record retained")
                elif fault == "write-denied":
                    self.assertEqual(record_path.read_bytes(), b"", "Retain the operation-owned partial record")
                else:
                    self.assertFalse(record_path.exists())

    def test_failed_error_reporting_publication_preserves_primary_gate_failure(self):
        program, mock_cli = pipeline.Pipeline(self.options()), MockCli(self)
        actual_replace = pipeline.os.replace
        denied, attempts = self.denied(), 0
        def intercept(source, target):
            nonlocal attempts
            if pipeline.read_json_file(Path(source))["status"] == "failed":
                attempts += 1
                raise denied
            return actual_replace(source, target)
        with patch.object(pipeline, "read_database", side_effect=mock_cli.read), patch.object(pipeline.subprocess, "run", side_effect=mock_cli.run), patch.object(pipeline.os, "replace", side_effect=intercept), patch.object(pipeline.time, "sleep"):
            with self.assertRaisesRegex(pipeline.PipelineError, "Missing/blank/pending"):
                program.run()
            self.assertEqual(attempts, 4)
            self.assertEqual([args[0] for args in mock_cli.calls], ["inject", "extract", "translate"])
        self.assertEqual(pipeline.read_json_file(program.root / PUBLICATION_FAILURE)["status"], "failed")


class ScriptedEndpoint:
    def __init__(self, answers, fault=None):
        self.answers, self.fault, self.requests, self.errors = answers, fault, [], []
        endpoint = self

        class Handler(BaseHTTPRequestHandler):
            def setup(self):
                super().setup()
                self.connection.settimeout(2)

            def log_message(self, *_):
                pass

            def do_POST(self):
                try:
                    if self.path != "/v1/chat/completions" or self.headers.get("Authorization") != "Bearer " + pipeline.DUMMY_KEY:
                        raise ValueError("wrong endpoint or key")
                    length = int(self.headers["Content-Length"])
                    if not 0 < length <= 1024 * 1024:
                        raise ValueError("request size")
                    body = json.loads(self.rfile.read(length))
                    if body["model"] != pipeline.MODEL:
                        raise ValueError("wrong model")
                    system = body["messages"][0]["content"]
                    match = re.search(r"from (ja|en) to (en|es)\.", system)
                    if not match or "Preserve all placeholder tokens" not in system:
                        raise ValueError("wrong language/control prompt")
                    source, target = match.groups()
                    records = json.loads(body["messages"][1]["content"])
                    translated = []
                    for record in records:
                        key = (source, target, record["source"])
                        endpoint.requests.append(key)
                        answer = endpoint.answers[key]
                        if (endpoint.fault == "missing" and source == "ja" or endpoint.fault == "partial-both") and "{PL_0}" in key[2]:
                            continue
                        if endpoint.fault == "placeholder" and target == "es" and "{PL_0}" in answer:
                            answer = answer.replace("{PL_0}", "{PL_99}")
                        if endpoint.fault == "kag" and target == "es":
                            answer = answer.replace('exp="f.player"', 'exp="f.other"')
                        translated.append(answer)
                    response = {"choices": [{"message": {"content": json.dumps(translated)}}],
                                "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}}
                    status = 200
                except (ValueError, KeyError, TypeError, OSError) as error:
                    endpoint.errors.append(type(error).__name__)
                    response, status = {"error": "unscripted fixture request"}, 400
                payload = json.dumps(response).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.send_header("Connection", "close")
                self.end_headers()
                self.wfile.write(payload)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.worker = threading.Thread(target=self.server.serve_forever, kwargs={"poll_interval": 0.05})
        self.worker.start()
        self.url = "http://127.0.0.1:" + str(self.server.server_port)

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join(timeout=3)
        if self.worker.is_alive():
            raise AssertionError("fixture endpoint failed to stop")


def fixture(root, engine, mixed_kirikiri=False, layouts=False):
    title = ("錬金術師", "Alchemist", "Alquimista")
    duplicate = ("こんにちは", "Hello", "Hola")
    next_line = ("進む", "Continue", "Continuar")
    back = ("戻る", "Return", "Volver")
    alice = ("Alice", "Alice", "Alice")
    lines = [title, duplicate, next_line, back, alice]
    if engine == "renpy":
        control = ("こんにちは [player] {b}旅人{/b}！", "Hello [player] {b}traveler{/b}!", "Hola [player] {b}viajero{/b}!")
        masked = ("こんにちは {PL_0} {PL_1}旅人{PL_2}！", "Hello {PL_0} {PL_1}traveler{PL_2}!", "Hola {PL_0} {PL_1}viajero{PL_2}!")
        script = '\n'.join([f'define e = Character("{title[0]}")', 'define a = Character("Alice")', 'label start:',
                            '    $ asset = "NON_TEXT_SENTINEL.png"', f'    e "{control[0]}"', f'    e "{duplicate[0]}"',
                            f'    e "{duplicate[0]}"', '    menu:', f'        "{next_line[0]}":', '            jump next',
                            f'        "{back[0]}":', '            return', 'label next:', '    return', ''])
        files = {"game/script.rpy": script.encode()}
    elif engine == "html-game":
        control = ("こんにちは {player} ようこそ旅人！", "Hello {player} welcome traveler!", "Hola {player} bienvenido viajero!")
        masked = tuple(part.replace("{player}", "{PL_0}") for part in control)
        entity = ('星 & 光 © "明るい"', 'Stars & light © "bright"', 'Luz & estrellas © "brillantes"')
        lines.append(entity)
        document = '''<!doctype html>\r\n<html lang="ja"><head><title>錬金術師</title>
<script>const untouched = "日本語のコード &amp;";</script><style>.keep { color: red; }</style></head>
<body data-label="翻訳しない" onclick="keep('日本語')"><!-- 日本語のコメント -->
<p>  こんにちは  </p><button onclick="continueGame()">こんにちは</button>
<input placeholder="進む" aria-label="戻る" title='星 &amp; 光 &#169; &quot;明るい&quot;'>
<p>こんにちは {player} ようこそ旅人！</p><span>Alice</span>
<p>星 &amp; 光 &#169; &quot;明るい&quot;</p>
<img src="data:image/png;base64,AAECAwQ=" data-note="日本語" alt="123">
<template>翻訳しない</template><svg><text>翻訳しない</text></svg>
</body></html>\r\n'''
        files = {"index.html": document.encode()}
    elif engine == "rpgmaker-mv":
        control = (r"\n<錬金術師>こんにちは \V[1]！", r"\n<Alchemist>Hello \V[1]!", r"\n<Alquimista>Hola \V[1]!")
        masked = ("{PL_0}<錬金術師>こんにちは {PL_1}！", "{PL_0}<Alchemist>Hello {PL_1}!", "{PL_0}<Alquimista>Hola {PL_1}!")
        wrapped = ("森へ\n進む", "Continue towards the forest", "Continuar hacia el bosque")
        lines.append(wrapped)
        extra_messages = []
        profile = ""
        if layouts:
            for label, kind, physical, literal, _ in LAYOUT_CASES[:6]:
                semantic = "A different English source for " + label
                if kind == "msg":
                    extra_messages.append(physical)
                else:
                    profile = physical
                # Name-box escape masking is the production provider contract.
                lines.append(tuple(part.replace(r"\n", "{PL_0}") for part in (physical, semantic, literal))
                             if label == "name-spacing" else (physical, semantic, literal))
                if label == "name-spacing":
                    lines[-1] = (lines[-1][0], "{PL_0}<Forest guide>" + semantic, lines[-1][2])
        commands = []
        for message in (control[0], duplicate[0], duplicate[0], wrapped[0], *extra_messages):
            commands.append({"code": 101, "indent": 0, "parameters": ["", 0, 0, 2]})
            for part in message.split("\n"):
                commands.append({"code": 401, "indent": 0, "parameters": [part]})
        commands.extend([{"code": 102, "indent": 0, "parameters": [[next_line[0], back[0]], 1, 0, 2, 0]},
                         {"code": 402, "indent": 0, "parameters": [0, next_line[0]]},
                         {"code": 121, "indent": 1, "parameters": [7, 7, 0]},
                         {"code": 402, "indent": 0, "parameters": [1, back[0]]},
                         {"code": 404, "indent": 0, "parameters": []}, {"code": 0, "indent": 0, "parameters": []}])
        game_map = {"displayName": "", "note": "NON_TEXT_SENTINEL", "width": 1, "height": 1, "data": [17, 23, 42],
                    "events": [None, {"id": 1, "name": "NON_TEXT_SENTINEL", "pages": [{"conditions": {"switch1Id": 7},
                              "image": {"characterName": "NON_TEXT_SENTINEL.png"}, "list": commands}]}]}
        system = {"gameTitle": title[0], "terms": {"basic": [], "commands": []}, "currencyUnit": "G", "switches": [None, "NON_TEXT_SENTINEL"]}
        files = {"data/Map001.json": json.dumps(game_map, ensure_ascii=False).encode(),
                 "data/System.json": json.dumps(system, ensure_ascii=False).encode(),
                 "data/Actors.json": json.dumps([None, {"id": 1, "name": "Alice", "nickname": "", "profile": profile}]).encode()}
    else:
        control = ('こんにちは [emb exp="f.player"][l]', 'Hello [emb exp="f.player"][l]', 'Hola [emb exp="f.player"][l]')
        masked = ('こんにちは [emb exp="f.player"]{PL_0}', 'Hello [emb exp="f.player"]{PL_0}', 'Hola [emb exp="f.player"]{PL_0}')
        next_line = tuple("[link target=*next]" + part + "[endlink]" for part in next_line)
        back = tuple("[link target=*back]" + part + "[endlink]" for part in back)
        lines[2:4] = [next_line, back]
        names = ['@name text="Alice"'] if mixed_kirikiri else []
        if not mixed_kirikiri:
            lines.remove(alice)
        script = '\n'.join(['; NON_TEXT_SENTINEL', '*start', f'@name text="{title[0]}"', *names,
                            control[0], duplicate[0], '[p]', duplicate[0], next_line[0], back[0],
                            '[iscript]', 'f.asset = "NON_TEXT_SENTINEL.png";', '[endscript]', '*next', '[jump target=*back]', '*back', ''])
        files = {"scenario/story.ks": script.encode()}
    answers = {}
    for line in lines:
        masks = tuple(part.replace("[endlink]", "{PL_0}") for part in line) if engine == "kirikiri" else line
        for index, pair in enumerate((("ja", "en"), ("en", "es"))):
            answers[(*pair, masks[index])] = masks[index + 1]
    for index, pair in enumerate((("ja", "en"), ("en", "es"))):
        answers[(*pair, masked[index])] = masked[index + 1]
    files.update({".locust-not-metadata.bin": b"\x00KEEP\xff\x10", "assets/NON_TEXT_SENTINEL.png": b"\x89PNG\r\nfixture\0"})
    for path, content in files.items():
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)
    return answers, files, control


class ProductionTransportTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(CLI.is_file(), "Build the checkout CLI in target/debug with cargo build -p locust-cli before these tests")
        self.temp = tempfile.TemporaryDirectory(prefix="pipeline-transport-", dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()

    def scenario(self, engine, fault=None, mixed_kirikiri=False):
        source = self.root / "source"
        answers, files, control = fixture(source, engine, mixed_kirikiri)
        server = ScriptedEndpoint(answers, fault)
        self.addCleanup(server.close)
        options = pipeline.Options(str(CLI), str(source), str(self.root / "run"), engine, "google",
                                   test_endpoint=server.url, batch_size=1, concurrency=2, timeout=60)
        return options, server, files, control

    def positive(self, engine):
        options, server, files, _ = self.scenario(engine)
        original = pipeline.scan(Path(options.game), source=True)
        actual_run, outputs = subprocess.run, []
        def record(argv, **kwargs):
            result = actual_run(argv, **kwargs)
            outputs.append(result)
            return result
        try:
            with patch.object(pipeline.subprocess, "run", side_effect=record):
                summary = pipeline.Pipeline(options).run()
        except pipeline.PipelineError as error:
            self.fail(str(error) + ": " + ((outputs[-1].stderr + outputs[-1].stdout).decode("utf-8", errors="replace") if outputs else "no child"))
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(pipeline.scan(Path(options.game)), original)
        manifest = pipeline.read_json_file(Path(options.output) / "pipeline-manifest.json")
        self.assertTrue(manifest["rollback_verified"])
        self.assertEqual(manifest["final_install"]["verification"], "strict")
        self.assertEqual(manifest["final_install"]["baseline"], "pristine")
        self.assertEqual(summary["translated_copy"], str(Path(options.output) / "install-game"))
        self.assertEqual(pipeline.file_hash(Path(summary["patch"]))["sha256"], summary["patch_sha256"])
        spanish, _ = pipeline.read_database(Path(options.output) / "spanish.db")
        english, _ = pipeline.read_database(Path(options.output) / "english.db")
        by_id = {value["id"]: value for value in english}
        self.assertEqual(len(spanish), {"renpy": 7, "rpgmaker-mv": 8, "kirikiri": 6}[engine])
        self.assertEqual(summary["resources_verified"], len(spanish))
        for value in spanish:
            self.assertEqual(value["source"], by_id[value["id"]]["translation"])
            self.assertEqual(value["metadata"]["locust_injection_source"], by_id[value["id"]]["source"])
            self.assertEqual(value["provider_used"], pipeline.PROVIDER)
        self.assertTrue(any(value["translation"] == "Alquimista" for value in spanish))
        self.assertEqual(sum(value["translation"] == "Hola" for value in spanish), 2)
        self.assertEqual(manifest["injection"]["skip_reasons"], {} if engine == "kirikiri" else {"unchanged": 1})
        self.assertTrue(manifest["injection"]["retained_originals_verified"])
        self.assertTrue(manifest["injection"]["informational_notice_verified"])
        self.assertEqual(manifest["final_equivalence"]["resources_verified"], len(spanish))
        self.assertEqual(len([stage for stage in manifest["stages"] if stage["stage"] == "final-reapply"]), 1)
        self.assertEqual(Path(summary["translated_copy"]).joinpath(".locust-not-metadata.bin").read_bytes(), files[".locust-not-metadata.bin"])
        self.assertEqual(server.errors, [])
        self.assertEqual(set(server.requests), set(server.answers))
        boundary = next(index for index, key in enumerate(server.requests) if key[0] == "en")
        self.assertTrue(all(key[0] == "ja" for key in server.requests[:boundary]))
        self.assertTrue(all(key[0] == "en" for key in server.requests[boundary:]))
        report_text = Path(options.output, "pipeline-manifest.json").read_text()
        self.assertNotIn(pipeline.DUMMY_KEY, report_text)
        self.assertNotIn("こんにちは", report_text)
        self.assertFalse(summary["runtime_verified"])

    def glossary_options(self, options, entries=None):
        path = self.root / "terms.json"
        policy = entries or {"version": 1, "ja-en": [
            {"term": "錬金術師", "translation": "Alchemist"}, {"term": "旅人", "translation": "traveler"}],
            "en-es": [{"term": "Alchemist", "translation": "Alquimista"}, {"term": "traveler", "translation": "viajero"}]}
        path.write_text(json.dumps(policy, ensure_ascii=False), encoding="utf-8")
        return replace(options, glossary_json=str(path))

    def test_glossary_actual_cli_exact_partial_fallback_and_install(self):
        options, primary, _, _ = self.scenario("renpy", "partial-both")
        fallback = ScriptedEndpoint({})
        self.addCleanup(fallback.close)
        options = self.glossary_options(replace(options, provider="antigravity", fallback_provider="google",
                                               test_fallback_endpoint=fallback.url))
        ja = "こんにちは {PL_0} {PL_1}{G_0_0}{PL_2}！"
        en = "Hello {PL_0} {PL_1}{G_0_0}{PL_2}!"
        es = "Hola {PL_0} {PL_1}{G_0_0}{PL_2}!"
        primary.answers.update({("ja", "en", ja): en, ("en", "es", en): es})
        fallback.answers = dict(primary.answers)
        input_hash = pipeline.file_hash(Path(options.glossary_json))
        original = pipeline.scan(Path(options.game))
        summary = pipeline.Pipeline(options).run()
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(pipeline.file_hash(Path(options.glossary_json)), input_hash)
        self.assertEqual(pipeline.scan(Path(options.game)), original)
        manifest = pipeline.read_json_file(Path(options.output) / "pipeline-manifest.json")
        self.assertEqual(manifest["final_install"]["verification"], "strict")
        self.assertTrue(manifest["rollback_verified"])
        self.assertEqual(primary.errors + fallback.errors, [])
        for database, pair in (("english.db", "ja-en"), ("spanish.db", "en-es")):
            terms = pipeline.read_glossary(Path(options.output) / database)
            self.assertEqual({v[2] for v in terms}, {pair})
            accepted = manifest["accepted_rows"][database]
            self.assertEqual({v["provider"] for v in accepted.values()},
                             {"glossary", pipeline.PROVIDER, pipeline.FALLBACK_TEST_PROVIDER})
        self.assertIn(("ja", "en", ja), fallback.requests)
        self.assertIn(("en", "es", en), fallback.requests)
        self.assertTrue(all("policy_sha256" in stage for stage in manifest["stages"]))

    def test_glossary_seed_all_excluded_and_resume_rederives_empty_subset(self):
        options, primary, seed, before, _, accepted = self.closed_seed()
        policy = {"version": 1, "ja-en": [{"term": "錬金術師", "translation": "Scholar"}],
                  "en-es": [{"term": "Scholar", "translation": "Erudito"}]}
        fresh = self.glossary_options(replace(options, output=str(self.root / "fresh"), english_seed=str(seed)), policy)
        with self.assertRaisesRegex(pipeline.PipelineError, "Missing/blank/pending"):
            pipeline.Pipeline(fresh).run()
        root = Path(fresh.output)
        manifest_path = root / "pipeline-manifest.json"
        manifest = pipeline.read_json_file(manifest_path)
        pipeline.validate_resume_manifest(manifest)
        self.assertTrue(accepted)
        self.assertEqual(manifest["seed"]["targets"], {})
        self.assertFalse(any(s["stage"] == "seed-import-english" for s in manifest["stages"]))
        snapshot, _ = pipeline.read_database(root / "seed-snapshot.db")
        complete = {r["id"]: r for r in snapshot if r["status"] in pipeline.READY}
        pipeline.verify_seed_catalog(root / "seed-english.xliff", complete)
        pipeline.verify_seed_catalog(root / "seed-english-filtered.xliff", {})
        rows, _ = pipeline.read_database(root / "english.db")
        self.assertTrue(any(r["translation"] == "Scholar" and r["provider_used"] == "glossary" for r in rows))
        self.assertFalse(any(r["provider_used"] == "import" for r in rows))
        # Forge a contradictory filtered unit AND its recorded hash. Resume
        # derives the permitted subset independently, before acquiring a lock.
        filtered = root / "seed-english-filtered.xliff"
        original_bytes = filtered.read_bytes()
        filtered.write_bytes((root / "seed-english.xliff").read_bytes())
        forged = copy.deepcopy(manifest)
        forged["seed"]["filtered_catalog"] = pipeline.file_hash(filtered)
        manifest_path.write_bytes(pipeline.encoded(forged))
        retained = pipeline.scan(root)
        with patch.object(pipeline.subprocess, "run") as child:
            with self.assertRaises(pipeline.PipelineError):
                pipeline.Pipeline(replace(fresh, english_seed=None, resume=True)).run()
            child.assert_not_called()
        self.assertEqual(pipeline.scan(root), retained)
        filtered.write_bytes(original_bytes)
        manifest_path.write_bytes(pipeline.encoded(manifest))
        primary.fault = None
        summary = pipeline.Pipeline(replace(fresh, english_seed=None, resume=True)).run()
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(pipeline.file_hash(seed), before)
        final = pipeline.read_json_file(manifest_path)
        self.assertEqual(sum(s["stage"] == "seed-export-english" for s in final["stages"]), 1)
        self.assertFalse(any(s["stage"] == "seed-import-english" for s in final["stages"]))
        self.assertEqual(primary.errors, [])

    def test_glossary_spanish_only_seed_uses_separate_complete_derived_catalog(self):
        options, primary, seed, before, _, accepted = self.closed_seed()
        primary.fault = None
        fresh = self.glossary_options(replace(options, output=str(self.root / "fresh"), english_seed=str(seed)),
                                     {"version": 1, "ja-en": [], "en-es": [{"term": "Alchemist", "translation": "Alquimista"}]})
        actual, imports = subprocess.run, []
        def run(argv, **kwargs):
            if argv[3] == "import":
                imports.append(argv[argv.index("-i") + 1])
            return actual(argv, **kwargs)
        with patch.object(pipeline.subprocess, "run", side_effect=run):
            summary = pipeline.Pipeline(fresh).run()
        self.assertEqual(summary["status"], "completed")
        root = Path(fresh.output)
        self.assertEqual(imports, [str(root / "seed-english-filtered.xliff")])
        manifest = pipeline.read_json_file(root / "pipeline-manifest.json")
        self.assertEqual(set(manifest["seed"]["targets"]), set(accepted))
        self.assertEqual(pipeline.file_hash(seed), before)
        self.assertEqual(primary.errors, [])

    def test_production_cli_layouts_use_immutable_ja_after_semantic_pivot(self):
        source = self.root / "source"
        answers, _, _ = fixture(source, "rpgmaker-mv", layouts=True)
        server = ScriptedEndpoint(answers)
        self.addCleanup(server.close)
        options = pipeline.Options(str(CLI), str(source), str(self.root / "run"), "rpgmaker-mv", "google",
                                   test_endpoint=server.url, batch_size=1, concurrency=2, timeout=60)
        before = pipeline.scan(source)
        summary = pipeline.Pipeline(options).run()
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(pipeline.scan(source), before)
        english, _ = pipeline.read_database(Path(options.output) / "english.db")
        spanish, _ = pipeline.read_database(Path(options.output) / "spanish.db")
        actual, _ = pipeline.read_database(Path(options.output) / "final-install-proof.db")
        wanted = pipeline.resource_slots(spanish, Path(options.output) / "work-game", "rpgmaker-mv")
        found = pipeline.resource_slots(actual, Path(options.output) / "install-game", "rpgmaker-mv")
        for label, kind, physical, literal, written in LAYOUT_CASES[:6]:
            key, entry = next((key, value) for key, value in wanted.items()
                              if value["metadata"]["locust_injection_source"] == physical)
            en = next(value for value in english if value["id"] == entry["id"])
            self.assertEqual(en["source"], physical)
            self.assertEqual(entry["source"], en["translation"])
            self.assertNotEqual(entry["source"], physical)
            self.assertEqual(entry["translation"], literal)
            self.assertEqual(found[key]["source"], written, label)
        self.assertEqual(summary["resources_verified"], 14)
        self.assertEqual(server.errors, [])

    def closed_seed(self, glossary=False):
        options, primary, _, _ = self.scenario("renpy", "missing")
        if glossary:
            options = self.glossary_options(options, {"version": 1, "ja-en": [
                {"term": "錬金術師", "translation": "Alchemist"}], "en-es": []})
        with self.assertRaises(pipeline.PipelineError):
            pipeline.Pipeline(options).run()
        seed = Path(options.output) / "english.db"
        before = pipeline.file_hash(seed)
        seed_rows, _ = pipeline.read_database(seed)
        accepted = {r["id"]: r["translation"] for r in seed_rows if r["status"] == "translated"}
        # Fixture owner closes its WAL reader state through its ordinary CLI;
        # the pipeline under test must never open/migrate this input afterward.
        closed = subprocess.run([str(CLI), "--config", str(Path(options.output) / "profile/config.json"),
                                 "export", str(seed), "-f", "xliff", "-l", "en", "-o", str(self.root / "owner-close.xliff")],
                                env=pipeline.child_environment(Path(options.output), options, subscription_login=False),
                                cwd=Path(options.output), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
        self.assertEqual(closed.returncode, 0)
        self.assertEqual(pipeline.file_hash(seed), before)
        return options, primary, seed, before, seed_rows, accepted

    def test_glossary_seed_owner_exact_hit_provenance_survives_borrow_and_resume(self):
        options, primary, seed, before, _, accepted = self.closed_seed(glossary=True)
        # A new run without active terms can borrow the historical canonical
        # targets; its original phase policy is retained as provenance, not
        # current matching or config authority.
        fresh = replace(options, output=str(self.root / "fresh"), glossary_json=None, english_seed=str(seed))
        with self.assertRaisesRegex(pipeline.PipelineError, "Missing/blank/pending"):
            pipeline.Pipeline(fresh).run()
        root = Path(fresh.output)
        manifest = pipeline.read_json_file(root / "pipeline-manifest.json")
        pipeline.validate_resume_manifest(manifest)
        self.assertIn("source_glossary", manifest["seed"])
        self.assertEqual(set(manifest["seed"]["targets"]), set(accepted))
        self.assertIn("glossary", {r["provider"] for r in manifest["seed"]["targets"].values()})
        primary.fault = None
        summary = pipeline.Pipeline(replace(fresh, english_seed=None, resume=True)).run()
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(pipeline.file_hash(seed), before)
        self.assertEqual(primary.errors, [])

    def test_closed_partial_seed_actual_export_import_and_fallback_install(self):
        options, primary, seed, before, seed_rows, accepted = self.closed_seed()
        primary.fault = "partial-both"
        fallback = ScriptedEndpoint(primary.answers)
        self.addCleanup(fallback.close)
        fresh = replace(options, output=str(self.root / "fresh"), provider="antigravity", english_seed=str(seed),
                        fallback_provider="google", test_fallback_endpoint=fallback.url)
        actual = subprocess.run
        calls = []
        def run(argv, **kwargs):
            calls.append(argv[3:])
            self.assertNotIn(str(seed), argv, "Never migrate/export the owner's seed")
            return actual(argv, **kwargs)
        with patch.object(pipeline.subprocess, "run", side_effect=run):
            summary = pipeline.Pipeline(fresh).run()
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(pipeline.file_hash(seed), before)
        english, _ = pipeline.read_database(Path(fresh.output) / "english.db")
        for r in english:
            if r["id"] in accepted:
                self.assertEqual((r["translation"], r["provider_used"]), (accepted[r["id"]], "import"))
        self.assertTrue(any(c[0] == "export" and "xliff" in c for c in calls))
        self.assertTrue(any(c[0] == "import" and "--keep-existing" in c for c in calls))
        self.assertTrue(fallback.requests)
        self.assertTrue(all("{PL_0}" in key[2] for key in fallback.requests if key[0] == "ja"))
        self.assertFalse((Path(options.output) / "spanish.db").exists())
        self.assertEqual(primary.errors + fallback.errors, [])
        manifest = pipeline.read_json_file(Path(fresh.output) / "pipeline-manifest.json")
        self.assertEqual(manifest["seed"]["targets"].keys(), accepted.keys())
        self.assertEqual(manifest["final_install"]["verification"], "strict")
        self.assertEqual(manifest["injection"]["strings_written"] + manifest["injection"]["strings_skipped"], len(english))
        self.assertEqual({r["provider_used"] for r in english}, {"import", pipeline.FALLBACK_TEST_PROVIDER})
        self.assertEqual({v["provider"] for v in manifest["accepted_rows"]["spanish.db"].values()},
                         {pipeline.PROVIDER, pipeline.FALLBACK_TEST_PROVIDER})

    def test_seed_owner_links_sidecars_remnants_unknown_and_unsafe_states(self):
        options, _, seed, before, _, _ = self.closed_seed()
        owner_path = seed.parent / "pipeline-manifest.json"
        original = pipeline.read_json_file(owner_path)
        source = pipeline.scan(Path(options.game), source=True)
        destination = self.root / "fresh"
        mutations = {
            "unknown-owner": lambda m: m.update(owner="unknown"),
            "active-child": lambda m: m.update(active_command="ja-en"),
            "uncertain-child": lambda m: m.update(uncertain_child=True),
            "injection": lambda m: m.update(injection_started=True),
            "wrong-phase": lambda m: m.update(phase="spanish-ready"),
            "wrong-format": lambda m: m["settings"].update(format="kirikiri"),
            "wrong-source": lambda m: next(iter(m["source"]["files"].values())).update(sha256="0" * 64),
            "wrong-root": lambda m: m["settings"].update(output=str(self.root / "foreign")),
            "malformed-provenance": lambda m: m["stages"][-1].update(stage="unknown"),
            "nonempty-wal-history": lambda m: m["databases"]["english.db"]["files"].update({"-wal": {"sha256": "0" * 64, "size": 10}}),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                changed = copy.deepcopy(original)
                mutate(changed)
                owner_path.write_bytes(pipeline.encoded(changed))
                with self.assertRaises(pipeline.PipelineError):
                    pipeline.seed_owner(seed, source, "renpy", destination, Path(options.game))
        owner_path.write_bytes(pipeline.encoded(original))
        for name in ("english.db-wal", "english.db-shm", "english.db-journal", "pipeline.lock",
                     "pipeline-manifest.next", pipeline.PUBLICATION_FAILURE, "spanish.db", "spanish-patch.zip"):
            with self.subTest(name=name):
                remnant = seed.parent / name
                remnant.write_bytes(b"synthetic remnant")
                with self.assertRaises(pipeline.PipelineError):
                    pipeline.seed_owner(seed, source, "renpy", destination, Path(options.game))
                remnant.rename(self.root / ("retained-" + name))
        alias = self.root / "seed-link"
        alias.symlink_to(seed.parent, target_is_directory=True)
        with self.assertRaises(pipeline.PipelineError):
            pipeline.seed_owner(alias / "english.db", source, "renpy", destination, Path(options.game))
        hard = self.root / "seed-hardlink"
        os.link(seed, hard)
        with self.assertRaises(pipeline.PipelineError):
            pipeline.seed_owner(seed, source, "renpy", destination, Path(options.game))
        # End this fixture here: retaining the hardlink is deliberate negative
        # evidence, not something the production pipeline may unlink.
        self.assertEqual(pipeline.file_hash(seed), before)

    def test_seed_independent_full_inventory_and_ready_evidence(self):
        options, primary, seed, before, seed_rows, _ = self.closed_seed()
        owner_path = seed.parent / "pipeline-manifest.json"
        original_owner = pipeline.read_json_file(owner_path)
        actual_read, actual_run = pipeline.read_database, subprocess.run
        variants = ("foreign-file", "foreign-id", "source", "context", "metadata", "stale", "format", "warning",
                    "root", "pivot", "blank-ready", "provider", "shrink", "created")
        for fault in variants:
            with self.subTest(fault=fault):
                changed = copy.deepcopy(seed_rows)
                metadata = {"extraction_warnings": [], "game_root": str(seed.parent / "work-game")}
                ready = next(r for r in changed if r["status"] == "translated")
                if fault == "foreign-file": ready["file_path"] = str(seed.parent / "other/game/script.rpy")
                elif fault == "foreign-id": ready["id"] += "foreign"
                elif fault == "source": ready["source"] = "違う"
                elif fault == "context": ready["context"] = "different"
                elif fault == "metadata": ready["metadata"]["unknown"] = True
                elif fault == "stale": ready["metadata"]["locust_stale_translation"] = True
                elif fault == "format": ready["metadata"]["format_id"] = "kirikiri"
                elif fault == "warning": metadata["extraction_warnings"] = ["partial"]
                elif fault == "root": metadata["game_root"] = str(self.root / "foreign")
                elif fault == "pivot": ready["metadata"]["locust_injection_source"] = ready["source"]
                elif fault == "blank-ready": ready["translation"] = " "
                elif fault == "provider": ready["provider_used"] = "import"
                elif fault == "shrink": changed.pop()
                elif fault == "created": ready["created_at"] = None
                owner = copy.deepcopy(original_owner)
                # Coordinated data forgery cannot authorize another source or
                # defeat comparison against a NEW actual CLI extraction.
                owner["databases"]["english.db"]["logical_sha256"] = pipeline.digest([changed, metadata])
                owner_path.write_bytes(pipeline.encoded(owner))
                fresh = replace(options, output=str(self.root / ("fresh-" + fault)), english_seed=str(seed))
                reads, calls = [], []
                def read(path):
                    reads.append(Path(path))
                    if Path(path).name == "seed-snapshot.db":
                        return copy.deepcopy(changed), copy.deepcopy(metadata)
                    return actual_read(path)
                def run(argv, **kwargs):
                    calls.append(argv[3:])
                    return actual_run(argv, **kwargs)
                with patch.object(pipeline, "read_database", side_effect=read), patch.object(pipeline.subprocess, "run", side_effect=run):
                    with self.assertRaises(pipeline.PipelineError):
                        pipeline.Pipeline(fresh).run()
                self.assertNotIn(seed, reads)
                self.assertFalse(any(c[0] in {"export", "import", "translate", "inject"} and "--help" not in c for c in calls))
                self.assertEqual(pipeline.file_hash(seed), before)
        owner_path.write_bytes(pipeline.encoded(original_owner))
        self.assertEqual(primary.errors, [])

    def test_seed_run_languages_memory_and_recordings_are_checked_on_copy(self):
        options, _, seed, before, _, _ = self.closed_seed()
        cases = {
            "foreign-language": "UPDATE translation_runs SET source_lang='en', target_lang='es'",
            "unknown-provider": "UPDATE translation_runs SET provider='unknown'",
            "missing-runs": "DELETE FROM translation_runs",
            "malformed-count": "UPDATE translation_runs SET strings_translated=-1",
            "recorded": "INSERT INTO injected_files(lang,root,rel,hash,size,recorded_at) VALUES ('en','foreign','game/script.rpy','bad',1,'date')",
            "memory": "INSERT INTO translation_memory(source_hash,lang_pair,source,translation,last_used) VALUES ('x','ja-en','synthetic','synthetic','date')",
        }
        for name, sql in cases.items():
            with self.subTest(name=name):
                snapshot = self.root / (name + ".db")
                with snapshot.open("xb") as stream:
                    stream.write(seed.read_bytes())
                connection = sqlite3.connect(snapshot)
                try:
                    connection.execute(sql)
                    connection.commit()
                finally:
                    connection.close()
                with self.assertRaises(pipeline.PipelineError):
                    if name == "memory":
                        pipeline.read_database(snapshot)
                    else:
                        pipeline.seed_database_evidence(snapshot, {pipeline.PROVIDER})
        self.assertEqual(pipeline.file_hash(seed), before)

    def test_seed_manifest_is_not_command_config_or_binary_authority(self):
        options, primary, seed, before, _, _ = self.closed_seed()
        manifest_path = seed.parent / "pipeline-manifest.json"
        manifest = pipeline.read_json_file(manifest_path)
        # Version 1 may be borrowed as closed data, but is never resumable by
        # the version 2 orchestrator. Its command authority is not adopted.
        manifest["version"] = 1
        manifest.pop("accepted_rows")
        for key in ("english_seed", "fallback_provider", "test_fallback_endpoint"):
            manifest["settings"].pop(key)
        manifest["settings"].update(cli=str(self.root / "untrusted.exe"), game=str(self.root / "foreign-game"),
                                    test_endpoint="https://untrusted.example", timeout=1)
        manifest["binary"]["sha256"] = "0" * 64
        manifest["tool_sha256"] = "0" * 64
        manifest["config"]["sha256"] = "0" * 64
        manifest_path.write_bytes(pipeline.encoded(manifest))
        (seed.parent / "profile/config.json").write_bytes(b"untrusted config must not be read")
        primary.fault = None
        fresh = replace(options, output=str(self.root / "fresh-untrusted-data"), english_seed=str(seed))
        actual = subprocess.run
        def run(argv, **kwargs):
            self.assertEqual(argv[:3], [str(CLI), "--config", str(Path(fresh.output) / "profile/config.json")])
            self.assertNotIn(str(seed), argv)
            return actual(argv, **kwargs)
        with patch.object(pipeline.subprocess, "run", side_effect=run):
            self.assertEqual(pipeline.Pipeline(fresh).run()["status"], "completed")
        self.assertEqual(pipeline.file_hash(seed), before)

    def test_seed_resume_uses_verified_owned_artifacts_without_seed_flag(self):
        options, primary, seed, before, _, _ = self.closed_seed()
        fresh = replace(options, output=str(self.root / "seeded-pending"), english_seed=str(seed))
        with self.assertRaises(pipeline.PipelineError):
            pipeline.Pipeline(fresh).run()
        manifest = pipeline.read_json_file(Path(fresh.output) / "pipeline-manifest.json")
        pipeline.validate_resume_manifest(manifest)
        with self.assertRaises(pipeline.PipelineError):
            pipeline.Pipeline(replace(fresh, resume=True)).run()  # flag is fresh-only
        primary.fault = None
        actual = subprocess.run
        calls = []
        def run(argv, **kwargs):
            calls.append(argv[3:])
            return actual(argv, **kwargs)
        with patch.object(pipeline.subprocess, "run", side_effect=run):
            summary = pipeline.Pipeline(replace(fresh, english_seed=None, resume=True)).run()
        self.assertEqual(summary["status"], "completed")
        self.assertFalse(any(c[0] in {"export", "import"} for c in calls))
        self.assertEqual(pipeline.file_hash(seed), before)

    def test_damaged_catalog_and_ready_controls_cannot_seed_or_advance(self):
        options, primary, seed, before, _, _ = self.closed_seed()
        actual = subprocess.run
        for fault in ("language", "source", "target", "incomplete", "unknown-control"):
            with self.subTest(fault=fault):
                fresh = replace(options, output=str(self.root / ("bad-cat-" + fault)), english_seed=str(seed))
                imported = False
                def run(argv, **kwargs):
                    nonlocal imported
                    result = actual(argv, **kwargs)
                    if argv[3] == "export":
                        path = Path(argv[argv.index("-o") + 1])
                        text = path.read_text(encoding="utf-8")
                        if fault == "language": text = text.replace('target-language="en"', 'target-language="es"')
                        elif fault == "source": text = text.replace("こんにちは", "違う")
                        elif fault == "target": text = text.replace("Hello", "Forged")
                        elif fault == "incomplete": text = re.sub(r"<trans-unit.*?</trans-unit>", "", text, count=1, flags=re.S)
                        if fault != "unknown-control": path.write_text(text, encoding="utf-8")
                    if argv[3] == "import": imported = True
                    if fault == "unknown-control" and argv[3] == "validate":
                        report = {"count": 1, "issues": [{"entry_id": "unknown", "kind": "UnknownControl", "message": "unsafe"}]}
                        return subprocess.CompletedProcess(argv, 1, json.dumps(report).encode(), b"")
                    return result
                with patch.object(pipeline.subprocess, "run", side_effect=run):
                    with self.assertRaises(pipeline.PipelineError):
                        pipeline.Pipeline(fresh).run()
                self.assertEqual(imported, fault == "unknown-control")
                manifest = pipeline.read_json_file(Path(fresh.output) / "pipeline-manifest.json")
                self.assertFalse(manifest["injection_started"])
                self.assertFalse((Path(fresh.output) / "spanish.db").exists())
                self.assertEqual(pipeline.file_hash(seed), before)
        self.assertEqual(primary.errors, [])

    def test_invalid_pending_seed_target_is_ignored_but_damaged_ready_controls_refuse(self):
        options, primary, seed, before, seed_rows, _ = self.closed_seed()
        owner_path = seed.parent / "pipeline-manifest.json"
        owner_original = pipeline.read_json_file(owner_path)
        actual_read, actual_run = pipeline.read_database, subprocess.run
        for ready_damage in (False, True):
            with self.subTest(ready_damage=ready_damage):
                changed = copy.deepcopy(seed_rows)
                chosen = next(r for r in changed if (r["status"] in pipeline.READY) == ready_damage)
                chosen["translation"] = "Invalid [unknown]"
                metadata = {"extraction_warnings": [], "game_root": str(seed.parent / "work-game")}
                owner = copy.deepcopy(owner_original)
                owner["databases"]["english.db"]["logical_sha256"] = pipeline.digest([changed, metadata])
                if ready_damage:
                    owner["accepted_rows"]["english.db"][chosen["id"]]["row_sha256"] = pipeline.digest(chosen)
                owner_path.write_bytes(pipeline.encoded(owner))
                fresh = replace(options, output=str(self.root / ("target-ready" if ready_damage else "target-pending")), english_seed=str(seed))
                def read(path):
                    if Path(path).name == "seed-snapshot.db":
                        return copy.deepcopy(changed), copy.deepcopy(metadata)
                    return actual_read(path)
                def run(argv, **kwargs):
                    result = actual_run(argv, **kwargs)
                    if ready_damage and argv[3] == "export":
                        path = Path(argv[argv.index("-o") + 1])
                        text = path.read_text(encoding="utf-8")
                        # Forge a fully matching catalog + old snapshot. The
                        # independent production validator must still refuse
                        # newly invented controls after import into the NEW DB.
                        escaped_id = chosen["id"].replace("&", "&amp;").replace('"', "&quot;")
                        pattern = r'(<trans-unit id="' + re.escape(escaped_id) + r'">.*?<target>).*?(</target>)'
                        text, count = re.subn(pattern, r'\g<1>Invalid [unknown]\g<2>', text, flags=re.S)
                        self.assertEqual(count, 1)
                        path.write_text(text, encoding="utf-8")
                    return result
                primary.fault = None
                with patch.object(pipeline, "read_database", side_effect=read), patch.object(pipeline.subprocess, "run", side_effect=run):
                    if ready_damage:
                        with self.assertRaisesRegex(pipeline.PipelineError, "Validation issue cannot release"):
                            pipeline.Pipeline(fresh).run()
                        self.assertFalse((Path(fresh.output) / "spanish.db").exists())
                    else:
                        self.assertEqual(pipeline.Pipeline(fresh).run()["status"], "completed")
                        english, _ = actual_read(Path(fresh.output) / "english.db")
                        pending = next(r for r in english if r["id"] == chosen["id"])
                        self.assertEqual(pending["provider_used"], pipeline.PROVIDER)
                        self.assertNotEqual(pending["translation"], "Invalid [unknown]")
                self.assertEqual(pipeline.file_hash(seed), before)
        owner_path.write_bytes(pipeline.encoded(owner_original))

    def test_seed_input_changed_after_snapshot_never_imports(self):
        options, _, seed, before, _, _ = self.closed_seed()
        actual_hash, actual_run = pipeline.file_hash, subprocess.run
        changed = False
        def hashed(path):
            value = actual_hash(path)
            return dict(value, sha256="0" * 64) if changed and Path(path) == seed else value
        def run(argv, **kwargs):
            nonlocal changed
            result = actual_run(argv, **kwargs)
            if argv[3] == "export": changed = True
            self.assertNotEqual(argv[3], "import")
            return result
        fresh = replace(options, output=str(self.root / "changed-input"), english_seed=str(seed))
        with patch.object(pipeline, "file_hash", side_effect=hashed), patch.object(pipeline.subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(pipeline.PipelineError, "bytes changed"):
                pipeline.Pipeline(fresh).run()
        self.assertEqual(pipeline.file_hash(seed), before)

    def html_public(self, options):
        argv = []
        for name, value in vars(options).items():
            if value is not None and value is not False:
                argv.append("--" + name.replace("_", "-"))
                if value is not True:
                    argv.append(str(value))
        stdout, stderr = StringIO(), StringIO()
        with patch("sys.stdout", stdout), patch("sys.stderr", stderr):
            code = pipeline.main(argv)
        return code, json.loads(stdout.getvalue()), stderr.getvalue()

    def test_html_public_lifecycle_exact_bytes_and_ordinals(self):
        options, server, files, _ = self.scenario("html-game")
        code, result, stderr = self.html_public(options)
        self.assertEqual((code, result["status"], stderr), (0, "completed", ""), result)
        output = Path(options.output)
        manifest = pipeline.read_json_file(output / "pipeline-manifest.json")
        self.assertTrue(manifest["rollback_verified"])
        self.assertEqual(manifest["final_install"]["verification"], "strict")
        self.assertEqual(manifest["final_install"]["baseline"], "pristine")
        self.assertEqual(sum(s["stage"] == "final-reapply" for s in manifest["stages"]), 1)
        english, _ = pipeline.read_database(output / "english.db")
        spanish, _ = pipeline.read_database(output / "spanish.db")
        final, _ = pipeline.read_database(output / "final-install-proof.db")
        self.assertEqual(len(spanish), 9)
        self.assertEqual(result["resources_verified"], 9)
        self.assertEqual(sum(r["translation"] == "Hola" for r in spanish), 2)
        self.assertNotEqual([r["metadata"]["html_start"] for r in spanish], [r["metadata"]["html_start"] for r in final])
        self.assertEqual({r["metadata"]["html_kind"] for r in spanish},
                         {"text:title", "text:p", "text:button", "text:span", "attr:placeholder", "attr:aria-label", "attr:title"})
        en = {r["id"]: r for r in english}
        for r in spanish:
            self.assertEqual(r["source"], en[r["id"]]["translation"])
            self.assertEqual(r["metadata"]["locust_injection_source"], en[r["id"]]["source"])
        expected = files["index.html"].decode().replace("錬金術師", "Alquimista").replace("こんにちは {player} ようこそ旅人！", "Hola {player} bienvenido viajero!")
        expected = expected.replace("こんにちは", "Hola").replace('placeholder="進む"', 'placeholder="Continuar"').replace('aria-label="戻る"', 'aria-label="Volver"')
        # Attribute quotes are encoded; the text slot retains literal quotes.
        expected = expected.replace('星 &amp; 光 &#169; &quot;明るい&quot;', 'Luz &amp; estrellas © &quot;brillantes&quot;', 1)
        expected = expected.replace('星 &amp; 光 &#169; &quot;明るい&quot;', 'Luz &amp; estrellas © "brillantes"')
        for copy_name in ("work-game", "install-game"):
            self.assertEqual((output / copy_name / "index.html").read_bytes(), expected.encode())
            for path in ("assets/NON_TEXT_SENTINEL.png", ".locust-not-metadata.bin"):
                self.assertEqual((output / copy_name / path).read_bytes(), files[path])
        self.assertEqual(pipeline.scan(Path(options.game)), pipeline.scan(output / "pristine"))
        self.assertEqual(server.errors, [])
        self.assertFalse(result["runtime_verified"])

    def test_html_glossary_pending_fallback_and_resume(self):
        options, primary, _, _ = self.scenario("html-game", "partial-both")
        fallback = ScriptedEndpoint({})
        self.addCleanup(fallback.close)
        options = self.glossary_options(replace(options, provider="antigravity", fallback_provider="google",
                                               test_fallback_endpoint=fallback.url))
        ja = "こんにちは {PL_0} ようこそ{G_0_0}！"
        en = "Hello {PL_0} welcome {G_0_0}!"
        es = "Hola {PL_0} bienvenido {G_0_0}!"
        primary.answers.update({("ja", "en", ja): en, ("en", "es", en): es})
        fallback.answers = dict(primary.answers)
        # Orderly primary+fallback omission retains a complete, resumable JA DB.
        fallback.fault = "partial-both"
        code, result, stderr = self.html_public(options)
        self.assertEqual((code, result, stderr), (1, {"status": "failed", "diagnostic": "Missing/blank/pending translation"}, ""))
        output = Path(options.output)
        pending = pipeline.read_json_file(output / "pipeline-manifest.json")
        self.assertEqual(pending["phase"], "english-pending")
        self.assertFalse(pending["injection_started"])
        original = pipeline.scan(Path(options.game))
        fallback.fault = None
        before = len(primary.requests)
        code, result, stderr = self.html_public(replace(options, resume=True))
        self.assertEqual((code, result["status"], stderr), (0, "completed", ""), result)
        self.assertTrue(all("{PL_0}" in key[2] for key in primary.requests[before:] if key[0] == "ja"))
        self.assertEqual(primary.errors + fallback.errors, [])
        self.assertEqual(pipeline.scan(Path(options.game)), original)
        manifest = pipeline.read_json_file(output / "pipeline-manifest.json")
        self.assertTrue(manifest["rollback_verified"])
        for database in ("english.db", "spanish.db"):
            self.assertEqual({proof["provider"] for proof in manifest["accepted_rows"][database].values()},
                             {"glossary", pipeline.PROVIDER, pipeline.FALLBACK_TEST_PROVIDER})
        self.assertEqual({v[2] for v in pipeline.read_glossary(output / "english.db")}, {"ja-en"})
        self.assertEqual({v[2] for v in pipeline.read_glossary(output / "spanish.db")}, {"en-es"})

    def test_html_spanish_control_refusal_and_pending_resume_keep_physical_ja(self):
        options, server, _, _ = self.scenario("html-game", "placeholder")
        original = pipeline.scan(Path(options.game))
        code, result, stderr = self.html_public(options)
        self.assertEqual((code, result, stderr), (1, {"status": "failed", "diagnostic": "Missing/blank/pending translation"}, ""))
        output = Path(options.output)
        manifest = pipeline.read_json_file(output / "pipeline-manifest.json")
        self.assertEqual(manifest["phase"], "spanish-pending")
        self.assertFalse(manifest["injection_started"])
        self.assertFalse((output / "spanish-patch.zip").exists())
        for name in ("pristine", "work-game", "install-game"):
            self.assertEqual(pipeline.scan(output / name), original)
        english = pipeline.read_database(output / "english.db")
        accepted = manifest["accepted_rows"]["english.db"]
        server.fault = None
        code, result, stderr = self.html_public(replace(options, resume=True))
        self.assertEqual((code, result["status"], stderr), (0, "completed", ""), result)
        self.assertEqual(pipeline.read_database(output / "english.db"), english)
        final = pipeline.read_json_file(output / "pipeline-manifest.json")
        self.assertEqual(final["accepted_rows"]["english.db"], accepted)
        self.assertEqual(sum(s["stage"] == "strict-pivot" for s in final["stages"]), 1)
        self.assertEqual(pipeline.scan(Path(options.game)), original)
        self.assertEqual(server.errors, [])

    def test_html_reextraction_rejects_missing_swapped_kind_and_wrong_target(self):
        base = self.root
        read = pipeline.read_database
        for fault, diagnostic in (("missing", "Reextraction resource identities differ"),
                                  ("duplicate", "Duplicate resource identity"),
                                  ("kind", "Reextraction resource identities differ"),
                                  ("target", "HTML physical source/range differs")):
            with self.subTest(fault=fault):
                self.root = base / fault
                self.root.mkdir()
                options, _, _, _ = self.scenario("html-game")
                output = Path(options.output)
                def forged(path):
                    rows, metadata = read(path)
                    if path.name == "work-proof.db":
                        rows = copy.deepcopy(rows)
                        if fault == "missing":
                            rows.pop()
                        elif fault == "duplicate":
                            rows.append(rows[0])
                        elif fault == "kind":
                            r = next(r for r in rows if r["metadata"]["html_kind"] == "text:p")
                            r["metadata"]["html_kind"] = "attr:title"
                            r["tags"] = ["html-attr"]
                            r["context"] = "HTML attr:title (preserve variables; adjacent inline markup is separate)"
                        else:
                            rows[0]["source"] += "wrong"
                    return rows, metadata
                with patch.object(pipeline, "read_database", side_effect=forged):
                    code, result, stderr = self.html_public(options)
                self.assertEqual((code, result, stderr), (1, {"status": "failed", "diagnostic": diagnostic}, ""))
                self.assertFalse((output / "spanish-patch.zip").exists())
                self.assertEqual(pipeline.scan(Path(options.game)), pipeline.scan(output / "pristine"))
                self.assertEqual(pipeline.scan(output / "install-game"), pipeline.scan(output / "pristine"))
        self.root = base

    def test_html_root_dependent_seed_ids_remain_conservatively_refused(self):
        options, _, _, _ = self.scenario("html-game", "missing")
        self.assertEqual(self.html_public(options)[0], 1)
        output = Path(options.output)
        seed = output / "english.db"
        closed = subprocess.run([str(CLI), "--config", str(output / "profile/config.json"),
                                 "export", str(seed), "-f", "xliff", "-l", "en", "-o", str(self.root / "owner-close.xliff")],
                                env=pipeline.child_environment(output, options, subscription_login=False),
                                cwd=output, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
        self.assertEqual(closed.returncode, 0)
        before, original = pipeline.scan(output), pipeline.scan(Path(options.game))
        fresh = replace(options, output=str(self.root / "fresh"), english_seed=str(seed))
        code, result, stderr = self.html_public(fresh)
        self.assertEqual((code, result, stderr), (1, {"status": "failed", "diagnostic": "Malformed HTML locator/kind provenance"}, ""))
        manifest = pipeline.read_json_file(Path(fresh.output) / "pipeline-manifest.json")
        self.assertFalse(any(s["stage"].startswith("seed-") for s in manifest["stages"]))
        self.assertFalse(manifest["injection_started"])
        self.assertFalse(Path(fresh.output, "spanish-patch.zip").exists())
        self.assertEqual(pipeline.scan(output), before)
        self.assertEqual(pipeline.scan(Path(options.game)), original)

    def test_html_coordinated_locator_forgery_refuses_before_any_write_or_child(self):
        options, server, _, _ = self.scenario("html-game", "missing")
        code, _, stderr = self.html_public(options)
        self.assertEqual((code, stderr), (1, ""))
        output = Path(options.output)
        rows, _ = pipeline.read_database(output / "english.db")
        pending = next(r for r in rows if r["status"] == "pending")
        pending["metadata"]["html_raw"] = "x" * len(pending["metadata"]["html_raw"].encode())
        connection = sqlite3.connect(output / "english.db")
        try:
            connection.execute("UPDATE strings SET metadata=? WHERE id=?", (json.dumps(pending["metadata"]), pending["id"]))
            connection.commit()
        finally:
            connection.close()
        manifest_path = output / "pipeline-manifest.json"
        manifest = pipeline.read_json_file(manifest_path)
        session = pipeline.Pipeline(replace(options, resume=True))
        manifest["databases"] = session.checkpoints()
        key = pipeline.encoded([pipeline.relative(pending["file_path"], session.work), pending["id"]]).decode()
        manifest["japanese_inventory"][key] = pipeline.digest({field: pending[field] for field in pipeline.STATIC_FIELDS})
        manifest_path.write_bytes(pipeline.encoded(manifest) + b"\n")
        before = pipeline.scan(output)
        original = pipeline.scan(Path(options.game))
        with patch.object(pipeline.subprocess, "run") as child:
            code, result, stderr = self.html_public(replace(options, resume=True))
        self.assertEqual((code, result, stderr), (1, {"status": "failed", "diagnostic": "HTML physical source/range differs"}, ""))
        child.assert_not_called()
        self.assertEqual(pipeline.scan(output), before)
        self.assertEqual(pipeline.scan(Path(options.game)), original)

    def test_html_resume_rejects_copy_drift_without_mutation(self):
        options, _, _, _ = self.scenario("html-game", "missing")
        self.assertEqual(self.html_public(options)[0], 1)
        output = Path(options.output)
        path = output / "work-game/index.html"
        path.write_bytes(path.read_bytes().replace(b"<!--", b"<!-- drift ", 1))
        before, original = pipeline.scan(output), pipeline.scan(Path(options.game))
        with patch.object(pipeline.subprocess, "run") as child:
            code, result, stderr = self.html_public(replace(options, resume=True))
        self.assertEqual((code, result, stderr), (1, {"status": "failed", "diagnostic": "Pre-injection copy drift"}, ""))
        child.assert_not_called()
        self.assertEqual(pipeline.scan(output), before)
        self.assertEqual(pipeline.scan(Path(options.game)), original)

    def test_html_full_byte_oracle_rejects_recorded_nontext_corruption(self):
        base = self.root
        real_run = subprocess.run
        for label, before, after in (
            ("script", b'const untouched', b'const changedXX'),
            ("handler", b'continueGame()', b'differentRun()'),
            ("comment", "日本語のコメント".encode(), b'changed comment'),
            ("attribute", 'data-label="翻訳しない"'.encode(), b'data-label="changed"'),
            ("embedded", b'base64,AAECAwQ=', b'base64,BAECAwQ='),
            ("binary", b'KEEP', b'LOST'),
            ("gap-whitespace", b'<p>  Hola  </p>', b'<p> Hola  </p>'),
            ("target", b'Hola {player}', b'Adios {player}'),
        ):
            with self.subTest(label=label):
                self.root = base / label
                self.root.mkdir()
                options, _, _, _ = self.scenario("html-game")
                output = Path(options.output)
                def run(argv, **kwargs):
                    result = real_run(argv, **kwargs)
                    if argv[3] == "inject" and "--help" not in argv:
                        work = output / "work-game"
                        target = work / (".locust-not-metadata.bin" if label == "binary" else "index.html")
                        data = target.read_bytes()
                        self.assertIn(before, data)
                        target.write_bytes(data.replace(before, after, 1))
                        # Model a faulty writer whose transaction truthfully
                        # records its wrong bytes, not an unrecorded hash drift.
                        active = pipeline.read_json_file(work / ".locust-injections/active.json")
                        plan_path = work / ".locust-injections/operations" / active["transaction_id"] / "plan.json"
                        plan = pipeline.read_json_file(plan_path)
                        if label != "binary":
                            for change in plan["files"]:
                                if change["path"] == "index.html":
                                    change["result"].update(pipeline.file_hash(target))
                            plan_path.write_bytes(pipeline.encoded(plan))
                    return result
                with patch.object(pipeline.subprocess, "run", side_effect=run):
                    code, result, stderr = self.html_public(options)
                self.assertEqual((code, result, stderr), (1, {"status": "failed", "diagnostic": "HTML bytes outside selected targets or exact writer output differ"}, ""))
                manifest = pipeline.read_json_file(output / "pipeline-manifest.json")
                self.assertEqual(manifest["status"], "failed")
                self.assertFalse(any(s["stage"] == "pack-spanish" for s in manifest["stages"]))
                self.assertFalse((output / "spanish-patch.zip").exists())
                self.assertEqual(pipeline.scan(Path(options.game)), pipeline.scan(output / "pristine"))
                self.assertEqual(pipeline.scan(output / "install-game"), pipeline.scan(output / "pristine"))
                retained = pipeline.scan(output)
                with patch.object(pipeline.subprocess, "run") as child:
                    resume_code, resume_result, resume_stderr = self.html_public(replace(options, resume=True))
                self.assertEqual((resume_code, resume_stderr), (1, ""))
                self.assertIn("Resume refused", resume_result["diagnostic"])
                child.assert_not_called()
                self.assertEqual(pipeline.scan(output), retained)
        self.root = base

    def test_renpy_final_install_is_spanish(self):
        self.positive("renpy")

    def test_mv_wrapping_offsets_and_final_install(self):
        self.positive("rpgmaker-mv")

    def test_loose_kirikiri_controls_and_final_install(self):
        self.positive("kirikiri")

    def negative(self, engine, fault):
        options, server, _, _ = self.scenario(engine, fault)
        before = pipeline.scan(Path(options.game))
        with self.assertRaises(pipeline.PipelineError):
            pipeline.Pipeline(options).run()
        manifest = pipeline.read_json_file(Path(options.output) / "pipeline-manifest.json")
        self.assertEqual(manifest["status"], "failed")
        self.assertFalse(manifest["injection_started"])
        self.assertFalse(Path(options.output, "spanish-patch.zip").exists())
        for name in ("work-game", "install-game", "pristine"):
            self.assertEqual(pipeline.scan(Path(options.output) / name), before)
        self.assertEqual(pipeline.scan(Path(options.game)), before)
        self.assertEqual(server.errors, [])
        self.assertTrue(server.requests, "Negative must reach scripted translation, not fail extraction")
        if fault == "missing":
            self.assertFalse(Path(options.output, "spanish.db").exists())
        return options, server

    def test_missing_answer_refuses_release_and_pending_resume_retries(self):
        options, server = self.negative("renpy", "missing")
        server.fault = None
        count = len(server.requests)
        summary = pipeline.Pipeline(replace(options, resume=True)).run()
        self.assertEqual(summary["status"], "completed")
        retries = [key for key in server.requests[count:] if key[0] == "ja"]
        self.assertTrue(retries)
        self.assertTrue(all("{PL_0}" in key[2] for key in retries))

    def test_corrupt_spanish_placeholder_refuses_release(self):
        self.negative("rpgmaker-mv", "placeholder")

    def test_altered_raw_kag_argument_refuses_release(self):
        self.negative("kirikiri", "kag")

    def test_mixed_kirikiri_unchanged_row_omission_rejects(self):
        options, _, _, _ = self.scenario("kirikiri", mixed_kirikiri=True)
        original = pipeline.scan(Path(options.game))
        with self.assertRaisesRegex(pipeline.PipelineError, "Injection inventory count mismatch"):
            pipeline.Pipeline(options).run()
        manifest = pipeline.read_json_file(Path(options.output) / "pipeline-manifest.json")
        self.assertEqual(manifest["status"], "failed")
        self.assertTrue(manifest["injection_started"])
        self.assertFalse(Path(options.output, "spanish-patch.zip").exists())
        self.assertEqual(pipeline.scan(Path(options.game)), original)
        with self.assertRaisesRegex(pipeline.PipelineError, "Resume refused"):
            pipeline.Pipeline(replace(options, resume=True)).run()

    def test_informational_notice_does_not_hide_warnings_or_bad_provenance(self):
        cases = ("warning", "altered-notice", "phase", "retained-original", "whole-backup", "foreign-backup", "nested-count", "outcome-count")
        real_run = subprocess.run
        for fault in cases:
            with self.subTest(fault=fault):
                case_root = self.root / fault
                case_root.mkdir()
                answers, _, _ = fixture(case_root / "source", "renpy")
                server = ScriptedEndpoint(answers)
                options = pipeline.Options(str(CLI), str(case_root / "source"), str(case_root / "run"), "renpy", "google",
                                           test_endpoint=server.url, batch_size=1, concurrency=1, timeout=60)
                original = pipeline.scan(Path(options.game))
                altered = False

                def intercept(argv, **kwargs):
                    nonlocal altered
                    result = real_run(argv, **kwargs)
                    if argv[3] != "inject" or "--help" in argv:
                        return result
                    self.assertEqual(result.returncode, 0)
                    altered = True
                    report = json.loads(result.stdout)
                    root = Path(options.output)
                    active = pipeline.read_json_file(root / "work-game/.locust-injections/active.json")
                    operation = root / "work-game/.locust-injections/operations" / active["transaction_id"]
                    if fault == "warning":
                        report["warnings"].append("partial insertion synthetic warning")
                        report["reports"]["es"]["warnings"] = report["warnings"]
                    elif fault == "altered-notice":
                        report["warnings"][0] += " unverified suffix"
                        report["reports"]["es"]["warnings"] = report["warnings"]
                    elif fault == "phase":
                        (operation / "phase.json").write_text('"committed_unrecorded"')
                    elif fault == "retained-original":
                        (operation / "originals/0").write_bytes(b"corrupted retained original")
                    elif fault == "whole-backup":
                        (root / "backups" / report["backup_id"] / "payload/game/script.rpy").write_bytes(b"corrupted full backup")
                    elif fault == "foreign-backup":
                        report["backup_path"] = options.game
                    elif fault == "nested-count":
                        report["reports"]["es"]["files_modified"] = True
                    else:
                        report["outcomes"][0][1]["Recorded"]["files"] = True
                    return subprocess.CompletedProcess(argv, 0, json.dumps(report).encode(), result.stderr)

                try:
                    with patch.object(pipeline.subprocess, "run", side_effect=intercept):
                        with self.assertRaises(pipeline.PipelineError):
                            pipeline.Pipeline(options).run()
                    self.assertTrue(altered)
                    manifest = pipeline.read_json_file(Path(options.output) / "pipeline-manifest.json")
                    self.assertEqual(manifest["status"], "failed")
                    self.assertNotIn("translated_copy", manifest)
                    self.assertFalse(Path(options.output, "spanish-patch.zip").exists())
                    self.assertEqual(pipeline.scan(Path(options.game)), original)
                    self.assertEqual(server.errors, [])
                finally:
                    server.close()

    def test_spanish_pending_resume_preserves_pivot_and_does_not_reextract(self):
        options, server = self.negative("rpgmaker-mv", "placeholder")
        english = pipeline.file_hash(Path(options.output) / "english.db")
        pivot_rows, _ = pipeline.read_database(Path(options.output) / "spanish.db")
        signature = pipeline.inventory(pivot_rows, Path(options.output) / "work-game", options.format)
        server.fault = None
        previous_requests = len(server.requests)
        summary = pipeline.Pipeline(replace(options, resume=True)).run()
        self.assertEqual(summary["status"], "completed")
        self.assertEqual(pipeline.file_hash(Path(options.output) / "english.db"), english)
        current, _ = pipeline.read_database(Path(options.output) / "spanish.db")
        self.assertEqual(pipeline.inventory(current, Path(options.output) / "work-game", options.format), signature)
        self.assertTrue(all(key[0] == "en" for key in server.requests[previous_requests:]))
        manifest = pipeline.read_json_file(Path(options.output) / "pipeline-manifest.json")
        self.assertEqual(sum(stage["stage"] == "extract-japanese" for stage in manifest["stages"]), 1)
        self.assertEqual(sum(stage["stage"] == "strict-pivot" for stage in manifest["stages"]), 1)

    def test_bad_receipt_final_bytes_or_final_resource_cannot_complete(self):
        real_run, real_reader = subprocess.run, pipeline.read_database
        for fault in ("legacy-receipt", "final-bytes", "final-resource"):
            with self.subTest(fault=fault):
                case_root = self.root / fault
                case_root.mkdir()
                answers, _, _ = fixture(case_root / "source", "renpy")
                server = ScriptedEndpoint(answers)
                options = pipeline.Options(str(CLI), str(case_root / "source"), str(case_root / "run"), "renpy", "google",
                                           test_endpoint=server.url, batch_size=2, concurrency=1, timeout=60)
                original = pipeline.scan(Path(options.game))
                applies = 0
                altered = False

                def intercept(argv, **kwargs):
                    nonlocal applies, altered
                    result = real_run(argv, **kwargs)
                    if argv[3] != "apply":
                        return result
                    applies += 1
                    install = Path(options.output) / "install-game"
                    if fault == "legacy-receipt" and applies == 2:
                        receipt = pipeline.read_json_file(install / ".locust/receipt.json")
                        receipt["verification"] = "legacy"
                        (install / ".locust/receipt.json").write_text(json.dumps(receipt))
                        altered = True
                    if fault == "final-bytes" and applies == 3:
                        (install / "game/script.rpy").write_bytes(b"synthetic final byte drift")
                        altered = True
                    return result

                def read(path):
                    nonlocal altered
                    rows, metadata = real_reader(path)
                    if fault == "final-resource" and Path(path).name == "final-install-proof.db":
                        rows[0]["source"] = "synthetic wrong resource"
                        altered = True
                    return rows, metadata

                try:
                    with patch.object(pipeline.subprocess, "run", side_effect=intercept), patch.object(pipeline, "read_database", side_effect=read):
                        with self.assertRaises(pipeline.PipelineError) as failure:
                            pipeline.Pipeline(options).run()
                    self.assertTrue(altered, str(failure.exception) + " endpoint errors=" + repr(server.errors))
                    manifest = pipeline.read_json_file(Path(options.output) / "pipeline-manifest.json")
                    self.assertEqual(manifest["status"], "failed")
                    self.assertNotIn("translated_copy", manifest)
                    self.assertEqual(pipeline.scan(Path(options.game)), original)
                    self.assertEqual(server.errors, [])
                finally:
                    server.close()

    def test_failed_final_reapply_never_complete_or_resumeable(self):
        options, _, _, _ = self.scenario("renpy")
        real_run = subprocess.run
        applies = 0

        def intercept(argv, **kwargs):
            nonlocal applies
            if argv[3] == "apply":
                applies += 1
                if applies == 3:
                    return subprocess.CompletedProcess(argv, 1, b"", b"synthetic final failure")
            return real_run(argv, **kwargs)

        with patch.object(pipeline.subprocess, "run", side_effect=intercept):
            with self.assertRaisesRegex(pipeline.PipelineError, "final-reapply"):
                pipeline.Pipeline(options).run()
        manifest = pipeline.read_json_file(Path(options.output) / "pipeline-manifest.json")
        self.assertEqual(manifest["status"], "failed")
        self.assertTrue(manifest["rollback_verified"])
        self.assertNotIn("translated_copy", manifest)
        self.assertEqual(pipeline.scan(Path(options.output) / "install-game"), manifest["source"])
        with self.assertRaisesRegex(pipeline.PipelineError, "Resume refused"):
            pipeline.Pipeline(replace(options, resume=True)).run()


if __name__ == "__main__":
    unittest.main()
