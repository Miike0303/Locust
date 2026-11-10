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
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("updater_manifest", ROOT / "scripts/generate-updater-manifest.py")
m = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(m)
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
        self.temp = tempfile.TemporaryDirectory(dir=ROOT / "tools", prefix="locust-manifest-tests-")
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

    def invoke(self, names, signatures=None, failure=None, args=None, metadata=None, draft=False):
        signatures = signatures if signatures is not None else {
            name + ".sig": signature(index + 1) for index, name in enumerate(ASSETS.values())}
        def run(command, **kwargs):
            self.calls.append(command)
            self.assertTrue(kwargs["check"])
            self.assertTrue(kwargs["capture_output"])
            self.assertLessEqual(kwargs["timeout"], 30)
            if command[:3] == ["gh", "release", "view"]:
                self.assertEqual(command[:7], ["gh", "release", "view", "v0.2.0", "--repo",
                                               "Miike0303/Locust", "--json"])
                self.assertEqual(command[7], "assets,body,isDraft")
                text = metadata if metadata is not None else json.dumps({
                    "assets": [{"name": name} for name in names], "body": "Public release notes", "isDraft": draft})
            else:
                if draft:
                    self.assertEqual(command[:3], ["gh", "release", "download"])
                    self.assertEqual(command[3:7], ["v0.2.0", "--repo", "Miike0303/Locust", "--pattern"])
                    self.assertEqual(command[8:], ["--output", "-"])
                    name = command[7]
                else:
                    name = command[-1].rsplit("/", 1)[-1]
                    self.assertEqual(command[:6], ["curl", "--fail", "--silent", "--show-error", "--location", "--"])
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

    def test_default_strict_mode_refuses_each_missing_platform_or_signature(self):
        for target in ASSETS:
            for missing in (ASSETS[target], ASSETS[target] + ".sig"):
                with self.subTest(missing=missing):
                    self.calls.clear()
                    names = [name for name in self.pairs(ASSETS) if name != missing]
                    self.assert_refused(self.invoke(names), "incomplete updater platform set: " + target)
                    self.assertEqual(len(self.calls), 1)

    def test_explicit_recovery_flag_allows_legacy_and_partial_platforms(self):
        legacy = [key for key in ASSETS if key != "windows-aarch64"]
        for selected in (legacy, ["windows-aarch64"], ["linux-x86_64"]):
            with self.subTest(selected=selected):
                code, out, err = self.invoke(self.pairs(selected) + ["unrelated.zip"], args=["--allow-partial", "v0.2.0"])
                self.assertEqual((code, err), (0, ""))
                self.assertEqual(set(json.loads(out)["platforms"]), set(selected))
        # Missing complete pairs are omitted, not substituted with another architecture.
        code, out, err = self.invoke(self.pairs(["linux-x86_64"]) + [ASSETS["windows-x86_64"], ASSETS["windows-aarch64"] + ".sig"], args=["--allow-partial", "v0.2.0"])
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(set(json.loads(out)["platforms"]), {"linux-x86_64"})

    def test_arm_alias_legacy_signature_and_empty_notes(self):
        name = "Locust_0.2.0_aarch64_en-US.msi"
        value = signature(7, b"Ed")
        metadata = json.dumps({"assets": [{"name": name}, {"name": name + ".sig"}], "body": None})
        code, out, err = self.invoke([], {name + ".sig": value}, args=["--allow-partial", "v0.2.0"], metadata=metadata)
        self.assertEqual((code, err), (0, ""))
        doc = json.loads(out)
        self.assertEqual(doc["notes"], "")
        self.assertEqual(doc["platforms"], {"windows-aarch64": {"signature": value,
            "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/" + name}})
        self.manifest.write_bytes(b"existing manifest\n")
        self.assert_refused(self.invoke([name, name + ".sig"] + self.pairs(["windows-aarch64"])),
                            "duplicate or ambiguous release assets")

    def test_recovery_flag_still_refuses_zero_pairs(self):
        self.assert_refused(self.invoke([], args=["--allow-partial", "v0.2.0"]),
                            "no complete updater artifact/signature pairs")

    def test_draft_signatures_use_authenticated_downloads_before_publication(self):
        code, out, err = self.invoke(self.pairs(ASSETS), draft=True)
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(set(json.loads(out)["platforms"]), set(ASSETS))
        self.assertEqual(len(self.calls), 6)
        self.assertTrue(all(command[:3] == ["gh", "release", "download"] for command in self.calls[1:]))

    def test_failed_draft_download_preserves_existing_manifest(self):
        self.assert_refused(self.invoke(self.pairs(ASSETS), draft=True,
                                       failure=ASSETS["linux-x86_64"] + ".sig"),
                            "signature download failed")

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
                self.assert_refused(self.invoke([name, name + ".sig"], {name + ".sig": value}, args=["--allow-partial", "v0.2.0"]), "invalid updater signature")

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
            code, out, err = self.invoke(self.pairs(ASSETS))
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(set(json.loads(out)["platforms"]), set(ASSETS))
        self.manifest.write_bytes(b"existing manifest\n")
        for repo, tag in (("https://example.invalid/repo", "v0.2.0"), ("Miike0303/Locust", "--help")):
            with mock.patch.dict(os.environ, {"GITHUB_REPOSITORY": repo}):
                self.assert_refused(self.invoke([], args=[tag]), "invalid repository or version tag")

    def updater_config(self, key_id, text=None):
        # Outside the working directory: refusals must leave only latest.json there.
        folder = tempfile.TemporaryDirectory(dir=ROOT / "tools", prefix="locust-manifest-config-")
        self.addCleanup(folder.cleanup)
        wire = ("untrusted comment: minisign public key: TEST\n" +
                base64.b64encode(b"Ed" + key_id + bytes(32)).decode() + "\n")
        pubkey = base64.b64encode(wire.encode()).decode() if text is None else text
        path = Path(folder.name) / "tauri.conf.json"
        path.write_text(json.dumps({"plugins": {"updater": {"pubkey": pubkey}}}), encoding="utf-8")
        return mock.patch.dict(os.environ, {"LOCUST_UPDATER_CONFIG": str(path)})

    def test_configured_public_key_accepts_only_its_own_key_id(self):
        same_key = {name + ".sig": signature(9) for name in ASSETS.values()}
        with self.updater_config(bytes([9]) * 8):
            code, out, err = self.invoke(self.pairs(ASSETS), signatures=same_key)
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(set(json.loads(out)["platforms"]), set(ASSETS))
        self.manifest.write_bytes(b"existing manifest\n")
        with self.updater_config(bytes([8]) * 8):
            self.assert_refused(self.invoke(self.pairs(ASSETS), signatures=same_key),
                                "updater signature was not made with the configured public key")
        one_foreign = dict(same_key)
        one_foreign[ASSETS["linux-x86_64"] + ".sig"] = signature(3)
        with self.updater_config(bytes([9]) * 8):
            self.assert_refused(self.invoke(self.pairs(ASSETS), signatures=one_foreign),
                                "updater signature was not made with the configured public key")

    def test_unreadable_configured_public_key_is_refused(self):
        same_key = {name + ".sig": signature(9) for name in ASSETS.values()}
        with self.updater_config(b"", text="not base64!"):
            self.assert_refused(self.invoke(self.pairs(ASSETS), signatures=same_key),
                                "invalid configured updater public key")
        with mock.patch.dict(os.environ, {"LOCUST_UPDATER_CONFIG": str(ROOT / "tools" / "missing.conf.json")}):
            self.assert_refused(self.invoke(self.pairs(ASSETS), signatures=same_key),
                                "invalid configured updater public key")

    def test_release_workflow_checks_the_key_and_requires_green_ci(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        self.assertIn("LOCUST_UPDATER_CONFIG: apps/desktop/src-tauri/tauri.conf.json", workflow)
        self.assertIn('gh run list --workflow ci.yml --commit "$RELEASE_SHA" --status success', workflow)
        self.assertIn("actions: read", workflow)

    def test_write_failure_leaves_no_scratch_and_no_output(self):
        with mock.patch.object(m.os, "replace", side_effect=OSError("PRIVATE PATH")):
            self.assert_refused(self.invoke(self.pairs(ASSETS)), "manifest write failed")

    def test_timeout_is_sanitized_without_retry(self):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(m.subprocess, "run", side_effect=subprocess.TimeoutExpired("gh", 30, output="RAW RESPONSE")) as run, \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = m.main(["v0.2.0"])
        self.assert_refused((code, out.getvalue(), err.getvalue()), "release lookup failed")
        self.assertEqual(run.call_count, 1)

    def test_usage_and_real_python_entrypoint_without_network(self):
        self.assertEqual(self.invoke([], args=[]), (1, "", "Usage: generate-updater-manifest.sh [--allow-partial] <tag>\n"))
        result = subprocess.run([sys.executable, str(ROOT / "scripts/generate-updater-manifest.py")],
                                capture_output=True, text=True, timeout=10)
        self.assertEqual((result.returncode, result.stdout, result.stderr),
                         (1, "", "Usage: generate-updater-manifest.sh [--allow-partial] <tag>\n"))
        self.assertEqual(self.manifest.read_bytes(), b"existing manifest\n")

    def test_real_shell_entrypoint_without_network(self):
        # On Windows use Git Bash, never a potentially WSL-backed bash.exe.
        bash = str(Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe") if os.name == "nt" else shutil.which("bash")
        self.assertTrue(bash and Path(bash).is_file(), "Git Bash / bash required for the shell entrypoint test")
        result = subprocess.run([bash, str(ROOT / "scripts/generate-updater-manifest.sh")],
                                capture_output=True, text=True, timeout=10)
        if os.name == "nt" and result.returncode == 0xC0000142 and not result.stdout and not result.stderr:
            self.skipTest("Git Bash cannot initialize in this Windows sandbox (0xC0000142)")
        self.assertEqual((result.returncode, result.stdout, result.stderr),
                         (1, "", "Usage: generate-updater-manifest.sh [--allow-partial] <tag>\n"))
        self.assertEqual(self.manifest.read_bytes(), b"existing manifest\n")


def load_gate():
    spec = importlib.util.spec_from_file_location("release_gate", ROOT / "scripts/release-gate.py")
    gate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gate)
    return gate


class ReleaseGateTests(unittest.TestCase):
    def setUp(self):
        self.gate = load_gate()
        self.temp = tempfile.TemporaryDirectory(dir=ROOT / "tools", prefix="locust-release-gate-tests-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.versions = {
            "Cargo.toml": '[workspace.package]\nversion = "0.2.0"\n',
            "apps/desktop/package.json": '{"version": "0.2.0"}',
            "apps/desktop/src-tauri/tauri.conf.json": '{"version": "0.2.0"}',
        }
        for name, content in self.versions.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        self.sha = "a" * 40
        names = [name for artifact in ASSETS.values() for name in (artifact, artifact + ".sig")]
        names += ["locust-x86_64-windows.exe", "locust-aarch64-macos",
                  "locust-x86_64-macos", "locust-x86_64-linux", "latest.json"]
        self.release = {
            "tagName": "v0.2.0", "targetCommitish": self.sha,
            "isDraft": True, "isPrerelease": False, "body": "Release notes",
            "assets": [{"name": name, "size": 123} for name in names],
        }
        self.manifest = {
            "version": "0.2.0", "notes": "Release notes", "pub_date": "2026-11-02T00:00:00Z",
            "platforms": {key: {
                "url": "https://github.com/Miike0303/Locust/releases/download/v0.2.0/" + name,
                "signature": signature(index + 1),
            } for index, (key, name) in enumerate(ASSETS.items())},
        }
        self.expected = json.loads(json.dumps(self.manifest))

    def verify(self):
        return self.gate.verify_inventory(self.release, self.manifest, self.expected,
                                          "v0.2.0", self.sha, "Miike0303/Locust")

    def test_tag_must_equal_all_three_manifest_versions(self):
        self.assertEqual(self.gate.check_versions("refs/tags/v0.2.0", self.root), "v0.2.0")

    def test_mismatch_in_each_manifest_is_refused(self):
        for name, content in self.versions.items():
            with self.subTest(manifest=name):
                path = self.root / name
                path.write_text(content.replace("0.2.0", "0.3.0"), encoding="utf-8")
                with self.assertRaisesRegex(self.gate.Refused, "version mismatch"):
                    self.gate.check_versions("refs/tags/v0.2.0", self.root)
                path.write_text(content, encoding="utf-8")

    def test_non_version_refs_and_version_named_branches_are_refused(self):
        for ref in ("refs/heads/main", "refs/heads/v0.2.0", "refs/tags/0.2.0",
                    "refs/tags/vnext", "v0.2.0", "refs/tags/v0.2"):
            with self.subTest(ref=ref), self.assertRaisesRegex(self.gate.Refused, "version tag ref"):
                self.gate.check_versions(ref, self.root)

    def test_matching_application_versions_still_must_match_tag(self):
        with self.assertRaisesRegex(self.gate.Refused, "version mismatch"):
            self.gate.check_versions("refs/tags/v0.3.0", self.root)

    def test_changelog_extracts_only_requested_section(self):
        section = "## [0.2.0] - 2026-10-17\n\n### Fixed\n\n- Actual change.\n"
        text = "# Changelog\n\n## [Unreleased]\n\n- Future.\n\n" + section + "\n## [0.1.0]\n\n- Old.\n"
        self.assertEqual(self.gate.changelog_section(text, "0.2.0"), section)
        self.assertEqual(self.gate.changelog_section(text.replace("\n", "\r\n"), "0.2.0"), section)

    def test_missing_empty_or_duplicate_changelog_section_is_refused(self):
        for text in ("## [Unreleased]\n\n- Future.\n", "## [0.2.0]\n\n## [0.1.0]\n",
                     "## [0.2.0]\n\n- One.\n\n## [0.2.0]\n\n- Two.\n"):
            with self.subTest(text=text), self.assertRaises(self.gate.Refused):
                self.gate.changelog_section(text, "0.2.0")

    def test_complete_draft_inventory_is_accepted(self):
        self.verify()

    def test_each_missing_desktop_signature_cli_or_manifest_is_refused(self):
        assets = self.release["assets"]
        for missing in assets:
            with self.subTest(asset=missing["name"]):
                self.release["assets"] = [asset for asset in assets if asset is not missing]
                with self.assertRaises(self.gate.Refused):
                    self.verify()
        self.release["assets"] = assets

    def test_zero_size_and_duplicate_required_assets_are_refused(self):
        self.release["assets"][0]["size"] = 0
        with self.assertRaises(self.gate.Refused):
            self.verify()
        self.release["assets"][0]["size"] = 123
        self.release["assets"].append(dict(self.release["assets"][0]))
        with self.assertRaises(self.gate.Refused):
            self.verify()

    def test_public_prerelease_wrong_tag_and_wrong_source_are_refused(self):
        for field, value in (("isDraft", False), ("isPrerelease", True),
                             ("tagName", "v0.3.0"), ("targetCommitish", "b" * 40)):
            with self.subTest(field=field):
                original = self.release[field]
                self.release[field] = value
                with self.assertRaises(self.gate.Refused):
                    self.verify()
                self.release[field] = original

    def test_incomplete_manifest_and_wrong_asset_url_are_refused(self):
        del self.manifest["platforms"]["windows-aarch64"]
        with self.assertRaises(self.gate.Refused):
            self.gate.verify_inventory(self.release, self.manifest, self.manifest,
                                       "v0.2.0", self.sha, "Miike0303/Locust")
        self.manifest = json.loads(json.dumps(self.expected))
        self.manifest["platforms"]["linux-x86_64"]["url"] = "https://example.invalid/wrong.AppImage"
        self.expected = json.loads(json.dumps(self.manifest))
        with self.assertRaises(self.gate.Refused):
            self.verify()

    def test_uploaded_manifest_must_equal_generated_signatures_and_notes(self):
        for field, value in (("signature", signature(99)), ("url", "https://example.invalid/other")):
            with self.subTest(field=field):
                self.manifest = json.loads(json.dumps(self.expected))
                self.manifest["platforms"]["windows-x86_64"][field] = value
                with self.assertRaises(self.gate.Refused):
                    self.verify()
        self.manifest = json.loads(json.dumps(self.expected))
        self.manifest["notes"] = "Different notes"
        with self.assertRaises(self.gate.Refused):
            self.verify()

    def test_wrong_manifest_version_notes_and_signature_shape_are_refused(self):
        for field, value in (("version", "0.3.0"), ("notes", "Different notes")):
            with self.subTest(field=field):
                self.manifest = json.loads(json.dumps(self.expected))
                self.manifest[field] = value
                with self.assertRaises(self.gate.Refused):
                    self.gate.verify_inventory(self.release, self.manifest, self.manifest,
                                               "v0.2.0", self.sha, "Miike0303/Locust")
        self.manifest = json.loads(json.dumps(self.expected))
        self.manifest["platforms"]["linux-x86_64"]["signature"] = "<html>error</html>"
        with self.assertRaises(self.gate.Refused):
            self.gate.verify_inventory(self.release, self.manifest, self.manifest,
                                       "v0.2.0", self.sha, "Miike0303/Locust")

    def test_prepare_command_emits_only_changelog_or_refuses_without_output(self):
        section = "## [0.2.0] - 2026-10-17\n\n- Released change.\n"
        (self.root / "CHANGELOG.md").write_text("## [Unreleased]\n\n" + section, encoding="utf-8")
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = self.gate.main(["prepare", "--ref", "refs/tags/v0.2.0", "--root", str(self.root)])
        self.assertEqual((code, out.getvalue(), err.getvalue()), (0, section, ""))
        (self.root / "CHANGELOG.md").write_text("## [Unreleased]\n", encoding="utf-8")
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = self.gate.main(["prepare", "--ref", "refs/tags/v0.2.0", "--root", str(self.root)])
        self.assertEqual((code, out.getvalue()), (1, ""))
        self.assertIn("CHANGELOG release section", err.getvalue())

    def test_verify_command_requires_changelog_body_before_accepting_inventory(self):
        files = {"release.json": self.release, "manifest.json": self.manifest, "expected.json": self.expected}
        for name, content in files.items():
            (self.root / name).write_text(json.dumps(content), encoding="utf-8")
        notes = self.root / "notes.md"
        notes.write_text("Release notes\n", encoding="utf-8")
        args = ["verify", "--tag", "v0.2.0", "--sha", self.sha,
                "--metadata", str(self.root / "release.json"), "--manifest", str(self.root / "manifest.json"),
                "--expected-manifest", str(self.root / "expected.json"), "--notes-file", str(notes),
                "--repo", "Miike0303/Locust"]
        self.assertEqual(self.gate.main(args), 0)
        notes.write_text("Different changelog notes\n", encoding="utf-8")
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = self.gate.main(args)
        self.assertEqual((code, out.getvalue()), (1, ""))
        self.assertIn("release body differs from CHANGELOG", err.getvalue())


if __name__ == "__main__":
    unittest.main()
