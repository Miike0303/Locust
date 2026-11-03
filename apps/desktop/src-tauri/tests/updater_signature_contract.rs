//! Run via `python scripts/qa-updater-signatures.py` for fresh signer fixtures.
//! No release credentials, network access, application runtime or installation.
use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
use serde::Deserialize;
use std::{collections::BTreeMap, env, error::Error, fs, path::PathBuf};

const ASSETS: [(&str, &str); 5] = [
    ("windows-x86_64", "Locust_0.3.0_x64_en-US.msi"),
    ("windows-aarch64", "Locust_0.3.0_arm64_en-US.msi"),
    ("darwin-aarch64", "Locust_aarch64.app.tar.gz"),
    ("darwin-x86_64", "Locust_x64.app.tar.gz"),
    ("linux-x86_64", "locust_0.3.0_amd64.AppImage"),
];

#[derive(Deserialize)]
struct Entry {
    signature: String,
    url: String,
}

#[derive(Deserialize)]
struct Manifest {
    platforms: BTreeMap<String, Entry>,
}

struct Fixtures {
    public_key: String,
    wrong_public_key: String,
    manifest: Manifest,
    payload_dir: PathBuf,
}

fn fixture_path(name: &str) -> PathBuf {
    env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{name} missing; run python scripts/qa-updater-signatures.py"))
}

fn fixtures() -> Fixtures {
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(fixture_path("LOCUST_UPDATER_MANIFEST")).expect("read production latest.json"),
    )
    .expect("parse production latest.json");
    assert_eq!(manifest.platforms.len(), ASSETS.len());
    for (platform, artifact) in ASSETS {
        let entry = manifest.platforms.get(platform).expect("required platform");
        assert_eq!(
            entry.url,
            format!("https://github.com/Miike0303/Locust/releases/download/v0.3.0/{artifact}")
        );
    }
    Fixtures {
        public_key: fs::read_to_string(fixture_path("LOCUST_UPDATER_PUBLIC_KEY"))
            .expect("read disposable public key"),
        wrong_public_key: fs::read_to_string(fixture_path("LOCUST_UPDATER_WRONG_PUBLIC_KEY"))
            .expect("read wrong public key"),
        manifest,
        payload_dir: fixture_path("LOCUST_UPDATER_PAYLOAD_DIR"),
    }
}

fn base64_to_string(encoded: &str) -> Result<String, Box<dyn Error>> {
    Ok(String::from_utf8(STANDARD.decode(encoded)?)?)
}

fn verify(data: &[u8], release_signature: &str, public_key: &str) -> Result<(), Box<dyn Error>> {
    // Match tauri-plugin-updater 2.10.1's updater.rs::verify_signature:
    // STANDARD base64 -> UTF-8 -> minisign decode -> verify(..., true).
    let public_key = PublicKey::decode(&base64_to_string(public_key)?)?;
    let signature = Signature::decode(&base64_to_string(release_signature)?)?;
    public_key.verify(data, &signature, true)?;
    Ok(())
}

fn assert_crypto_rejected(result: Result<(), Box<dyn Error>>) {
    let error = result.expect_err("cryptographic mismatch must be rejected");
    assert!(matches!(
        error.downcast_ref::<minisign_verify::Error>(),
        Some(minisign_verify::Error::InvalidSignature | minisign_verify::Error::UnexpectedKeyId)
    ));
}

#[test]
#[ignore = "requires disposable Tauri signer fixtures; run python scripts/qa-updater-signatures.py"]
fn all_five_manifest_entries_verify() {
    let fixtures = fixtures();
    for (platform, artifact) in ASSETS {
        let payload = fs::read(fixtures.payload_dir.join(artifact)).expect("read signed payload");
        verify(
            &payload,
            &fixtures.manifest.platforms[platform].signature,
            &fixtures.public_key,
        )
        .unwrap_or_else(|error| panic!("{platform} signature must verify: {error}"));
    }
    println!("VERIFIED: 5/5 manifest entries against their payloads and disposable public key");
}

#[test]
#[ignore = "requires disposable Tauri signer fixtures; run python scripts/qa-updater-signatures.py"]
fn altered_payloads_are_rejected() {
    let fixtures = fixtures();
    for (platform, artifact) in ASSETS {
        let mut payload =
            fs::read(fixtures.payload_dir.join(artifact)).expect("read signed payload");
        payload[0] ^= 1;
        assert_crypto_rejected(verify(
            &payload,
            &fixtures.manifest.platforms[platform].signature,
            &fixtures.public_key,
        ));
    }
    println!("REJECTED: altered payloads (5/5)");
}

#[test]
#[ignore = "requires disposable Tauri signer fixtures; run python scripts/qa-updater-signatures.py"]
fn wrong_public_key_is_rejected() {
    let fixtures = fixtures();
    assert_ne!(fixtures.public_key, fixtures.wrong_public_key);
    for (platform, artifact) in ASSETS {
        let payload = fs::read(fixtures.payload_dir.join(artifact)).expect("read signed payload");
        assert_crypto_rejected(verify(
            &payload,
            &fixtures.manifest.platforms[platform].signature,
            &fixtures.wrong_public_key,
        ));
    }
    println!("REJECTED: wrong public key (5/5)");
}

#[test]
#[ignore = "requires disposable Tauri signer fixtures; run python scripts/qa-updater-signatures.py"]
fn swapped_platform_signatures_are_rejected() {
    let fixtures = fixtures();
    for (platform, artifact) in ASSETS {
        let payload = fs::read(fixtures.payload_dir.join(artifact)).expect("read signed payload");
        for (other_platform, _) in ASSETS {
            if platform != other_platform {
                assert_crypto_rejected(verify(
                    &payload,
                    &fixtures.manifest.platforms[other_platform].signature,
                    &fixtures.public_key,
                ));
            }
        }
    }
    println!("REJECTED: swapped platform signatures (20/20)");
}

#[test]
#[ignore = "requires disposable Tauri signer fixtures; run python scripts/qa-updater-signatures.py"]
fn malformed_base64_is_rejected() {
    let fixtures = fixtures();
    for (platform, artifact) in ASSETS {
        let payload = fs::read(fixtures.payload_dir.join(artifact)).expect("read signed payload");
        let mut signature = fixtures.manifest.platforms[platform].signature.clone();
        signature.replace_range(..1, "%");
        let error = verify(&payload, &signature, &fixtures.public_key)
            .expect_err("malformed manifest signature base64 must be rejected");
        assert!(error.downcast_ref::<base64::DecodeError>().is_some());
    }
    println!("REJECTED: malformed base64 (5/5)");
}
