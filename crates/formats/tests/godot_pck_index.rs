use std::io::{self, Cursor, Read, Seek, SeekFrom};

use locust_formats::godot_pck::read_index;

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

// Independent literal layout: header 100, two 48-byte records, base 208.
// Paths are eight bytes (no terminator needed); each record has 36 trailing bytes.
fn two_members() -> Vec<u8> {
    let mut bytes = vec![0; 224];
    bytes[..4].copy_from_slice(b"GDPC");
    put32(&mut bytes, 4, 2);
    put32(&mut bytes, 8, 4);
    put32(&mut bytes, 12, 3);
    put64(&mut bytes, 24, 208);
    put32(&mut bytes, 96, 2);
    put32(&mut bytes, 100, 8);
    bytes[104..112].copy_from_slice(b"res://aa");
    put64(&mut bytes, 112, 0);
    put64(&mut bytes, 120, 3);
    bytes[128..144].copy_from_slice(&[0x12; 16]);
    put32(&mut bytes, 148, 8);
    bytes[152..160].copy_from_slice(b"res://bb");
    put64(&mut bytes, 160, 8);
    put64(&mut bytes, 168, 8);
    bytes[176..192].copy_from_slice(&[0x34; 16]);
    bytes[208..211].copy_from_slice(b"one");
    bytes[216..224].copy_from_slice(b"second!!");
    bytes
}

#[test]
fn reads_multiple_members_with_independent_field_positions() {
    let mut reader = Cursor::new(two_members());
    let index = read_index(&mut reader).expect("valid standalone v2 index");
    assert_eq!(index.file_size, 224);
    assert_eq!(index.format_version, 2);
    assert_eq!(index.engine_version, [4, 3, 0]);
    assert_eq!(index.flags, 0);
    assert_eq!(index.file_base, 208);
    assert_eq!(index.reserved, [0; 16]);
    assert_eq!(index.index_end, 196);
    assert_eq!(reader.position(), 196);
    assert_eq!(index.entries.len(), 2);
    let first = &index.entries[0];
    assert_eq!(first.path, "res://aa");
    assert_eq!(first.path_length_position, 100);
    assert_eq!(first.path_position, 104);
    assert_eq!(first.path_length, 8);
    assert_eq!(first.offset_position, 112);
    assert_eq!(first.size_position, 120);
    assert_eq!(first.md5_position, 128);
    assert_eq!(first.flags_position, 144);
    assert_eq!(first.stored_offset, 0);
    assert_eq!(first.offset, 208);
    assert_eq!(first.size, 3);
    assert_eq!(first.stored_md5, [0x12; 16]);
    assert_eq!(first.flags, 0);
    let second = &index.entries[1];
    assert_eq!(second.path, "res://bb");
    assert_eq!(second.path_length_position, 148);
    assert_eq!(second.offset_position, 160);
    assert_eq!(second.stored_offset, 8);
    assert_eq!(second.offset, 216);
    assert_eq!(second.size, 8);
    assert_eq!(second.stored_md5, [0x34; 16]);
}

#[test]
fn refuses_unsupported_pack_version() {
    let mut bytes = two_members();
    put32(&mut bytes, 4, 3);
    let error = read_index(&mut Cursor::new(bytes)).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("format version"));
}

fn reject(bytes: Vec<u8>, diagnostic: &str) {
    let error = read_index(&mut Cursor::new(bytes)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains(diagnostic),
        "expected {diagnostic}, got {error}"
    );
}

fn single_path(raw_path: &[u8]) -> Vec<u8> {
    let padded_len = raw_path.len().div_ceil(4) * 4;
    let index_end = 140 + padded_len;
    let mut bytes = two_members();
    bytes.resize(index_end + 4, 0);
    bytes[100..].fill(0);
    put32(&mut bytes, 96, 1);
    put64(&mut bytes, 24, index_end as u64);
    put32(&mut bytes, 100, padded_len as u32);
    bytes[104..104 + raw_path.len()].copy_from_slice(raw_path);
    put64(&mut bytes, 112 + padded_len, 4);
    bytes
}

#[test]
fn refuses_every_truncated_header_and_index_prefix() {
    let bytes = two_members();
    for end in 0..196 {
        assert!(read_index(&mut Cursor::new(&bytes[..end])).is_err());
    }
}

#[test]
fn refuses_executable_embedded_and_wrong_endian_headers() {
    let mut exe = two_members();
    exe[..4].copy_from_slice(b"MZ\0\0");
    reject(exe, "offset zero");
    let mut embedded = vec![0; 128];
    embedded.extend(two_members());
    reject(embedded, "embedded/executable");
    let mut reversed = two_members();
    reversed[..4].copy_from_slice(b"CPDG");
    reject(reversed, "magic");
    let mut big_endian_version = two_members();
    big_endian_version[4..8].copy_from_slice(&2u32.to_be_bytes());
    reject(big_endian_version, "format version");
}

#[test]
fn bounds_engine_versions_but_does_not_invent_a_patch_limit() {
    for (major, minor) in [(0, 0), (3, 6), (4, 4), (4, u32::MAX), (5, 0)] {
        let mut bytes = two_members();
        put32(&mut bytes, 8, major);
        put32(&mut bytes, 12, minor);
        reject(bytes, "engine version");
    }
    for minor in 0..=3 {
        let mut bytes = two_members();
        put32(&mut bytes, 12, minor);
        put32(&mut bytes, 16, u32::MAX);
        let index = read_index(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(index.engine_version, [4, minor, u32::MAX]);
    }
}

#[test]
fn refuses_encrypted_and_unknown_header_flags() {
    for flags in [1, 3, 5, u32::MAX] {
        let mut bytes = two_members();
        put32(&mut bytes, 20, flags);
        reject(bytes, "encrypted directory");
    }
    for bit in 2..32 {
        let mut bytes = two_members();
        put32(&mut bytes, 20, 1 << bit);
        reject(bytes, "unknown header flags");
    }
}

#[test]
fn relative_filebase_flag_is_identity_at_standalone_start() {
    let mut bytes = two_members();
    put32(&mut bytes, 20, 2);
    let mut reader = Cursor::new(bytes);
    reader.set_position(160); // API deliberately starts at zero, not here.
    let index = read_index(&mut reader).unwrap();
    assert_eq!(index.flags, 2);
    assert_eq!(index.file_base, 208);
    assert_eq!(index.entries[0].offset, 208);
    assert_eq!(index.entries[1].offset, 216);
}

#[test]
fn preserves_nonzero_reserved_words_as_opaque_primary_fields() {
    let mut bytes = two_members();
    for word in 0..16 {
        put32(&mut bytes, 32 + word * 4, 0xfedc_0000 + word as u32);
    }
    let index = read_index(&mut Cursor::new(bytes)).unwrap();
    for word in 0..16 {
        assert_eq!(index.reserved[word], 0xfedc_0000 + word as u32);
    }
}

#[test]
fn refuses_entry_counts_before_reading_any_record() {
    for count in [100_001, u32::MAX, 0x8000_0000] {
        let mut bytes = two_members();
        put32(&mut bytes, 96, count);
        let mut reader = Cursor::new(bytes);
        let error = read_index(&mut reader).unwrap_err();
        assert!(error.to_string().contains("entry count"));
        assert_eq!(reader.position(), 100);
    }
    let mut bytes = two_members();
    put32(&mut bytes, 96, 3);
    reject(bytes, "entry count cannot fit");
}

#[test]
fn checks_path_allocation_bounds_before_reading_path_bytes() {
    for length in [0, 1, 7, 9, 16_388, u32::MAX, 0xffff_fffc] {
        let mut bytes = two_members();
        put32(&mut bytes, 100, length);
        let mut reader = Cursor::new(bytes);
        let error = read_index(&mut reader).unwrap_err();
        assert!(error.to_string().contains("path length"));
        assert_eq!(reader.position(), 104);
    }
    // Length is within the per-path cap, but cannot fit the file/remaining entry.
    for length in [40, 16_384] {
        let mut bytes = two_members();
        put32(&mut bytes, 100, length);
        let mut reader = Cursor::new(bytes);
        let error = read_index(&mut reader).unwrap_err();
        assert!(error.to_string().contains("bounded index"));
        assert_eq!(reader.position(), 104);
    }
}

#[test]
fn accepts_path_at_cap_and_refuses_next_aligned_size() {
    let path = format!("res://{}", "x".repeat(16_378));
    let index = read_index(&mut Cursor::new(single_path(path.as_bytes()))).unwrap();
    assert_eq!(index.entries[0].path.len(), 16_384);
    assert_eq!(index.entries[0].path_length, 16_384);
    let over = format!("{path}xxxx");
    reject(single_path(over.as_bytes()), "path length");
}

#[test]
fn accepts_zero_to_three_padding_bytes_and_unicode_names() {
    for name in [
        "res://a",
        "res://ab",
        "res://abc",
        "res://abcd",
        "res://日本語.dtl",
    ] {
        let index = read_index(&mut Cursor::new(single_path(name.as_bytes()))).unwrap();
        assert_eq!(index.entries[0].path, name);
    }
}

#[test]
fn refuses_interior_nul_excess_padding_and_invalid_utf8() {
    reject(single_path(b"res://a\0b"), "padding or interior NUL");
    reject(single_path(b"res://ab\0\0\0\0"), "padding or interior NUL");
    reject(single_path(b"res://\xff"), "strict UTF-8");
    reject(single_path(b"res://\xc0\xaf"), "strict UTF-8");
    reject(single_path(b"res://\xed\xa0\x80"), "strict UTF-8");
    reject(single_path(b"res://\xe2\x82"), "strict UTF-8");
}

#[test]
fn refuses_host_paths_traversal_controls_and_directory_names() {
    for name in [
        "",
        "a.dtl",
        "/a",
        "C:/a",
        "C:\\a",
        "//host/share/a",
        "\\\\host\\a",
        "user://a",
        "file:///a",
        "res://",
        "res:///a",
        "res://../a",
        "res://a/../b",
        "res://a/..",
        "res://a\\b",
        "res://C:/a",
        "res://a:b",
        "res://a\nb",
        "res://a\u{7f}",
        "res://a/",
        "res://.",
        "res://a/.",
    ] {
        assert!(read_index(&mut Cursor::new(single_path(name.as_bytes()))).is_err());
    }
}

#[test]
fn canonicalizes_only_safe_redundant_components() {
    let index = read_index(&mut Cursor::new(single_path(b"res://a//./b.dtl"))).unwrap();
    assert_eq!(index.entries[0].path, "res://a/b.dtl");
    assert_eq!(index.entries[0].path_length, 16);
}

#[test]
fn refuses_duplicate_raw_and_canonical_names_but_keeps_case_distinct() {
    let mut bytes = two_members();
    bytes[152..160].copy_from_slice(b"res://aa");
    reject(bytes, "duplicate canonical");
    let mut bytes = two_members();
    bytes[104..112].copy_from_slice(b"res://a\0");
    // Grow only the second record's path from 8 to 12 bytes; payload base stays 208.
    bytes.splice(152..160, b"res://./a\0\0\0".iter().copied());
    put32(&mut bytes, 148, 12);
    reject(bytes, "duplicate canonical");
    let mut bytes = two_members();
    bytes[152..160].copy_from_slice(b"res://AA");
    assert!(read_index(&mut Cursor::new(bytes)).is_ok());
}

#[test]
fn refuses_all_member_encryption_and_unknown_flags() {
    for position in [144, 192] {
        for bit in 0..32 {
            let mut bytes = two_members();
            put32(&mut bytes, position, 1 << bit);
            reject(
                bytes,
                if bit == 0 {
                    "encrypted member"
                } else {
                    "unknown member flags"
                },
            );
        }
    }
}

#[test]
fn filebase_is_an_addend_not_an_assumed_index_boundary() {
    let mut bytes = two_members();
    put64(&mut bytes, 24, 0);
    put64(&mut bytes, 112, 208);
    put64(&mut bytes, 160, 216);
    let index = read_index(&mut Cursor::new(bytes)).unwrap();
    assert_eq!(index.file_base, 0);
    assert_eq!(index.entries[0].stored_offset, 208);
    assert_eq!(index.entries[0].offset, 208);
}

#[test]
fn refuses_filebase_range_and_addition_overflow() {
    for base in [225, u64::MAX] {
        let mut bytes = two_members();
        put64(&mut bytes, 24, base);
        reject(bytes, "file base exceeds EOF");
    }
    let mut bytes = two_members();
    put64(&mut bytes, 112, u64::MAX);
    reject(bytes, "file base plus offset overflow");
    let mut bytes = two_members();
    put64(&mut bytes, 120, u64::MAX);
    reject(bytes, "member range overflow");
}

#[test]
fn refuses_eof_and_final_index_overlap_including_empty_members() {
    let mut bytes = two_members();
    put64(&mut bytes, 168, 9);
    reject(bytes, "exceeds EOF");
    for offset in [0, 99, 100, 148, 195] {
        for size in [0, 1] {
            let mut bytes = two_members();
            put64(&mut bytes, 24, 0);
            put64(&mut bytes, 112, offset);
            put64(&mut bytes, 120, size);
            put64(&mut bytes, 160, 216);
            reject(bytes, "overlaps header or index");
        }
    }
    let mut bytes = two_members();
    put64(&mut bytes, 160, 17);
    put64(&mut bytes, 168, 0);
    reject(bytes, "exceeds EOF");
}

#[test]
fn refuses_partial_nested_and_exact_shared_nonempty_ranges() {
    for (offset, size) in [(0, 3), (1, 1), (2, 8), (0, 8)] {
        let mut bytes = two_members();
        put64(&mut bytes, 160, offset);
        put64(&mut bytes, 168, size);
        reject(bytes, "overlapping or shared nonempty");
    }
}

#[test]
fn accepts_adjacent_ranges_and_unsorted_directory_order() {
    let mut bytes = two_members();
    put64(&mut bytes, 160, 3);
    assert!(read_index(&mut Cursor::new(bytes)).is_ok());
    let mut bytes = two_members();
    put64(&mut bytes, 112, 8);
    put64(&mut bytes, 120, 8);
    put64(&mut bytes, 160, 0);
    put64(&mut bytes, 168, 3);
    let index = read_index(&mut Cursor::new(bytes)).unwrap();
    assert_eq!(index.entries[0].offset, 216);
    assert_eq!(index.entries[1].offset, 208);
}

#[test]
fn empty_members_may_share_offsets_at_index_end_inside_payload_or_eof() {
    for offset in [196, 208, 209, 224] {
        let mut bytes = two_members();
        put64(&mut bytes, 24, 0);
        put64(&mut bytes, 112, offset);
        put64(&mut bytes, 120, 0);
        put64(&mut bytes, 160, 208);
        put64(&mut bytes, 168, 3);
        assert!(read_index(&mut Cursor::new(bytes)).is_ok());
    }
    let mut bytes = two_members();
    put64(&mut bytes, 120, 0);
    put64(&mut bytes, 160, 0);
    put64(&mut bytes, 168, 0);
    assert!(read_index(&mut Cursor::new(bytes)).is_ok());
}

#[test]
fn accepts_empty_archive_without_requiring_payload_or_padding() {
    let mut bytes = two_members();
    bytes.truncate(100);
    put32(&mut bytes, 96, 0);
    put64(&mut bytes, 24, 100);
    let index = read_index(&mut Cursor::new(bytes)).unwrap();
    assert!(index.entries.is_empty());
    assert_eq!(index.index_end, 100);
    assert_eq!(index.file_size, 100);
}

// Reports a potentially huge file, but permits reading only supplied metadata.
// A payload read or full-file buffering attempt fails rather than being hidden.
struct MetadataOnly {
    bytes: Cursor<Vec<u8>>,
    file_size: u64,
    read_bytes: usize,
}

impl Read for MetadataOnly {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let position = self.bytes.position();
        if position + buffer.len() as u64 > self.bytes.get_ref().len() as u64 {
            return Err(io::Error::other("attempted read beyond available metadata"));
        }
        let read = self.bytes.read(buffer)?;
        self.read_bytes += read;
        Ok(read)
    }
}

impl Seek for MetadataOnly {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        if from == SeekFrom::End(0) {
            self.bytes.set_position(self.file_size);
            Ok(self.file_size)
        } else {
            self.bytes.seek(from)
        }
    }
}

#[test]
fn reads_only_metadata_even_for_huge_payload_and_keeps_unverified_md5() {
    let mut bytes = two_members();
    bytes.truncate(196);
    put64(&mut bytes, 168, u64::MAX - 216);
    let mut reader = MetadataOnly {
        bytes: Cursor::new(bytes),
        file_size: u64::MAX,
        read_bytes: 0,
    };
    let index = read_index(&mut reader).unwrap();
    assert_eq!(reader.read_bytes, 196);
    assert_eq!(index.file_size, u64::MAX);
    assert_eq!(index.entries[1].size, u64::MAX - 216);
    // These arbitrary MD5 bytes do not match the synthetic payload, intentionally.
    assert_eq!(index.entries[1].stored_md5, [0x34; 16]);
}

#[test]
fn bounds_total_index_before_reading_or_allocating_next_path() {
    let mut bytes = two_members();
    bytes.truncate(100);
    put32(&mut bytes, 96, 1024);
    put64(&mut bytes, 24, 32 * 1024 * 1024);
    for i in 0..1024u32 {
        bytes.extend_from_slice(&16_384u32.to_le_bytes());
        let path = format!("res://{i:04}{}", "x".repeat(16_374));
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(&[0; 36]);
    }
    let mut reader = MetadataOnly {
        bytes: Cursor::new(bytes),
        file_size: 64 * 1024 * 1024,
        read_bytes: 0,
    };
    let error = read_index(&mut reader).unwrap_err();
    assert!(error.to_string().contains("bounded index"));
    assert!(reader.read_bytes <= 100 + 16 * 1024 * 1024);
    // The rejected record has consumed its length only, not its path allocation.
    assert_eq!((reader.bytes.position() - 104) % 16_424, 0);
}

#[test]
fn source_io_errors_propagate_without_a_successful_partial_index() {
    let mut reader = MetadataOnly {
        bytes: Cursor::new(two_members()[..120].to_vec()),
        file_size: 224,
        read_bytes: 0,
    };
    assert_eq!(
        read_index(&mut reader).unwrap_err().kind(),
        io::ErrorKind::Other
    );
}

#[test]
#[ignore = "optional read-only G01 index; requires LOCUST_GODOT_PCK_FIXTURE"]
fn optional_g01_index_metadata_only() {
    let path = std::env::var_os("LOCUST_GODOT_PCK_FIXTURE")
        .expect("LOCUST_GODOT_PCK_FIXTURE must be set when explicitly running this test");
    let mut file = std::fs::File::open(path)
        .unwrap_or_else(|_| panic!("could not open optional PCK fixture read-only"));
    let index = read_index(&mut file).expect("optional fixture index must validate");
    assert_eq!(index.file_size, 327_672_864);
    assert_eq!(index.format_version, 2);
    assert_eq!(index.engine_version, [4, 3, 0]);
    assert_eq!(index.flags, 0);
    assert_eq!(index.entries.len(), 2112);
    assert_eq!(index.index_end, 100 + 220_156);
    assert_eq!(index.file_base, 220_256);
    assert_eq!(file.stream_position().unwrap(), 220_256);
    assert_eq!(
        index
            .entries
            .iter()
            .filter(|e| e.path.ends_with(".dtl"))
            .count(),
        112
    );
    assert_eq!(
        index
            .entries
            .iter()
            .filter(|e| e.path.ends_with(".dch"))
            .count(),
        7
    );
}
