use locust_core::extraction::FormatPlugin;
use std::path::Path;

#[path = "support/real_copy.rs"]
mod real_copy;

#[test]
#[ignore = "requires LOCUST_REAL_RENPY_COPY pointing to an explicit game copy"]
fn test_inject_renpy_rpa_replace() {
    let plugin = locust_formats::renpy::RenPyPlugin::new();
    let (fixture, entry) = real_copy::isolated_entry(
        &plugin,
        "LOCUST_REAL_RENPY_COPY",
        Path::new(r"D:\juegos\renpy\FindingCloud9-0.9.2-pc"),
        Some("rpa"),
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
