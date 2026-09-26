use locust_core::extraction::FormatPlugin;
use std::path::Path;

#[path = "support/real_copy.rs"]
mod real_copy;

#[test]
#[ignore = "requires LOCUST_REAL_UNITY_COPY pointing to an explicit game copy"]
fn test_inject_unity_direct() {
    let plugin = locust_formats::unity::UnityPlugin::new();
    let (fixture, entry) = real_copy::isolated_entry(
        &plugin,
        "LOCUST_REAL_UNITY_COPY",
        Path::new(r"D:\juegos\unity\Out of Touch"),
        None,
    );
    let report = plugin.inject(fixture.path(), &[entry]).unwrap();
    assert!(report.files_modified > 0);
    assert!(report.strings_written > 0);
    for file in report.files_written {
        assert!(
            file.starts_with(fixture.path()),
            "Write escaped fixture: {}",
            file.display()
        );
    }
}
