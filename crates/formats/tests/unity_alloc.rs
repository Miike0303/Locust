//! Counts allocations in the in-memory paths, after the caller loads the input.
//! Thread-local counters isolate these measurements from the test harness.
use locust_core::extraction::InjectionReport;
use locust_formats::unity::UnityPlugin;
use locust_formats::unity_serialized::CLASS_ID_TEXT_ASSET;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::Path;

#[path = "support/unity_fixture.rs"]
mod fixture;

struct CountingAllocator;

thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
    static LARGEST: Cell<usize> = const { Cell::new(0) };
    static FULL_SIZE: Cell<usize> = const { Cell::new(usize::MAX) };
    static FULL_COUNT: Cell<usize> = const { Cell::new(0) };
}

fn record(size: usize) {
    MEASURING.with(|active| {
        if active.get() {
            LARGEST.with(|largest| largest.set(largest.get().max(size)));
            if FULL_SIZE.with(|limit| size >= limit.get()) {
                FULL_COUNT.with(|count| count.set(count.get() + 1));
            }
        }
    });
}

// SAFETY: Every operation forwards its pointer/layout unchanged to System.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn measured<T>(asset_size: usize, run: impl FnOnce() -> T) -> (T, usize, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            MEASURING.with(|active| active.set(false));
        }
    }
    LARGEST.with(|largest| largest.set(0));
    FULL_SIZE.with(|limit| limit.set(asset_size));
    FULL_COUNT.with(|count| count.set(0));
    MEASURING.with(|active| active.set(true));
    let guard = Reset;
    let result = run();
    drop(guard);
    (result, LARGEST.with(Cell::get), FULL_COUNT.with(Cell::get))
}

fn large_asset() -> Vec<u8> {
    // A small translatable TextAsset plus an opaque 4 MiB neighboring object.
    // This keeps returned strings small, so an asset-sized allocation is a copy.
    fixture::write_v17_fixture_ex(
        "UI",
        "Welcome traveler!",
        Some(("Padding", &"x".repeat(4 * 1024 * 1024))),
    )
}

#[test]
fn extraction_borrows_the_asset_buffer() {
    let bytes = large_asset();
    let (entries, largest, full) = measured(bytes.len(), || {
        UnityPlugin::extract_strings_from_assets(&bytes, "test.assets", Path::new("test.assets"))
    });
    assert!(entries.iter().any(|e| e.source == "Welcome traveler!"));
    eprintln!(
        "extract: asset={}, largest={largest}, full-size allocations={full}",
        bytes.len()
    );
    assert!(
        largest < bytes.len(),
        "extraction allocated {largest} bytes for a {} byte asset",
        bytes.len()
    );
}

#[test]
fn fixed_slot_injection_borrows_the_asset_buffer() {
    let mut bytes = large_asset();
    let mut entries =
        UnityPlugin::extract_strings_from_assets(&bytes, "test.assets", Path::new("test.assets"));
    let entry = entries
        .iter_mut()
        .find(|e| e.source == "Welcome traveler!")
        .unwrap();
    entry.translation = Some("Hello traveler!".into());
    // Exercise the existing fixed-slot compatibility path, without relayout.
    entry.metadata.remove("textasset_rewrite");
    let mut report = report();
    let (modified, largest, full) = measured(bytes.len(), || {
        UnityPlugin::inject_serialized_bytes(&mut bytes, &[entry], "test.assets", &mut report)
    });
    assert!(modified);
    assert_eq!(report.strings_written, 1);
    eprintln!(
        "fixed inject: asset={}, largest={largest}, full-size allocations={full}",
        bytes.len()
    );
    assert!(
        largest < bytes.len(),
        "fixed injection allocated {largest} bytes for a {} byte asset",
        bytes.len()
    );
}

#[test]
fn large_textasset_extraction_does_not_copy_the_container() {
    let bytes = fixture::write_v17_fixture("Payload", &"x".repeat(3 * 1024 * 1024));
    let (_, largest, full) = measured(bytes.len(), || {
        UnityPlugin::extract_strings_from_assets(&bytes, "test.assets", Path::new("test.assets"))
    });
    eprintln!(
        "large TextAsset: asset={}, largest={largest}, full-size allocations={full}",
        bytes.len()
    );
    assert!(largest < bytes.len());
}

#[test]
fn structural_injection_allocates_only_the_rebuilt_output() {
    let mut bytes = large_asset();
    let mut entries =
        UnityPlugin::extract_strings_from_assets(&bytes, "test.assets", Path::new("test.assets"));
    let entry = entries
        .iter_mut()
        .find(|e| e.source == "Welcome traveler!")
        .unwrap();
    entry.translation = Some("A much longer welcome for every traveler!".into());
    let mut report = report();
    let size = bytes.len();
    let (modified, largest, full) = measured(size, || {
        UnityPlugin::inject_serialized_bytes(&mut bytes, &[entry], "test.assets", &mut report)
    });
    assert!(modified);
    assert_eq!(report.strings_written, 1);
    eprintln!("resize inject: asset={size}, largest={largest}, full-size allocations={full}");
    assert_eq!(
        full, 1,
        "only the rebuilt output may allocate an asset-sized buffer"
    );
}

#[test]
fn parser_borrows_slices_and_moves_owned_buffers() {
    use locust_formats::unity_serialized::SerializedFile;
    use std::borrow::Cow;
    let bytes = large_asset();
    let ptr = bytes.as_ptr();
    let borrowed = SerializedFile::parse(bytes.as_slice(), "test.assets").unwrap();
    assert!(matches!(borrowed.data, Cow::Borrowed(_)));
    assert_eq!(borrowed.data.as_ptr(), ptr);
    let borrowed_text = borrowed.read_text_asset(1).unwrap();
    drop(borrowed);
    let owned = SerializedFile::parse(bytes, "test.assets").unwrap();
    assert!(matches!(owned.data, Cow::Owned(_)));
    assert_eq!(owned.data.as_ptr(), ptr);
    assert_eq!(
        owned.read_text_asset(1).unwrap().script,
        borrowed_text.script
    );
    let unchanged = owned.rewrite_text_assets(&Default::default()).unwrap();
    assert_eq!(unchanged.as_ptr(), ptr);
}

#[test]
fn unityfs_decodes_into_one_output_buffer() {
    use locust_formats::unity_fs::UnityFsArchive;
    let asset = large_asset();
    for compression in [0u16, 2, 3] {
        let bundle = single_block_bundle(&asset, compression);
        let (archive, largest, full) = measured(asset.len(), || {
            UnityFsArchive::parse(bundle, "data.unity3d").unwrap()
        });
        assert_eq!(archive.node_bytes(&archive.nodes[0]).unwrap(), asset);
        eprintln!(
            "UnityFS compression={compression}: largest={largest}, full-size allocations={full}"
        );
        assert_eq!(
            full, 1,
            "only decoded storage should allocate a full buffer"
        );
    }
}

fn single_block_bundle(asset: &[u8], compression: u16) -> Vec<u8> {
    let storage = if compression == 0 {
        asset.to_vec()
    } else {
        lz4_flex::compress(asset)
    };
    let mut info = vec![0; 16];
    info.extend_from_slice(&1u32.to_be_bytes());
    info.extend_from_slice(&(asset.len() as u32).to_be_bytes());
    info.extend_from_slice(&(storage.len() as u32).to_be_bytes());
    info.extend_from_slice(&compression.to_be_bytes());
    info.extend_from_slice(&1u32.to_be_bytes());
    info.extend_from_slice(&0i64.to_be_bytes());
    info.extend_from_slice(&(asset.len() as i64).to_be_bytes());
    info.extend_from_slice(&4u32.to_be_bytes());
    info.extend_from_slice(b"main.assets\0");
    let mut out = b"UnityFS\0".to_vec();
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(b"5.x.x\0test\0");
    let size_at = out.len();
    out.extend_from_slice(&0i64.to_be_bytes());
    out.extend_from_slice(&(info.len() as u32).to_be_bytes());
    out.extend_from_slice(&(info.len() as u32).to_be_bytes());
    out.extend_from_slice(&0x40u32.to_be_bytes());
    out.extend_from_slice(&info);
    out.extend_from_slice(&storage);
    let size = out.len() as i64;
    out[size_at..size_at + 8].copy_from_slice(&size.to_be_bytes());
    out
}

fn report() -> InjectionReport {
    InjectionReport {
        skip_reasons: Default::default(),
        files_modified: 0,
        strings_written: 0,
        strings_skipped: 0,
        warnings: Vec::new(),
        files_written: Vec::new(),
    }
}

#[test]
fn multi_object_injection_matches_head() {
    let mut bytes = fixture::write_v17_fixture("UI", "Welcome traveler!");
    let mut entries =
        UnityPlugin::extract_strings_from_assets(&bytes, "test.assets", Path::new("test.assets"));
    let entry = entries
        .iter_mut()
        .find(|e| e.source == "Welcome traveler!")
        .unwrap();
    entry.translation = Some("A much longer welcome for every traveler!".into());
    let mut report = report();
    assert!(UnityPlugin::inject_serialized_bytes(
        &mut bytes,
        &[entry],
        "test.assets",
        &mut report
    ));
    assert_eq!(report.strings_written, 1);
    // Captured from HEAD before removing any parser copies.
    let expected: &[u8] = &[
        0, 0, 0, 112, 0, 0, 0, 240, 0, 0, 0, 17, 0, 0, 0, 144, 0, 0, 0, 0, 50, 48, 49, 57, 46, 52,
        46, 48, 102, 49, 0, 1, 0, 0, 0, 0, 2, 0, 0, 0, 49, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 56, 0, 0, 0, 0, 0, 0, 0,
        2, 0, 0, 0, 0, 0, 0, 0, 56, 0, 0, 0, 40, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 2, 0, 0, 0, 85, 73, 0, 0, 41, 0, 0, 0, 65, 32, 109, 117, 99, 104, 32, 108, 111, 110,
        103, 101, 114, 32, 119, 101, 108, 99, 111, 109, 101, 32, 102, 111, 114, 32, 101, 118, 101,
        114, 121, 32, 116, 114, 97, 118, 101, 108, 101, 114, 33, 0, 0, 0, 5, 0, 0, 0, 68, 117, 109,
        109, 121, 0, 0, 0, 21, 0, 0, 0, 110, 111, 116, 32, 97, 32, 116, 101, 120, 116, 32, 97, 115,
        115, 101, 116, 32, 98, 111, 100, 121, 0, 0, 0,
    ];
    assert_eq!(bytes, expected);
}
