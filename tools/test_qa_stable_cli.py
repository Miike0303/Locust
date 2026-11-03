"""Fixture and fail-closed checks; no CLI build or existing game access."""
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import qa_stable_cli as runner
import stable_cli_fixtures as fixtures
from qa_rgss_oracle import decode


def objects(node, class_name):
    if node[0] == "object":
        if node[1] == class_name:
            yield dict(node[2])
        for _, value in node[2]:
            yield from objects(value, class_name)
    elif node[0] == "array":
        for value in node[1]:
            yield from objects(value, class_name)
    elif node[0] == "hash":
        for key, value in node[1]:
            yield from objects(key, class_name)
            yield from objects(value, class_name)


class FixtureTests(unittest.TestCase):
    def test_all_six_distinct_fixtures_and_nonempty_predictions(self):
        self.assertEqual([c.name for c in fixtures.CASES], ["mv", "mz", "xp", "vx", "vx-ace", "renpy"])
        for case in fixtures.CASES:
            with self.subTest(case=case.name):
                before, after = fixtures.files(case), fixtures.files(case, True)
                self.assertEqual(before.keys(), after.keys())
                self.assertNotEqual(before, after)
                self.assertTrue(fixtures.expected_rows(case))
                self.assertEqual(fixtures.expected_rows(case).keys(), fixtures.expected_rows(case, True).keys())
                self.assertTrue(any(b == fixtures.SENTINEL and b == after[p] for p, b in before.items()))

    def test_mv_and_mz_use_distinct_engine_markers_and_headers(self):
        for case in fixtures.CASES[:2]:
            data = fixtures.files(case)
            marker = "js/rpg_core.js" if case.name == "mv" else "js/rmmz_core.js"
            self.assertIn(marker, data)
            event = json.loads(data["data/Map001.json"])["events"][1]
            header = event["pages"][0]["list"][0]
            self.assertEqual(header["code"], 101)
            self.assertEqual(len(header["parameters"]), 4 if case.name == "mv" else 5)

    def test_genuine_xp_text_and_vx_ace_headers(self):
        for case in fixtures.CASES:
            if not case.extension:
                continue
            with self.subTest(case=case.name):
                graph = decode(fixtures.files(case)[f"Data/Map001.{case.extension}"])
                commands = list(objects(graph, "RPG::EventCommand"))
                codes = [c["@code"][1] for c in commands]
                if case.name == "xp":
                    self.assertEqual(codes, [101, 101, 102, 0])
                    self.assertEqual(commands[0]["@parameters"][1][0][1],
                                     fixtures.TEXT["hello"][0].encode("utf-8"))
                else:
                    self.assertEqual(codes, [101, 401, 101, 401, 102, 0])
                    self.assertEqual(len(commands[0]["@parameters"][1]), 4)
                    self.assertEqual(commands[0]["@parameters"][1][0][1], b"")
                for path, data in fixtures.files(case).items():
                    if path.endswith(case.extension):
                        decode(data)

    def test_full_payload_check_refuses_changed_text_or_nontext(self):
        with tempfile.TemporaryDirectory(prefix="locust-stable-unit-") as scratch:
            for case in fixtures.CASES:
                root = Path(scratch) / case.name
                fixtures.create(case, root)
                fixtures.check_payload(case, root, False)
                with self.assertRaises(FileExistsError):
                    fixtures.create(case, root)
                with self.assertRaisesRegex(AssertionError, "unexpected translated payload"):
                    fixtures.check_payload(case, root)
                for name, data in fixtures.files(case, True).items():
                    (root / name).write_bytes(data)
                fixtures.check_payload(case, root)
                sentinel = next(p for p, b in fixtures.files(case).items() if b == fixtures.SENTINEL)
                (root / sentinel).write_bytes(b"changed")
                with self.assertRaisesRegex(AssertionError, "nontext sentinel"):
                    fixtures.check_payload(case, root)

    def test_inventory_detects_added_files_empty_directories_and_byte_changes(self):
        with tempfile.TemporaryDirectory(prefix="locust-inventory-unit-") as scratch:
            root = Path(scratch)
            path = root / "payload"
            path.write_bytes(b"original")
            baseline = runner.inventory(root)
            (root / "empty").mkdir()
            self.assertNotEqual(runner.inventory(root), baseline)
            (root / "empty").rmdir()
            path.write_bytes(b"modified")  # Same length: hashes must catch it.
            self.assertNotEqual(runner.inventory(root), baseline)
            path.write_bytes(b"original")
            (root / ".locust").mkdir()
            (root / ".locust/receipt.json").write_bytes(b"{}")
            self.assertNotEqual(runner.inventory(root), baseline)
            self.assertEqual(runner.inventory(root, payload_only=True), baseline)

    def test_missing_explicit_or_environment_cli_is_an_error(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            for args in ([], ["--cli", "missing-locust-stable-cli-binary"]):
                with self.subTest(args=args), contextlib.redirect_stderr(io.StringIO()) as err:
                    with self.assertRaises(SystemExit) as exit:
                        runner.main(args)
                    self.assertEqual(exit.exception.code, 2)
                    self.assertIn("already-built CLI missing", err.getvalue())


if __name__ == "__main__":
    unittest.main()
