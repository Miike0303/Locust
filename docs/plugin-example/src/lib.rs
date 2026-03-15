//! Example Locust WASM plugin that extracts lines from .txt files.

use serde::{Deserialize, Serialize};
use std::alloc::{alloc, dealloc, Layout};

static METADATA: &str = r#"{"id":"txt-lines","name":"Text Lines","description":"Extracts each line from .txt files","version":"0.1.0","extensions":[".txt"],"author":"Locust Example"}"#;

// ─── Memory management ────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn locust_alloc(size: i32) -> i32 {
    let layout = Layout::from_size_align(size as usize, 1).unwrap();
    unsafe { alloc(layout) as i32 }
}

#[no_mangle]
pub extern "C" fn locust_free(ptr: i32, size: i32) {
    let layout = Layout::from_size_align(size as usize, 1).unwrap();
    unsafe { dealloc(ptr as *mut u8, layout) }
}

// ─── String helpers ────────────────────────────────────────────────────────

fn write_string(s: &str) -> i32 {
    let bytes = s.as_bytes();
    let total = 4 + bytes.len();
    let ptr = locust_alloc(total as i32);
    unsafe {
        let dst = ptr as *mut u8;
        let len_bytes = (bytes.len() as u32).to_le_bytes();
        std::ptr::copy_nonoverlapping(len_bytes.as_ptr(), dst, 4);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst.add(4), bytes.len());
    }
    ptr
}

fn read_string(ptr: i32, _len: i32) -> String {
    unsafe {
        let src = ptr as *const u8;
        let str_len = u32::from_le_bytes([
            *src,
            *src.add(1),
            *src.add(2),
            *src.add(3),
        ]) as usize;
        let slice = std::slice::from_raw_parts(src.add(4), str_len);
        String::from_utf8_lossy(slice).to_string()
    }
}

// ─── Plugin interface ──────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn locust_plugin_metadata() -> i32 {
    write_string(METADATA)
}

#[derive(Serialize, Deserialize)]
struct SimpleEntry {
    id: String,
    source: String,
    file_path: String,
}

#[no_mangle]
pub extern "C" fn locust_extract(path_ptr: i32, path_len: i32) -> i32 {
    let path_str = read_string(path_ptr, path_len);

    // Read the file and extract lines
    let content = match std::fs::read_to_string(&path_str) {
        Ok(c) => c,
        Err(_) => return write_string("[]"),
    };

    let entries: Vec<SimpleEntry> = content
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(i, line)| SimpleEntry {
            id: format!("line_{}", i),
            source: line.to_string(),
            file_path: path_str.clone(),
        })
        .collect();

    let json = serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string());
    write_string(&json)
}

#[no_mangle]
pub extern "C" fn locust_inject(
    _path_ptr: i32,
    _path_len: i32,
    _entries_ptr: i32,
    _entries_len: i32,
) -> i32 {
    let report = r#"{"files_modified":0,"strings_written":0,"strings_skipped":0,"warnings":[]}"#;
    write_string(report)
}
