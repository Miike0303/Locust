//! Shared pruning for Locust-owned filesystem recovery namespaces.
//! Only directory names match; similarly named game resources remain eligible.
use std::ffi::OsStr;

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
