//! Reproduce tiny synthetic IoStore fixtures with an independently built retoc.
//! Usage: cargo run -p locust-formats --example iostore_reference_fixture -- <retoc.exe> <output-dir>
//! The caller selects the output directory; no original game files are needed.
use locust_formats::unreal_locres::{LocresFile, LocresNamespace, LocresString, LocresVersion};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let executable =
        PathBuf::from(args.next().ok_or("retoc executable argument missing")?).canonicalize()?;
    let output = PathBuf::from(args.next().ok_or("output directory argument missing")?);
    fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    let payload = LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: "Fixture".into(),
            name_hash: 0,
            strings: vec![LocresString {
                key: "Greeting".into(),
                value: "Independent retoc fixture greeting".into(),
                source_string_hash: 0x12345678,
                key_hash: 0,
            }],
        }],
    }
    .serialize()?;
    fs::write(output.join("expected.locres"), &payload)?;
    let input = tempfile::tempdir()?;
    fs::create_dir(input.path().join("chunks"))?;
    let id = "010000000000000000000007";
    fs::write(input.path().join("chunks").join(id), payload)?;
    for (number, version) in [
        (5, "PerfectHashWithOverflow"),
        (7, "RemovedOnDemandMetaData"),
        (8, "ReplaceIoChunkHashWithIoHash"),
    ] {
        let manifest = serde_json::json!({
            "chunk_paths": { id: "../../../FixtureGame/Content/Localization/Game/en/Game.locres" },
            "version": version, "mount_point": "../../../",
        });
        fs::write(
            input.path().join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        let toc = output.join(format!("retoc-v{number}.utoc"));
        let status = Command::new(&executable)
            .arg("pack-raw")
            .arg(input.path())
            .arg(&toc)
            .status()?;
        if !status.success() {
            return Err(format!("retoc pack-raw failed: {status}").into());
        }
        let status = Command::new(&executable).arg("verify").arg(&toc).status()?;
        if !status.success() {
            return Err(format!("retoc verify failed: {status}").into());
        }
        let status = Command::new(&executable)
            .arg("list")
            .arg("--path")
            .arg(&toc)
            .status()?;
        if !status.success() {
            return Err(format!("retoc list failed: {status}").into());
        }
    }
    Ok(())
}
