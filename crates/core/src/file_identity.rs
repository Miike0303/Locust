//! Physical file identities shared by filesystem readers and writers.
use std::{fs::File, io};

/// An open file's filesystem and file identifier, independent of its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileIdentity {
    volume: u64,
    index: u64,
}

// Keep the original file handle alive so its identifier cannot be reused if
// another process moves/deletes it and installs an unrelated file at its name.
pub(crate) fn same_file(left: &File, right: &File) -> bool {
    identity(left)
        .ok()
        .zip(identity(right).ok())
        .is_some_and(|(a, b)| a == b)
}

/// Identify an open file. Keep its handle open while comparing identities so a
/// deleted file's identifier cannot be reused for an unrelated file.
pub fn identity(file: &File) -> io::Result<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        Ok(FileIdentity {
            volume: metadata.dev(),
            index: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        let info = information(file)?;
        Ok(FileIdentity {
            volume: u64::from(info.volume),
            index: (u64::from(info.index_high) << 32) | u64::from(info.index_low),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file identity is unavailable",
        ))
    }
}

#[cfg(windows)]
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

#[cfg(windows)]
fn information(file: &File) -> io::Result<FileInformation> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            info: *mut FileInformation,
        ) -> i32;
    }
    let mut info = FileInformation::default();
    // SAFETY: the handle remains open; the initialized, exclusively borrowed
    // output struct matches Win32 BY_HANDLE_FILE_INFORMATION.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

#[cfg(all(test, any(unix, windows)))]
pub(crate) fn link_count(file: &File) -> io::Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink())
    }
    #[cfg(windows)]
    {
        Ok(u64::from(information(file)?.links))
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
