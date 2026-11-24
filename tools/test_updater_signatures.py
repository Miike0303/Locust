"""Offline tests for scripts/verify-updater-signatures.py.

Fixture: a payload signed by the real Tauri signer (`tauri signer sign`) with a
disposable key whose private half was deleted right after signing.
"""
import base64
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("verify_updater_signatures",
                                              ROOT / "scripts/verify-updater-signatures.py")
v = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(v)
FIXTURE = ROOT / "tools/updater-signature-fixture"
ASSETS = {
    "windows-x86_64": "Locust_0.3.0_x64_en-US.msi",
    "windows-aarch64": "Locust_0.3.0_arm64_en-US.msi",
    "darwin-aarch64": "Locust_aarch64.app.tar.gz",
    "darwin-x86_64": "Locust_x64.app.tar.gz",
    "linux-x86_64": "locust_0.3.0_amd64.AppImage",
}


def wire_lines(text):
    return base64.b64decode(text.strip()).decode("utf-8").splitlines()


def encode_lines(lines):
    return base64.b64encode(("\n".join(lines) + "\n").encode("utf-8")).decode("ascii")


class Ed25519Tests(unittest.TestCase):
    def test_rfc8032_test_vector_1(self):
        public = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
        signature = bytes.fromhex(
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555"
            "fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b")
        self.assertTrue(v.ed25519_verify(public, b"", signature))
        self.assertFalse(v.ed25519_verify(public, b"x", signature))
        tampered = signature[:-1] + bytes([signature[-1] ^ 1])
        self.assertFalse(v.ed25519_verify(public, b"", tampered))


class SignatureTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="locust-updater-sig-tests-")
        self.addCleanup(self.temp.cleanup)
        self.dir = Path(self.temp.name)
        self.config = self.dir / "tauri.conf.json"
        self.write_config((FIXTURE / "key.pub").read_text(encoding="ascii").strip())
        self.signature = (FIXTURE / "payload.bin.sig").read_text(encoding="ascii")
        self.artifacts = self.dir / "artifacts"
        self.artifacts.mkdir()
        for name in ASSETS.values():
            shutil.copyfile(FIXTURE / "payload.bin", self.artifacts / name)
            (self.artifacts / (name + ".sig")).write_text(self.signature, encoding="ascii")

    def write_config(self, pubkey):
        self.config.write_text(json.dumps({"plugins": {"updater": {"pubkey": pubkey}}}), encoding="utf-8")

    def run_main(self):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = v.main([str(self.config), str(self.artifacts)])
        return code, out.getvalue(), err.getvalue()

    def assert_refused(self, reason):
        code, out, err = self.run_main()
        self.assertEqual((code, out), (1, ""))
        self.assertEqual(err, "Updater signature verification refused: " + reason + "\n")

    def test_genuine_tauri_signatures_verify_for_all_five_platforms(self):
        code, out, err = self.run_main()
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(out.splitlines(), [f"verified {p}: {n}" for p, n in ASSETS.items()])

    def test_a_changed_payload_is_refused(self):
        name = ASSETS["linux-x86_64"]
        payload = self.artifacts / name
        payload.write_bytes(payload.read_bytes().replace(b"Locust", b"Lacust"))
        self.assert_refused(f"{name} does not match its updater signature")

    def test_a_different_key_with_the_same_key_id_is_refused(self):
        lines = wire_lines(self.write_and_read_pubkey())
        packet = bytearray(base64.b64decode(lines[1]))
        packet[10] ^= 0x01
        lines[1] = base64.b64encode(bytes(packet)).decode("ascii")
        self.write_config(encode_lines(lines))
        self.assert_refused(f"{ASSETS['windows-x86_64']} does not match its updater signature")

    def test_a_signature_from_another_key_id_is_refused(self):
        lines = wire_lines(self.write_and_read_pubkey())
        packet = bytearray(base64.b64decode(lines[1]))
        packet[2] ^= 0xFF
        lines[1] = base64.b64encode(bytes(packet)).decode("ascii")
        self.write_config(encode_lines(lines))
        self.assert_refused(f"{ASSETS['windows-x86_64']} was not signed with the configured public key")

    def test_a_tampered_trusted_comment_is_refused(self):
        lines = wire_lines(self.signature)
        lines[2] = lines[2].replace("timestamp:", "timestamp:9")
        name = ASSETS["darwin-x86_64"]
        (self.artifacts / (name + ".sig")).write_text(encode_lines(lines), encoding="ascii")
        self.assert_refused(f"{name} has a tampered trusted comment")

    def test_missing_artifact_or_signature_is_refused(self):
        (self.artifacts / (ASSETS["darwin-aarch64"] + ".sig")).unlink()
        self.assert_refused(f"missing signature for {ASSETS['darwin-aarch64']}")
        (self.artifacts / ASSETS["darwin-aarch64"]).unlink()
        self.assert_refused("expected exactly one updater artifact for darwin-aarch64, found 0")

    def test_unreadable_public_key_is_refused(self):
        self.write_config("not base64!")
        self.assert_refused("invalid configured updater public key")

    def test_release_publish_verifies_signatures_before_writing_the_manifest(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        verify = workflow.index("python scripts/verify-updater-signatures.py apps/desktop/src-tauri/tauri.conf.json updater-artifacts")
        self.assertLess(verify, workflow.index("bash scripts/generate-updater-manifest.sh"))
        for pattern in ["'*_en-US.msi'", "'*_en-US.msi.sig'", "'*.app.tar.gz'", "'*.app.tar.gz.sig'",
                        "'*.AppImage'", "'*.AppImage.sig'"]:
            self.assertIn("--pattern " + pattern, workflow)

    def write_and_read_pubkey(self):
        return json.loads(self.config.read_text(encoding="utf-8"))["plugins"]["updater"]["pubkey"]


if __name__ == "__main__":
    unittest.main()
