use locust_core::patch::stream::StagingDir;
use std::fs;

#[test]
fn public_owned_file_rejects_path_escape_before_creation() {
    let root = tempfile::tempdir().unwrap();
    let stage = StagingDir::create_prepared(root.path()).unwrap();
    for name in [
        "../escape",
        "..\\escape",
        "/escape",
        "C:\\escape",
        "",
        ".",
        "..",
        "bad:stream",
        "trailing.",
        "trailing ",
        "CON",
        "con.txt",
        "LPT1",
        "nul",
        "control\nname",
        "nul\0name",
    ] {
        assert!(stage.create_file(name).is_err(), "{name:?}");
    }
    assert!(!root.path().join("escape").exists());
    assert_eq!(fs::read_dir(stage.path()).unwrap().count(), 0);
}

#[test]
fn public_owned_file_collision_does_not_claim_or_delete_existing_content() {
    let root = tempfile::tempdir().unwrap();
    let stage = StagingDir::create_prepared(root.path()).unwrap();
    let folder = stage.path().to_owned();
    fs::write(stage.child("collision"), b"unowned bytes").unwrap();
    fs::create_dir(stage.child("unowned")).unwrap();
    fs::write(stage.child("unowned").join("sentinel"), b"recursive bytes").unwrap();
    assert!(stage.create_file("collision").is_err());
    stage.create_file("owned").unwrap();
    drop(stage);
    assert!(!folder.join("owned").exists());
    assert_eq!(
        fs::read(folder.join("collision")).unwrap(),
        b"unowned bytes"
    );
    assert_eq!(
        fs::read(folder.join("unowned/sentinel")).unwrap(),
        b"recursive bytes"
    );
}
