use std::fs::File;

// Keep the original file handle alive so its identifier cannot be reused if
// another process moves/deletes it and installs an unrelated file at its name.
pub(crate) fn same_file(left: &File, right: &File) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.metadata()
            .ok()
            .zip(right.metadata().ok())
            .is_some_and(|(a, b)| a.dev() == b.dev() && a.ino() == b.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        #[repr(C)]
        #[derive(Default)]
        struct FileInformation {
            attributes: u32,
            creation: [u32; 2],
            access: [u32; 2],
            write: [u32; 2],
            volume: u32,
            size_high: u32,
            size_low: u32,
            links: u32,
            index_high: u32,
            index_low: u32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetFileInformationByHandle(
                handle: *mut std::ffi::c_void,
                info: *mut FileInformation,
            ) -> i32;
        }
        let mut a = FileInformation::default();
        let mut b = FileInformation::default();
        // SAFETY: both handles remain open; output structs match Win32
        // BY_HANDLE_FILE_INFORMATION and are initialized and exclusively borrowed.
        let ok = unsafe {
            GetFileInformationByHandle(left.as_raw_handle(), &mut a) != 0
                && GetFileInformationByHandle(right.as_raw_handle(), &mut b) != 0
        };
        ok && (a.volume, a.index_high, a.index_low) == (b.volume, b.index_high, b.index_low)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn assert_identity(left: &File, right: &File, expected: bool) {
        assert_eq!(same_file(left, right), expected);
        assert_eq!(same_file(right, left), expected);
    }

    #[test]
    fn same_file_matches_same_handle() {
        let root = tempfile::tempdir().unwrap();
        let file = File::create(root.path().join("original")).unwrap();
        assert_identity(&file, &file, cfg!(any(unix, windows)));
    }

    #[test]
    fn same_file_matches_cloned_handle() {
        let root = tempfile::tempdir().unwrap();
        let file = File::create(root.path().join("original")).unwrap();
        let clone = file.try_clone().unwrap();
        assert_identity(&file, &clone, cfg!(any(unix, windows)));
    }

    #[test]
    fn same_file_matches_reopened_handle() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original");
        let file = File::create(&path).unwrap();
        let reopened = File::open(&path).unwrap();
        assert_identity(&file, &reopened, cfg!(any(unix, windows)));
    }

    #[test]
    fn same_file_matches_hard_link() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original");
        let alias = root.path().join("alias");
        let file = File::create(&path).unwrap();
        fs::hard_link(&path, &alias).unwrap();
        let linked = File::open(&alias).unwrap();
        assert_identity(&file, &linked, cfg!(any(unix, windows)));
    }

    #[test]
    fn same_file_rejects_distinct_files_with_identical_contents() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join("left");
        let right = root.path().join("right");
        fs::write(&left, b"identical contents").unwrap();
        fs::write(&right, b"identical contents").unwrap();
        assert_identity(
            &File::open(left).unwrap(),
            &File::open(right).unwrap(),
            false,
        );
    }
}
