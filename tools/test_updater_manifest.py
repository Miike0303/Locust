"""Offline updater contract tests: no network, credentials, or build needed."""
import base64
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("updater_manifest", ROOT / "scripts/generate-updater-manifest.py")
m = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(m)
SCRATCH = ROOT / "tmp/release-v0.2.0/manifest-tests"
ASSETS = {
    "windows-x86_64": "Locust_0.2.0_x64_en-US.msi",
    "windows-aarch64": "Locust_0.2.0_arm64_en-US.msi",
    "darwin-aarch64": "Locust_aarch64.app.tar.gz",
    "darwin-x86_64": "Locust_x64.app.tar.gz",
    "linux-x86_64": "locust_0.2.0_amd64.AppImage",
}


def signature(seed, algorithm=b"ED"):
    wire = ("untrusted comment: synthetic test signature\n" +
            base64.b64encode(algorithm + bytes([seed]) * 72).decode() +
            "\ntrusted comment: timestamp:1\n" + base64.b64encode(bytes([seed]) * 64).decode() + "\n")
    return base64.b64encode(wire.encode()).decode()


class ManifestTests(unittest.TestCase):
    def setUp(self):
        SCRATCH.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.previous = Path.cwd()
        os.chdir(self.temp.name)
        self.addCleanup(os.chdir, self.previous)
        self.manifest = Path("latest.json")
        self.manifest.write_bytes(b"existing manifest\n")
        self.env = mock.patch.dict(os.environ, {"GITHUB_REPOSITORY": "Miike0303/Locust"})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.calls = []

    def invoke(self, names, signatures=None, failure=None, args=None, metadata=None):
        signatures = signatures if signatures is not None else {
            name + ".sig": signature(index + 1) for index, name in enumerate(ASSETS.values())}
        def run(command, **kwargs):
            self.calls.append(command)
            self.assertTrue(kwargs["check"])
            self.assertTrue(kwargs["capture_output"])
            self.assertLessEqual(kwargs["timeout"], 30)
            if command[0] == "gh":
                self.assertEqual(command, ["gh", "release", "view", "v0.2.0", "--repo",
                                           "Miike0303/Locust", "--json", "assets,body"])
                text = metadata if metadata is not None else json.dumps({
                    "assets": [{"name": name} for name in names], "body": "Public release notes"})
            else:
                self.assertEqual(command[:6], ["curl", "--fail", "--silent", "--show-error", "--location", "--"])
                name = command[-1].rsplit("/", 1)[-1]
                if failure == name:
                    raise subprocess.CalledProcessError(22, command, output="RAW PRIVATE RESPONSE", stderr="RAW HTTP ERROR")
                text = signatures[name] + "\n"
            return subprocess.CompletedProcess(command, 0, text, "")
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(m.subprocess, "run", side_effect=run), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = m.main(["v0.2.0"] if args is None else args)
        return code, out.getvalue(), err.getvalue()

    def pairs(self, selected):
        # Deliberately place ARM signature before x64; order cannot choose a signature.
        names = [ASSETS[key] for key in selected]
        return list(reversed([name + ".sig" for name in names])) + names

    def assert_refused(self, result, reason):
        self.assertEqual(result, (1, "", "Updater manifest refused: " + reason + "\n"))
        self.assertEqual(self.manifest.read_bytes(), b"existing manifest\n")
        self.assertEqual(sorted(p.name for p in Path.cwd().iterdir()), ["latest.json"])

    def test_all_five_platforms_use_exact_signature_pairs(self):
        code, out, err = self.invoke(self.pairs(ASSETS))
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(self.manifest.read_text(encoding="utf-8"), out)
        doc = json.loads(out)
        self.assertEqual(set(doc), {"version", "notes", "pub_date", "platforms"})
        self.assertEqual((doc["version"], doc["notes"]), ("0.2.0", "Public release notes"))
        self.assertRegex(doc["pub_date"], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$")
        self.assertEqual(set(doc["platforms"]), set(ASSETS))
        for index, (key, name) in enumerate(ASSETS.items()):
            self.assertEqual(doc["platforms"][key], {
                "signature": signature(index + 1),
                "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/" + name})
        self.assertEqual(len(self.calls), 6)

    def test_legacy_four_platform_shape_and_partial_remain_supported(self):
        legacy = [key for key in ASSETS if key != "windows-aarch64"]
        for selected in (legacy, ["windows-aarch64"], ["linux-x86_64"]):
            with self.subTest(selected=selected):
                code, out, err = self.invoke(self.pairs(selected) + ["unrelated.zip"])
                self.assertEqual((code, err), (0, ""))
                self.assertEqual(set(json.loads(out)["platforms"]), set(selected))
        # Missing complete pairs are omitted, not substituted with another architecture.
        code, out, err = self.invoke(self.pairs(["linux-x86_64"]) + [ASSETS["windows-x86_64"], ASSETS["windows-aarch64"] + ".sig"])
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(set(json.loads(out)["platforms"]), {"linux-x86_64"})

    def test_arm_alias_legacy_signature_and_empty_notes(self):
        name = "Locust_0.2.0_aarch64_en-US.msi"
        value = signature(7, b"Ed")
        metadata = json.dumps({"assets": [{"name": name}, {"name": name + ".sig"}], "body": None})
        code, out, err = self.invoke([], {name + ".sig": value}, metadata=metadata)
        self.assertEqual((code, err), (0, ""))
        doc = json.loads(out)
        self.assertEqual(doc["notes"], "")
        self.assertEqual(doc["platforms"], {"windows-aarch64": {"signature": value,
            "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/" + name}})
        self.manifest.write_bytes(b"existing manifest\n")
        self.assert_refused(self.invoke([name, name + ".sig"] + self.pairs(["windows-aarch64"])),
                            "duplicate or ambiguous release assets")

    def test_http_error_does_not_replace_manifest_or_leak_response(self):
        self.assert_refused(self.invoke(self.pairs(ASSETS), failure=ASSETS["linux-x86_64"] + ".sig"),
                            "signature download failed")
        self.assertEqual(len(self.calls), 6)  # No automatic retry.

    def test_empty_html_malformed_and_wrong_minisign_shapes_refused(self):
        name = ASSETS["windows-x86_64"]
        invalid = ["", "   ", "<html>Not found</html>", "%%%%", base64.b64encode(b"not minisign").decode(),
                   base64.b64encode(("untrusted comment: test\n" + base64.b64encode(b"ED" + b"x" * 72).decode() +
                       "\ntrusted comment: test\n" + base64.b64encode(b"short").decode()).encode()).decode()]
        for wire in (b"ZZ" + b"x" * 72, b"EDshort"):
            invalid.append(base64.b64encode(("untrusted comment: test\n" + base64.b64encode(wire).decode() +
                "\ntrusted comment: test\n" + base64.b64encode(b"x" * 64).decode()).encode()).decode())
        for value in invalid:
            with self.subTest(value=value[:12]):
                self.assert_refused(self.invoke([name, name + ".sig"], {name + ".sig": value}), "invalid updater signature")

    def test_ambiguous_artifacts_and_duplicate_names_refused_before_download(self):
        name = ASSETS["windows-x86_64"]
        for names in ([name, name + ".sig", name + ".sig"],
                      [name, name + ".sig", "Other_x64_en-US.msi", "Other_x64_en-US.msi.sig"]):
            self.calls.clear()
            self.assert_refused(self.invoke(names), "duplicate or ambiguous release assets")
            self.assertEqual(len(self.calls), 1)

    def test_empty_unmatched_and_malformed_metadata_refused(self):
        name = ASSETS["windows-x86_64"]
        for names in ([], [name], [name + ".sig"], [name, ASSETS["windows-aarch64"] + ".sig"]):
            self.assert_refused(self.invoke(names), "no complete updater artifact/signature pairs")
        for text in ("not json", "{}", '{"assets": [null]}', '{"assets": [], "body": 3}',
                     '{"assets": [], "body": []}', '{"assets": [], "body": false}'):
            self.assert_refused(self.invoke([], metadata=text), "invalid release metadata")

    def test_release_lookup_error_is_sanitized_and_no_retry(self):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(m.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "gh", stderr="RAW SECRET")) as run, \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = m.main(["v0.2.0"])
        self.assert_refused((code, out.getvalue(), err.getvalue()), "release lookup failed")
        self.assertEqual(run.call_count, 1)

    def test_default_repository_and_rejected_configuration(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            code, out, err = self.invoke(self.pairs(["linux-x86_64"]))
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(set(json.loads(out)["platforms"]), {"linux-x86_64"})
        self.manifest.write_bytes(b"existing manifest\n")
        for repo, tag in (("https://example.invalid/repo", "v0.2.0"), ("Miike0303/Locust", "--help")):
            with mock.patch.dict(os.environ, {"GITHUB_REPOSITORY": repo}):
                self.assert_refused(self.invoke([], args=[tag]), "invalid repository or version tag")

    def test_write_failure_leaves_no_scratch_and_no_output(self):
        with mock.patch.object(m.os, "replace", side_effect=OSError("PRIVATE PATH")):
            self.assert_refused(self.invoke(self.pairs(["linux-x86_64"])), "manifest write failed")

    def test_timeout_is_sanitized_without_retry(self):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(m.subprocess, "run", side_effect=subprocess.TimeoutExpired("gh", 30, output="RAW RESPONSE")) as run, \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = m.main(["v0.2.0"])
        self.assert_refused((code, out.getvalue(), err.getvalue()), "release lookup failed")
        self.assertEqual(run.call_count, 1)

    def test_usage_and_real_shell_entrypoint_without_network(self):
        self.assertEqual(self.invoke([], args=[]), (1, "", "Usage: generate-updater-manifest.sh <tag>\n"))
        # On Windows use Git Bash, never a potentially WSL-backed bash.exe.
        bash = str(Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe") if os.name == "nt" else shutil.which("bash")
        self.assertTrue(bash and Path(bash).is_file(), "Git Bash / bash required for the shell entrypoint test")
        result = subprocess.run([bash, str(ROOT / "scripts/generate-updater-manifest.sh")],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual((result.returncode, result.stdout, result.stderr),
                         (1, "", "Usage: generate-updater-manifest.sh <tag>\n"))
        self.assertEqual(self.manifest.read_bytes(), b"existing manifest\n")


if __name__ == "__main__":
    unittest.main()
