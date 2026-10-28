use std::io::{self, Cursor, Read, Seek, SeekFrom};

use locust_formats::godot_pck::{self, read_index, read_member};
use md5::{Digest, Md5};

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn wide(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

// Independent byte fixture: never calls the production reader or writer.
// Both profiles have nonzero file_base, opaque reserved words, and real MD5s.
fn assemble(version: u32, members: &[(&str, &[u8])]) -> Vec<u8> {
    let header = if version == 3 { 104 } else { 100 };
    let record_bytes: usize = members
        .iter()
        .map(|(p, _)| 40 + p.len().div_ceil(4) * 4)
        .sum();
    let base = if version == 3 {
        128
    } else {
        (100 + record_bytes).div_ceil(32) * 32
    };
    let mut bytes = vec![0; base];
    let mut records = Vec::new();
    for (path, payload) in members {
        let offset = bytes.len() - base;
        bytes.extend_from_slice(payload);
        bytes.resize(bytes.len().div_ceil(32) * 32, 0);
        let length = path.len().div_ceil(4) * 4;
        records.extend_from_slice(&(length as u32).to_le_bytes());
        records.extend_from_slice(path.as_bytes());
        records.resize(records.len() + length - path.len(), 0);
        records.extend_from_slice(&(offset as u64).to_le_bytes());
        records.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        records.extend_from_slice(&Md5::digest(payload));
        records.extend_from_slice(&0u32.to_le_bytes());
    }
    if version == 3
        && members
            .last()
            .is_some_and(|(_, payload)| payload.is_empty())
    {
        bytes.resize(bytes.len() + 32, 0);
    }
    let directory = if version == 3 { bytes.len() } else { 100 };
    if version == 3 {
        bytes.extend_from_slice(&(members.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&records);
        put64(&mut bytes, 32, directory as u64);
    } else {
        bytes[100..100 + records.len()].copy_from_slice(&records);
        put32(&mut bytes, 96, members.len() as u32);
    }
    bytes[..4].copy_from_slice(b"GDPC");
    put32(&mut bytes, 4, version);
    put32(&mut bytes, 8, 4);
    put32(&mut bytes, 12, if version == 3 { 5 } else { 4 });
    put32(&mut bytes, 16, 99);
    put32(&mut bytes, 20, 2);
    put64(&mut bytes, 24, base as u64);
    let reserved = if version == 3 { 40 } else { 32 };
    for n in 0..16 {
        put32(&mut bytes, reserved + n * 4, 0xabcd_0000 + n as u32);
    }
    assert!(directory >= header);
    bytes
}

fn fixture(version: u32) -> Vec<u8> {
    assemble(
        version,
        &[
            (
                "story/./main.dtl",
                b"hero: Hello [b]{player}[/b]! #id:greet\n- Yes\n",
            ),
            ("res://characters/hero.dch", br#"{"display_name":"Hero"}"#),
            ("story/main.dtl.uid", b"uid://opaque123\n"),
            (
                "res://assets/image.png.remap",
                b"[remap]\npath=\"res://.godot/imported/image.ctex\"\n",
            ),
            ("assets/raw.bin", &[0, 255, 17, 33, 0]),
            ("res://other.dtl", b"Other words\n"),
        ],
    )
}

struct MetadataOnly {
    bytes: Cursor<Vec<u8>>,
    directory: u64,
    read_bytes: u64,
}

impl Read for MetadataOnly {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let start = self.bytes.position();
        assert!(
            start + buffer.len() as u64 <= 104 || start >= self.directory,
            "reader touched payload or padding"
        );
        let n = self.bytes.read(buffer)?;
        self.read_bytes += n as u64;
        Ok(n)
    }
}

impl Seek for MetadataOnly {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.bytes.seek(from)
    }
}

#[test]
fn v3_reads_only_header_and_trailing_directory_with_independent_positions() {
    let bytes = fixture(3);
    let directory = wide(&bytes, 32);
    let mut source = MetadataOnly {
        bytes: Cursor::new(bytes.clone()),
        directory,
        read_bytes: 0,
    };
    let index = read_index(&mut source).unwrap();
    assert_eq!(source.read_bytes, 104 + bytes.len() as u64 - directory);
    assert_eq!(source.bytes.position(), index.index_end);
    assert_eq!(index.directory_offset, directory);
    assert_eq!(index.index_end, bytes.len() as u64);
    assert_eq!(index.format_version, 3);
    assert_eq!(index.engine_version, [4, 5, 99]);
    assert_eq!(index.file_base, 128);
    assert_eq!(index.reserved[15], 0xabcd_000f);
    assert_eq!(index.entries.len(), 6);
    assert_eq!(index.entries[0].path, "res://story/main.dtl");
    assert_eq!(index.entries[0].path_length_position, directory + 4);
    assert_eq!(index.entries[0].path_position, directory + 8);
    assert_eq!(index.entries[0].offset, 128);
    assert_eq!(index.entries[0].stored_offset, 0);
    assert_eq!(index.entries[0].padded_path, b"story/./main.dtl");
    for e in &index.entries {
        assert_eq!(wide(&bytes, e.offset_position as usize), e.stored_offset);
        assert_eq!(wide(&bytes, e.size_position as usize), e.size);
        assert_eq!(word(&bytes, e.flags_position as usize), e.flags);
        assert!(e.offset + e.size <= directory);
        assert!(godot_pck::verify_member(&mut Cursor::new(&bytes), e).unwrap());
    }
}

#[test]
fn independent_godot_4_4_v2_fixture_authenticates_bare_and_prefixed_paths() {
    let bytes = fixture(2);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    assert_eq!(index.engine_version, [4, 4, 99]);
    assert_eq!(index.format_version, 2);
    for e in &index.entries {
        assert!(godot_pck::verify_member(&mut Cursor::new(&bytes), e).unwrap());
    }
}

fn reject(bytes: &[u8], message: &str) {
    let error = read_index(&mut Cursor::new(bytes)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error.to_string().contains(message),
        "expected {message}, got {error}"
    );
}

#[test]
fn v3_refuses_every_truncation_and_unsupported_version_profile() {
    let bytes = fixture(3);
    for end in 0..bytes.len() {
        assert!(
            read_index(&mut Cursor::new(&bytes[..end])).is_err(),
            "prefix {end}"
        );
    }
    for (at, value, expected) in [
        (4, 4, "format version"),
        (8, 3, "v3 engine"),
        (12, 4, "v3 engine"),
    ] {
        let mut bad = bytes.clone();
        put32(&mut bad, at, value);
        reject(&bad, expected);
    }
    let mut later = bytes;
    put32(&mut later, 12, 6);
    assert!(read_index(&mut Cursor::new(later)).is_ok());
}

#[test]
fn v3_refuses_directory_header_member_flags_and_sparse_bundles() {
    let bytes = fixture(3);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    for (flags, reason) in [
        (1, "encrypted directory"),
        (3, "encrypted directory"),
        (4, "sparse bundles"),
        (8, "unknown header flags"),
        (u32::MAX, "encrypted directory"),
    ] {
        let mut bad = bytes.clone();
        put32(&mut bad, 20, flags);
        reject(&bad, reason);
    }
    for bit in 0..32 {
        let mut bad = bytes.clone();
        put32(&mut bad, index.entries[0].flags_position as usize, 1 << bit);
        reject(
            &bad,
            match bit {
                0 => "encrypted member",
                1 => "removal entries",
                _ => "unknown member flags",
            },
        );
    }
    for flags in [0, 2] {
        let mut good = bytes.clone();
        put32(&mut good, 20, flags);
        let next = read_index(&mut Cursor::new(good)).unwrap();
        assert_eq!(next.entries[0].offset, 128);
    }
}

#[test]
fn v3_refuses_directory_and_payload_overflows_out_of_bounds_and_metadata_overlaps() {
    let bytes = fixture(3);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    let first = &index.entries[0];
    for (offset, reason) in [
        (0, "directory overlaps header"),
        (103, "directory overlaps header"),
        (bytes.len() as u64 - 3, "directory count"),
        (bytes.len() as u64 + 1, "directory count"),
        (u64::MAX, "directory offset overflow"),
    ] {
        let mut bad = bytes.clone();
        put64(&mut bad, 32, offset);
        reject(&bad, reason);
    }
    for (position, value, reason) in [
        (24, u64::MAX, "file base exceeds EOF"),
        (
            first.offset_position as usize,
            u64::MAX,
            "file base plus offset overflow",
        ),
        (
            first.size_position as usize,
            u64::MAX,
            "member range overflow",
        ),
        (
            first.size_position as usize,
            bytes.len() as u64,
            "exceeds EOF",
        ),
    ] {
        let mut bad = bytes.clone();
        put64(&mut bad, position, value);
        reject(&bad, reason);
    }
    for offset in [
        0,
        103,
        index.directory_offset - 1,
        index.directory_offset,
        index.index_end - 1,
    ] {
        let mut bad = bytes.clone();
        put64(&mut bad, 24, 0);
        for e in &index.entries {
            put64(&mut bad, e.offset_position as usize, e.offset);
        }
        put64(&mut bad, first.offset_position as usize, offset);
        put64(
            &mut bad,
            first.size_position as usize,
            if offset < index.directory_offset {
                2
            } else {
                0
            },
        );
        reject(&bad, "overlaps header or index");
    }
}

#[test]
fn v3_refuses_shared_nested_partial_payload_aliases_and_canonical_collisions() {
    let bytes = fixture(3);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    for (relative, size) in [
        (0, index.entries[0].size),
        (1, 2),
        (index.entries[0].size - 1, 3),
    ] {
        let mut bad = bytes.clone();
        put64(
            &mut bad,
            index.entries[1].offset_position as usize,
            relative,
        );
        put64(&mut bad, index.entries[1].size_position as usize, size);
        reject(&bad, "overlapping or shared nonempty");
    }
    for other in ["res://a", "a", "res://./a", "./a"] {
        reject(
            &assemble(3, &[("a", b"a"), (other, b"b")]),
            "duplicate canonical",
        );
    }
}

#[test]
fn v3_refuses_unsafe_paths_invalid_padding_and_utf8() {
    for path in [
        "../a",
        "/a",
        "//host/a",
        "C:/a",
        "a\\b",
        "a:b",
        "a\nb",
        "a\u{7f}",
        "a/",
        ".",
        "a/.",
        "res://",
        "res:///a",
        "user://a",
        "a\0b",
        "a\0\0\0\0",
    ] {
        assert!(
            read_index(&mut Cursor::new(assemble(3, &[(path, b"a")]))).is_err(),
            "{path:?}"
        );
    }
    let mut bytes = assemble(3, &[("a", b"a")]);
    let dir = wide(&bytes, 32) as usize;
    bytes[dir + 8] = 255;
    reject(&bytes, "strict UTF-8");
}

#[test]
fn v3_keeps_count_path_and_directory_allocation_caps() {
    let bytes = fixture(3);
    let dir = wide(&bytes, 32) as usize;
    for count in [100_001, u32::MAX, 100_000] {
        let mut bad = bytes.clone();
        put32(&mut bad, dir, count);
        reject(&bad, "entry count");
    }
    for length in [0, 1, 7, 16_388, u32::MAX, 16_384] {
        let mut bad = bytes.clone();
        put32(&mut bad, dir + 4, length);
        reject(
            &bad,
            if length == 16_384 {
                "bounded index"
            } else {
                "path length"
            },
        );
    }
    let names: Vec<_> = (0..1024)
        .map(|n| format!("{n:04}{}", "x".repeat(16_380)))
        .collect();
    let members: Vec<_> = names.iter().map(|p| (p.as_str(), b"".as_slice())).collect();
    reject(&assemble(3, &members), "bounded index");
}

#[test]
fn v3_accepts_empty_archive_and_empty_member_at_metadata_boundaries() {
    let bytes = assemble(3, &[]);
    let index = read_index(&mut Cursor::new(bytes)).unwrap();
    assert!(index.entries.is_empty());
    let bytes = assemble(3, &[("a", b""), ("b", b"payload")]);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    for position in [104, index.directory_offset - 1, index.index_end] {
        let mut changed = bytes.clone();
        put64(&mut changed, 24, 0);
        for e in &index.entries {
            put64(&mut changed, e.offset_position as usize, e.offset);
        }
        put64(
            &mut changed,
            index.entries[0].offset_position as usize,
            position,
        );
        assert!(read_index(&mut Cursor::new(changed)).is_ok());
    }
    assert_eq!(
        read_member(&mut Cursor::new(&bytes), &index.entries[0], 0).unwrap(),
        b""
    );
}

// Literal-byte oracle independent of read_index/rebuild/verify_rebuild. It also
// checks directory ordering, raw padded spellings, occupied intervals, and EOF.
fn oracle(bytes: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    assert_eq!(&bytes[..4], b"GDPC");
    assert_eq!(word(bytes, 4), 3);
    let base = wide(bytes, 24) as usize;
    let directory = wide(bytes, 32) as usize;
    assert!(directory >= 104);
    let mut position = directory + 4;
    let mut members = Vec::new();
    let mut ranges = Vec::new();
    for _ in 0..word(bytes, directory) {
        let length = word(bytes, position) as usize;
        assert_eq!(length % 4, 0);
        let raw = bytes[position + 4..position + 4 + length].to_vec();
        position += 4 + length;
        let offset = base.checked_add(wide(bytes, position) as usize).unwrap();
        let size = wide(bytes, position + 8) as usize;
        let end = offset.checked_add(size).unwrap();
        assert!(offset >= 104 && end <= directory);
        let payload = &bytes[offset..end];
        assert_eq!(
            &bytes[position + 16..position + 32],
            Md5::digest(payload).as_slice()
        );
        assert_eq!(word(bytes, position + 32), 0);
        if size != 0 {
            ranges.push(offset..end);
        }
        members.push((raw, payload.to_vec()));
        position += 36;
    }
    ranges.sort_unstable_by_key(|r| r.start);
    assert!(ranges.windows(2).all(|p| p[0].end <= p[1].start));
    assert_eq!(position, bytes.len());
    members
}

#[test]
fn v3_rebuild_grow_shrink_empty_additions_preserve_metadata_and_opaque_bytes() {
    let bytes = fixture(3);
    let original = read_index(&mut Cursor::new(&bytes)).unwrap();
    let original_members = oracle(&bytes);
    for replacement in [
        b"x".as_slice(),
        b"a much longer translated timeline that crosses several alignment boundaries".as_slice(),
        b"".as_slice(),
    ] {
        let mut output = Vec::new();
        let plan = godot_pck::rebuild(
            &mut Cursor::new(&bytes),
            &original,
            &[("res://story/main.dtl", replacement)],
            &[("res://added.dtl", b"added"), ("res://empty", b"")],
            &mut output,
        )
        .unwrap();
        godot_pck::verify_rebuild(&mut Cursor::new(&output), &plan).unwrap();
        let members = oracle(&output);
        assert_eq!(&output[4..24], &bytes[4..24]);
        assert_eq!(&output[40..104], &bytes[40..104]);
        assert_eq!(members.len(), 8);
        for (n, (raw, payload)) in original_members.iter().enumerate() {
            assert_eq!(&members[n].0, raw);
            assert_eq!(members[n].1, if n == 0 { replacement } else { payload });
        }
        assert_eq!(members[6].0, b"res://added.dtl\0");
        assert_eq!(members[7].1, b"");
        let generated = read_index(&mut Cursor::new(&output)).unwrap();
        assert_eq!(generated.file_base, 128);
        assert_eq!(generated.directory_offset, wide(&output, 32));
        assert_eq!(generated.file_size, output.len() as u64);
        for e in &generated.entries {
            assert_eq!(e.offset % 32, 0);
        }
    }
}

#[test]
fn v3_rebuild_noop_and_empty_archives_verify_independently() {
    for bytes in [fixture(3), assemble(3, &[]), assemble(3, &[("a", b"")])] {
        let index = read_index(&mut Cursor::new(&bytes)).unwrap();
        let mut output = Vec::new();
        let plan =
            godot_pck::rebuild(&mut Cursor::new(&bytes), &index, &[], &[], &mut output).unwrap();
        godot_pck::verify_rebuild(&mut Cursor::new(&output), &plan).unwrap();
        assert_eq!(oracle(&output), oracle(&bytes));
    }
}

#[test]
fn v3_independent_verification_refuses_metadata_payload_order_length_and_self_consistent_substitution(
) {
    let bytes = fixture(3);
    let original = read_index(&mut Cursor::new(&bytes)).unwrap();
    let mut output = Vec::new();
    let plan =
        godot_pck::rebuild(&mut Cursor::new(&bytes), &original, &[], &[], &mut output).unwrap();
    let index = read_index(&mut Cursor::new(&output)).unwrap();
    let first = &index.entries[0];
    for position in [
        4,
        8,
        12,
        16,
        20,
        24,
        32,
        40,
        100,
        index.directory_offset,
        first.path_length_position,
        first.path_position,
        first.offset_position,
        first.size_position,
        first.md5_position,
        first.flags_position,
        first.offset,
        index.entries[1].path_position + u64::from(index.entries[1].path_length) - 1,
    ] {
        let mut bad = output.clone();
        bad[position as usize] ^= 1;
        assert!(
            godot_pck::verify_rebuild(&mut Cursor::new(bad), &plan).is_err(),
            "at {position}"
        );
    }
    let mut bad = output.clone();
    bad[first.offset as usize] ^= 1;
    let md5 = Md5::digest(&bad[first.offset as usize..(first.offset + first.size) as usize]);
    bad[first.md5_position as usize..first.md5_position as usize + 16].copy_from_slice(&md5);
    assert!(godot_pck::verify_rebuild(&mut Cursor::new(bad), &plan).is_err());
    let mut reordered = output.clone();
    let a = first.path_length_position as usize;
    let b = index.entries[1].path_length_position as usize;
    let c = index.entries[2].path_length_position as usize;
    reordered.splice(a..c, output[b..c].iter().chain(&output[a..b]).copied());
    assert!(godot_pck::verify_rebuild(&mut Cursor::new(reordered), &plan).is_err());
    for grow in [false, true] {
        let mut bad = output.clone();
        if grow {
            bad.push(0);
        } else {
            bad.pop();
        }
        assert!(godot_pck::verify_rebuild(&mut Cursor::new(bad), &plan).is_err());
    }
}

#[test]
fn v3_rebuild_authenticates_untouched_bytes_and_refuses_stale_metadata_before_output() {
    let mut bytes = fixture(3);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    bytes[index.entries[4].offset as usize] ^= 1;
    let mut output = Vec::new();
    let error = godot_pck::rebuild(
        &mut Cursor::new(&bytes),
        &index,
        &[("res://story/main.dtl", b"replacement")],
        &[],
        &mut output,
    )
    .unwrap_err();
    assert!(error.to_string().contains("untouched member MD5"));
    assert!(!output.is_empty());
    bytes[40] ^= 1;
    let mut output = Vec::new();
    assert!(godot_pck::rebuild(&mut Cursor::new(&bytes), &index, &[], &[], &mut output).is_err());
    assert!(output.is_empty());
}

#[test]
fn maximal_bare_path_can_be_replaced_but_cannot_be_added_with_overlong_canonical_spelling() {
    let bare = format!("{}.dtl", "x".repeat(16_380));
    let canonical = format!("res://{bare}");
    for version in [2, 3] {
        let bytes = assemble(version, &[(&bare, b"Original\n")]);
        let index = read_index(&mut Cursor::new(&bytes)).unwrap();
        assert_eq!(index.entries[0].path, canonical);
        let mut output = Vec::new();
        let plan = godot_pck::rebuild(
            &mut Cursor::new(&bytes),
            &index,
            &[(&canonical, b"Translated\n")],
            &[],
            &mut output,
        )
        .unwrap();
        godot_pck::verify_rebuild(&mut Cursor::new(&output), &plan).unwrap();
        let next = read_index(&mut Cursor::new(&output)).unwrap();
        assert_eq!(next.entries[0].padded_path, bare.as_bytes());
        assert_eq!(
            read_member(&mut Cursor::new(&output), &next.entries[0], 4096).unwrap(),
            b"Translated\n"
        );
        let empty = assemble(version, &[]);
        let index = read_index(&mut Cursor::new(&empty)).unwrap();
        let mut output = Vec::new();
        assert!(godot_pck::rebuild(
            &mut Cursor::new(&empty),
            &index,
            &[],
            &[(&canonical, b"Translated\n")],
            &mut output
        )
        .is_err());
        assert!(output.is_empty());
    }
}

#[test]
fn v3_directory_before_or_between_payloads_and_unsorted_entries_rebuild_correctly() {
    let bytes = assemble(3, &[("a", b"one"), ("b", b"two")]);
    let old_directory = wide(&bytes, 32) as usize;
    let records = &bytes[old_directory..];
    for (directory, offsets) in [(104usize, [224usize, 256usize]), (160, [128, 256])] {
        let mut moved = vec![0; 288];
        moved[..104].copy_from_slice(&bytes[..104]);
        moved[directory..directory + records.len()].copy_from_slice(records);
        put64(&mut moved, 32, directory as u64);
        put64(&mut moved, 24, 0);
        for (n, offset) in offsets.into_iter().enumerate() {
            put64(&mut moved, directory + 12 + n * 44, offset as u64);
            moved[offset..offset + 3].copy_from_slice(if n == 0 { b"one" } else { b"two" });
        }
        // Directory order is deliberately unrelated to physical payload order.
        let first = moved[directory + 4..directory + 48].to_vec();
        let second = moved[directory + 48..directory + 92].to_vec();
        moved[directory + 4..directory + 48].copy_from_slice(&second);
        moved[directory + 48..directory + 92].copy_from_slice(&first);
        let index = read_index(&mut Cursor::new(&moved)).unwrap();
        assert_eq!(index.entries[0].path, "res://b");
        let mut output = Vec::new();
        let plan =
            godot_pck::rebuild(&mut Cursor::new(&moved), &index, &[], &[], &mut output).unwrap();
        godot_pck::verify_rebuild(&mut Cursor::new(&output), &plan).unwrap();
        assert_eq!(
            oracle(&output),
            vec![
                (b"b\0\0\0".to_vec(), b"two".to_vec()),
                (b"a\0\0\0".to_vec(), b"one".to_vec())
            ]
        );
    }
}

struct BoundedReader {
    bytes: Cursor<Vec<u8>>,
    file_size: u64,
    ranges: Vec<std::ops::Range<u64>>,
    read_bytes: u64,
    largest_read: usize,
}

impl Read for BoundedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let start = self.bytes.position();
        let end = start + buffer.len() as u64;
        assert!(self.ranges.iter().any(|r| r.start <= start && end <= r.end));
        assert!(buffer.len() <= 65_536);
        let n = self.bytes.read(buffer)?;
        self.read_bytes += n as u64;
        self.largest_read = self.largest_read.max(buffer.len());
        Ok(n)
    }
}

impl Seek for BoundedReader {
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
fn v3_rebuild_streams_untouched_once_and_does_not_read_replaced_payload() {
    let payload = vec![0x95; 3 * 65_536 + 7];
    let mut bytes = assemble(3, &[("a", b"replaced"), ("b", &payload)]);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    bytes[index.entries[0].offset as usize] ^= 1;
    let file_size = bytes.len() as u64;
    let entry = &index.entries[1];
    let mut source = BoundedReader {
        bytes: Cursor::new(bytes),
        file_size,
        ranges: vec![
            0..104,
            index.directory_offset..file_size,
            entry.offset..entry.offset + entry.size,
        ],
        read_bytes: 0,
        largest_read: 0,
    };
    let mut output = Vec::new();
    let plan = godot_pck::rebuild(
        &mut source,
        &index,
        &[("res://a", b"new")],
        &[],
        &mut output,
    )
    .unwrap();
    assert_eq!(
        source.read_bytes,
        104 + file_size - index.directory_offset + entry.size
    );
    assert_eq!(source.largest_read, 65_536);
    godot_pck::verify_rebuild(&mut Cursor::new(&output), &plan).unwrap();
    assert_eq!(oracle(&output)[1].1, payload);
}

#[test]
fn v3_relayout_overflow_refused_before_payload_reads_or_output() {
    let seed = assemble(3, &[("a", b"")]);
    let directory = wide(&seed, 32) as usize;
    let mut bytes = seed[..104].to_vec();
    bytes.extend_from_slice(&seed[directory..]);
    put64(&mut bytes, 32, 104);
    put64(&mut bytes, 24, 0);
    put64(&mut bytes, 116, 160);
    put64(&mut bytes, 124, u64::MAX - 160);
    let metadata_end = bytes.len() as u64;
    let mut source = BoundedReader {
        bytes: Cursor::new(bytes),
        file_size: u64::MAX,
        ranges: std::iter::once(0..metadata_end).collect(),
        read_bytes: 0,
        largest_read: 0,
    };
    let index = read_index(&mut source).unwrap();
    let mut output = Vec::new();
    let error = godot_pck::rebuild(&mut source, &index, &[], &[], &mut output).unwrap_err();
    assert!(error.to_string().contains("arithmetic overflow"), "{error}");
    assert!(output.is_empty());
    assert_eq!(source.read_bytes, 2 * metadata_end);
}

#[test]
fn v3_codec_revisions_install_strict_pack_rollback_and_reapply_before_plugin_admission() {
    use locust_core::backup::BackupManager;
    use locust_core::database::Database;
    use locust_core::models::StringEntry;
    use locust_core::patch::{
        self, ApplyOptions, GameLock, PackOptions, RollbackOptions, VerificationOutcome,
    };
    use locust_formats::dialogic;
    use std::collections::HashMap;
    use std::fs;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("game");
    let pristine = temp.path().join("pristine");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&pristine).unwrap();
    let path = root.join("game.pck");
    let original = fixture(3);
    fs::write(&path, &original).unwrap();
    fs::write(pristine.join("game.pck"), &original).unwrap();
    fs::write(root.join("sibling.bin"), b"foreign bytes").unwrap();
    let index = read_index(&mut Cursor::new(&original)).unwrap();
    let timeline = read_member(&mut Cursor::new(&original), &index.entries[0], 4096).unwrap();
    let mut characters = dialogic::CharacterSet::default();
    let character = read_member(&mut Cursor::new(&original), &index.entries[1], 4096).unwrap();
    dialogic::parse_character(std::str::from_utf8(&character).unwrap()).unwrap();
    characters.insert_filename(&index.entries[1].path).unwrap();
    let text = std::str::from_utf8(&timeline).unwrap();
    let parsed = dialogic::parse_timeline(text, &characters).unwrap();
    let original_members = oracle(&original);
    let mut previous = original.clone();
    for translation in [
        "Primera [b]{player}[/b]!",
        "Una revisión mucho más larga que crece hasta varios bloques [b]{player}[/b]!",
        "C [b]{player}[/b]!",
    ] {
        let span = &parsed.spans[0];
        let edits = HashMap::from([(
            span.id.clone(),
            span.encode_literal_translation(translation).unwrap(),
        )]);
        let replacement = dialogic::rebuild(text, &parsed, &edits).unwrap();
        let lock = GameLock::acquire(&root).unwrap();
        let backup = godot_pck::replace_archive(
            &lock,
            &path,
            &[("res://story/main.dtl", replacement.as_bytes())],
            &[],
        )
        .unwrap();
        drop(lock);
        assert_eq!(fs::read(backup).unwrap(), previous);
        previous = fs::read(&path).unwrap();
        let members = oracle(&previous);
        assert_eq!(members[0].1, replacement.as_bytes());
        assert_eq!(&members[1..], &original_members[1..]);
    }
    let db = Database::open_in_memory().unwrap();
    let mut row = StringEntry::new("v3-codec", "Hello", path.clone());
    row.translation = Some("C".into());
    row.status = locust_core::models::StringStatus::Translated;
    db.save_entries(&[row]).unwrap();
    db.record_injection_with_backup(Some("es"), &root, std::slice::from_ref(&path), None)
        .unwrap();
    let zip = temp.path().join("strict.zip");
    let options = PackOptions {
        game_path: root.clone(),
        lang: Some("es".into()),
        output: zip.clone(),
        pristine: Some(pristine),
        engine: Some("godot-dialogic".into()),
        project: temp.path().join("project.db"),
        require_pristine: true,
        game: None,
    };
    let store = BackupManager::new(temp.path().join("backups"));
    let report = patch::pack_with_pristine_backup(&db, options, &store, None, true).unwrap();
    assert_eq!(report.tier, "strict");
    assert_eq!(report.files_packed, 1);
    let target = temp.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("game.pck"), &original).unwrap();
    fs::write(target.join("sibling.bin"), b"foreign bytes").unwrap();
    assert_eq!(
        patch::verify(&target, &zip).unwrap().outcome,
        VerificationOutcome::Clean
    );
    let applied = patch::apply(&target, &zip, ApplyOptions::default(), |_| {}).unwrap();
    assert_eq!((applied.replaced, applied.added), (1, 0));
    assert_eq!(fs::read(target.join("game.pck")).unwrap(), previous);
    assert_eq!(
        patch::verify(&target, &zip).unwrap().outcome,
        VerificationOutcome::AlreadyApplied
    );
    assert!(patch::apply(&target, &zip, ApplyOptions::default(), |_| {}).is_err());
    assert_eq!(
        patch::rollback(&target, RollbackOptions::default())
            .unwrap()
            .restored,
        1
    );
    assert_eq!(fs::read(target.join("game.pck")).unwrap(), original);
    assert_eq!(
        patch::verify(&target, &zip).unwrap().outcome,
        VerificationOutcome::Clean
    );
    patch::apply(&target, &zip, ApplyOptions::default(), |_| {}).unwrap();
    assert_eq!(fs::read(target.join("game.pck")).unwrap(), previous);
    assert_eq!(
        fs::read(target.join("sibling.bin")).unwrap(),
        b"foreign bytes"
    );
}

#[tokio::test]
async fn independent_v2_4_4_and_v3_fixtures_plugin_replace_direct_revisions_and_noop() {
    use locust_core::backup::BackupManager;
    use locust_core::database::Database;
    use locust_core::extraction::{inject_direct, FormatPlugin, FormatRegistry, MultiLangInjector};
    use locust_core::models::{OutputMode, StringStatus};
    use locust_formats::godot_dialogic::GodotDialogicPlugin;
    use std::{fs, sync::Arc};

    for version in [2, 3] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir(&root).unwrap();
        let path = root.join("game.pck");
        let original = fixture(version);
        fs::write(&path, &original).unwrap();
        fs::write(root.join("sibling.bin"), b"foreign").unwrap();
        let plugin = GodotDialogicPlugin;
        assert!(plugin.detect(&path));
        plugin.validate_source(&path).unwrap();
        let extraction = plugin.extract_with_diagnostics(&path).unwrap();
        assert!(extraction.complete);
        assert!(extraction.warnings.is_empty());
        let mut rows = extraction.entries;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].source, "Hello [b]{player}[/b]!");
        assert_eq!(
            rows[0].metadata["godot_dialogic"]["member"],
            "res://story/main.dtl"
        );
        let noop = plugin.inject(&path, &rows).unwrap();
        assert_eq!(noop.strings_written, 0);
        assert_eq!(fs::read(&path).unwrap(), original);
        rows[0].translation = Some("Un saludo considerablemente más largo [b]{player}[/b]!".into());
        rows[1].translation = Some("Sí".into());
        for row in &mut rows {
            if row.translation.is_some() {
                row.status = StringStatus::Translated;
            }
        }
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.save_entries(&rows).unwrap();
        let mut registry = FormatRegistry::new();
        registry.register(Box::new(plugin));
        let registry = Arc::new(registry);
        let store = Arc::new(BackupManager::new(temp.path().join("backups")));
        let injector = MultiLangInjector::new(registry.clone(), db.clone(), store.clone());
        let output = temp.path().join("output");
        let (tx, _rx) = tokio::sync::mpsc::channel(100);
        let report = injector
            .inject(
                &path,
                "godot-dialogic",
                OutputMode::Replace,
                vec!["es".into()],
                Some(output),
                tx,
            )
            .await
            .unwrap();
        assert!(report.languages_failed.is_empty(), "{report:?}");
        let replaced = report.injected_roots["es"].join("game.pck");
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(
            fs::read_dir(replaced.parent().unwrap())
                .unwrap()
                .filter(|entry| entry.as_ref().unwrap().file_type().unwrap().is_file())
                .count(),
            1
        );
        let first = inject_direct(
            &registry,
            &db,
            &store,
            &path,
            "godot-dialogic",
            &["es".into()],
        )
        .unwrap();
        assert_eq!(first.strings_written, 2);
        let translated = fs::read(&path).unwrap();
        assert_eq!(translated, fs::read(replaced).unwrap());
        let index = read_index(&mut Cursor::new(&translated)).unwrap();
        assert_eq!(index.format_version, version);
        assert_eq!(&translated[4..24], &original[4..24]);
        let reserved = if version == 3 { 40 } else { 32 };
        assert_eq!(
            &translated[reserved..reserved + 64],
            &original[reserved..reserved + 64]
        );
        let backup = std::path::Path::new(first.backup_path.as_ref().unwrap()).join("payload/file");
        assert_eq!(fs::read(&backup).unwrap(), original);
        let provenance = db
            .get_injection(Some("es"))
            .unwrap()
            .unwrap()
            .pristine_backup;
        for value in [
            "Otra revisión todavía más larga para variar el tamaño [b]{player}[/b]!",
            "C [b]{player}[/b]!",
        ] {
            rows[0].translation = Some(value.into());
            rows[1].translation = None;
            rows[1].status = StringStatus::Pending;
            db.save_entries(&rows).unwrap();
            let report = inject_direct(
                &registry,
                &db,
                &store,
                &path,
                "godot-dialogic",
                &["es".into()],
            )
            .unwrap();
            assert_eq!(report.strings_written, 1);
            let current = fs::read(&path).unwrap();
            let index = read_index(&mut Cursor::new(&current)).unwrap();
            assert_eq!(
                read_member(&mut Cursor::new(&current), &index.entries[0], 4096).unwrap(),
                format!("hero: {value} #id:greet\n- Sí\n").as_bytes()
            );
            if version == 3 {
                let members = oracle(&current);
                let before = oracle(&original);
                assert_eq!(&members[1..], &before[1..]);
            }
            assert_eq!(
                db.get_injection(Some("es"))
                    .unwrap()
                    .unwrap()
                    .pristine_backup,
                provenance
            );
        }
        let before = fs::read(&path).unwrap();
        let backups = store.list_backups().unwrap().len();
        let report = inject_direct(
            &registry,
            &db,
            &store,
            &path,
            "godot-dialogic",
            &["es".into()],
        )
        .unwrap();
        assert_eq!(report.strings_written, 0);
        assert!(report.files_written.is_empty());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(store.list_backups().unwrap().len(), backups);
        assert_eq!(fs::read(&backup).unwrap(), original);
        assert_eq!(fs::read(root.join("sibling.bin")).unwrap(), b"foreign");
    }
}
