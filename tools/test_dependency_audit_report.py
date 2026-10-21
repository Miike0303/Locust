"""Public subprocess checks for report-only audit collection; no network or scanners."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
REPORTER = ROOT / "tools" / "dependency_audit_report.py"


class AuditReportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=ROOT / ".test-tmp")
        self.addCleanup(self.temp.cleanup)
        self.cwd = Path(self.temp.name)
        self.locks = {name: b"unchanged fixture\n" for name in ("Cargo.lock", "package-lock.json")}
        for name, content in self.locks.items():
            (self.cwd / name).write_bytes(content)

    def run_report(self, kind, payload, code, output="report", command=None):
        raw = payload if isinstance(payload, bytes) else json.dumps(payload).encode() + b"\n"
        child = command or [sys.executable, "-c",
                            f"import sys;sys.stdout.buffer.write({raw!r});"
                            f"sys.stderr.buffer.write(b'detail\\n');sys.exit({code})"]
        result = subprocess.run([sys.executable, str(REPORTER), "--kind", kind,
                                 "--output", output, "--", *child], cwd=self.cwd,
                                capture_output=True, env={**os.environ, "PYTHONIOENCODING": "utf-8"})
        for name, content in self.locks.items():
            self.assertEqual((self.cwd / name).read_bytes(), content)
        return result, raw

    def assert_report(self, kind, payload, child_code, status, wrapper_code):
        result, raw = self.run_report(kind, payload, child_code)
        self.assertEqual(result.returncode, wrapper_code, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["status"], status)
        self.assertEqual(report["exit_code"], child_code)
        self.assertEqual((self.cwd / "report" / "stdout.json").read_bytes(), raw)
        self.assertEqual((self.cwd / "report" / "stderr.txt").read_bytes(), b"detail\n")
        self.assertEqual((self.cwd / "report" / "result.json").read_bytes(), result.stdout)
        self.assertEqual(result.stdout, (json.dumps(report, indent=2, ensure_ascii=True) + "\n").encode())
        self.assertEqual(result.stderr, b"")
        return report

    def test_clean_rust_and_npm(self):
        for kind, payload in [("rust", {"vulnerabilities": {"count": 0, "list": []}, "warnings": {}}),
                              ("npm", {"metadata": {"vulnerabilities": {"total": 0}}})]:
            with self.subTest(kind=kind):
                # Each report owns a new directory, as in CI.
                if (self.cwd / "report").exists():
                    self.cwd = self.cwd / "next"
                    self.cwd.mkdir()
                    for name, content in self.locks.items():
                        (self.cwd / name).write_bytes(content)
                self.assert_report(kind, payload, 0, "clean", 0)

    def test_findings_preserve_child_failure_but_are_report_only(self):
        self.assert_report("rust", {"vulnerabilities": {"count": 1, "list": [{"advisory": {"id": "RUSTSEC-fixture"}}]}},
                           1, "findings", 0)

    def test_npm_findings_are_report_only(self):
        self.assert_report("npm", {"metadata": {"vulnerabilities": {"total": 2}}}, 1, "findings", 0)

    def test_rust_warnings_are_not_called_clean(self):
        self.assert_report("rust", {"vulnerabilities": {"count": 0, "list": []},
                                    "warnings": {"unmaintained": [{"advisory": {"id": "fixture"}}]}},
                           0, "warnings", 0)

    def test_malformed_falsey_rust_warnings_are_not_clean(self):
        for index, warnings in enumerate([[], False, "", 0, None]):
            with self.subTest(warnings=warnings):
                self.cwd = self.cwd / f"case-{index}"
                self.cwd.mkdir()
                for name, content in self.locks.items():
                    (self.cwd / name).write_bytes(content)
                self.assert_report("rust", {"vulnerabilities": {"count": 0, "list": []},
                                            "warnings": warnings}, 0, "error", 2)

    def test_tool_failure_is_not_a_clean_or_findings_report(self):
        self.assert_report("rust", b"database unavailable\n", 1, "error", 2)

    def test_malformed_success_is_not_clean(self):
        self.assert_report("npm", {"error": {"message": "network error"}}, 0, "error", 2)

    def test_missing_tool_records_unavailable(self):
        result, _ = self.run_report("rust", b"", 0, command=[str(self.cwd / "missing-auditor")])
        self.assertEqual(result.returncode, 2)
        report = json.loads(result.stdout)
        self.assertEqual(report["status"], "error")
        self.assertIsNone(report["exit_code"])
        self.assertTrue(report["error"])
        self.assertEqual((self.cwd / "report" / "stdout.json").read_bytes(), b"")
        self.assertEqual((self.cwd / "report" / "stderr.txt").read_bytes(), b"")
        self.assertEqual(result.stderr, b"")

    def test_existing_evidence_is_rejected_without_overwrite(self):
        directory = self.cwd / "report"
        directory.mkdir()
        sentinel = directory / "result.json"
        sentinel.write_bytes(b"retained evidence")
        result, _ = self.run_report("npm", {}, 0)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, b"")
        self.assertIn(b"output directory already exists", result.stderr)
        self.assertEqual(sentinel.read_bytes(), b"retained evidence")
        self.assertEqual(list(directory.iterdir()), [sentinel])


class WorkflowPolicyTests(unittest.TestCase):
    def test_audit_is_informational_and_existing_gates_remain_independent(self):
        source = (ROOT / ".github" / "workflows" / "ci.yml").read_text()
        trigger = source.split("on:\n", 1)[1].split("concurrency:", 1)[0]
        self.assertEqual(trigger.strip(), "push:\n    tags:\n      - 'v*'\n  workflow_dispatch:")
        audit = source.split("  dependency-audit:\n", 1)[1]
        required = source.split("  dependency-audit:\n", 1)[0]
        self.assertIn("    continue-on-error: true\n", audit)
        self.assertIn("      contents: read\n", audit)
        self.assertIn("      - name: Preserve reports including operational failures\n        if: always()", audit)
        self.assertNotIn("needs:", required)
        self.assertNotIn("continue-on-error:", required)
        for gate in ["Build CLI for Python tests", "Python pipeline tests", "Python replay-oracle tests",
                     "Python RGSS oracle tests", "Offline updater manifest tests", "Build frontend",
                     "Frontend unit tests", "Rust format", "Rust lint", "Rust tests"]:
            self.assertEqual(required.count(f"- name: {gate}\n"), 2, gate)


if __name__ == "__main__":
    unittest.main()
