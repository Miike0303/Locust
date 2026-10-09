//! Shared filesystem discovery helpers for format plugins.
//! Recovery pruning only matches directory names; similarly named game resources remain eligible.
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub(crate) fn find_capital_data_dir(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        let data = path.join("Data");
        if data.is_dir() {
            return Some(data);
        }
    }
    None
}

pub(crate) fn is_internal_directory_name(name: &OsStr) -> bool {
    #[cfg(windows)]
    {
        name.eq_ignore_ascii_case(".locust") || name.eq_ignore_ascii_case(".locust-injections")
    }
    #[cfg(not(windows))]
    {
        name == ".locust" || name == ".locust-injections"
    }
}

pub(crate) fn is_game_entry(entry: &walkdir::DirEntry) -> bool {
    !entry.file_type().is_dir() || !is_internal_directory_name(entry.file_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_data_dir(path: &Path, expected: Option<PathBuf>) {
        assert_eq!(find_capital_data_dir(path), expected);
    }

    #[test]
    fn capital_data_dir_finds_data_directory() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("Data");
        std::fs::create_dir(&data).unwrap();
        assert_data_dir(root.path(), Some(data));
    }

    #[test]
    fn capital_data_dir_rejects_directory_without_data() {
        let root = tempfile::tempdir().unwrap();
        assert_data_dir(root.path(), None);
    }

    #[test]
    fn capital_data_dir_rejects_file_path() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("game.exe");
        std::fs::write(&file, b"not a directory").unwrap();
        std::fs::create_dir(root.path().join("Data")).unwrap();
        assert_data_dir(&file, None);
    }

    #[test]
    fn capital_data_dir_rejects_data_file() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("Data"), b"not a directory").unwrap();
        assert_data_dir(root.path(), None);
    }

    #[test]
    fn capital_data_dir_uses_host_case_sensitivity() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("data")).unwrap();
        // The old helpers probe the literal "Data", not a case-folded path.
        // Whether it resolves to "data" depends on the host filesystem.
        let data = root.path().join("Data");
        let expected = data.is_dir().then_some(data);
        assert_data_dir(root.path(), expected);
    }

    #[test]
    fn recovery_discovery_prunes_only_exact_internal_directories() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            ".locust",
            ".locust-injections",
            ".locust-user",
            ".locust-injections-old",
            "files",
        ] {
            std::fs::create_dir(root.path().join(name)).unwrap();
            std::fs::write(root.path().join(name).join("sentinel"), name).unwrap();
        }
        // A same-named ordinary file is not a recovery directory.
        std::fs::write(root.path().join("files/.locust"), b"user file").unwrap();
        let mut visible: Vec<_> = walkdir::WalkDir::new(root.path())
            .into_iter()
            .filter_entry(is_game_entry)
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| {
                entry
                    .path()
                    .strip_prefix(root.path())
                    .unwrap()
                    .to_path_buf()
            })
            .collect();
        visible.sort();
        let mut expected: Vec<std::path::PathBuf> = [
            ".locust-user/sentinel",
            ".locust-injections-old/sentinel",
            "files/sentinel",
            "files/.locust",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        expected.sort();
        assert_eq!(visible, expected);
        assert!(is_internal_directory_name(OsStr::new(".locust")));
        assert!(is_internal_directory_name(OsStr::new(".locust-injections")));
        assert!(!is_internal_directory_name(OsStr::new(".locust-injection")));
        assert_eq!(
            is_internal_directory_name(OsStr::new(".LOCUST")),
            cfg!(windows)
        );
    }
}
