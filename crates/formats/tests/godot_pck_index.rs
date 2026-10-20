use std::io::{self, Cursor, Read, Seek, SeekFrom};

use locust_formats::godot_pck::{
    read_index, read_member, rebuild, replace_archive, verify_member, verify_rebuild, MemberBytes,
};
use md5::{Digest, Md5};

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

// Independent literal layout: header 100, two 48-byte records, base 208.
// Paths are eight bytes (no terminator needed); each record has 36 trailing bytes.
fn two_members() -> Vec<u8> {
    two_members_for_engine(3)
}

fn two_members_for_engine(minor: u32) -> Vec<u8> {
    let mut bytes = vec![0; 224];
    bytes[..4].copy_from_slice(b"GDPC");
    put32(&mut bytes, 4, 2);
    put32(&mut bytes, 8, 4);
    put32(&mut bytes, 12, minor);
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

// Extend GD-1's independent fixture with real hashes, opaque header words and a
// redundant, padded spelling whose bytes a rebuild must retain verbatim.
fn authenticated_members(second: &[u8]) -> Vec<u8> {
    authenticated_members_for_engine(second, 3)
}

fn authenticated_members_for_engine(second: &[u8], minor: u32) -> Vec<u8> {
    let mut bytes = two_members_for_engine(minor);
    bytes.splice(152..160, b"res://./b/b\0".iter().copied());
    put32(&mut bytes, 148, 12);
    put64(&mut bytes, 24, 212);
    bytes.resize(220 + second.len(), 0);
    bytes[220..].copy_from_slice(second);
    put64(&mut bytes, 172, second.len() as u64);
    bytes[128..144].copy_from_slice(&Md5::digest(b"one"));
    bytes[180..196].copy_from_slice(&Md5::digest(second));
    put32(&mut bytes, 20, 2);
    put32(&mut bytes, 16, 99);
    for word in 0..16 {
        put32(&mut bytes, 32 + word * 4, 0xfedc_0000 + word as u32);
    }
    bytes
}

struct RangeReader {
    bytes: Cursor<Vec<u8>>,
    allowed: Vec<std::ops::Range<u64>>,
    read_bytes: usize,
    largest_read: usize,
}

impl Read for RangeReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let start = self.bytes.position();
        let end = start + buffer.len() as u64;
        assert!(buffer.len() <= 65536, "unbounded payload read");
        assert!(self
            .allowed
            .iter()
            .any(|r| r.start <= start && end <= r.end));
        self.largest_read = self.largest_read.max(buffer.len());
        let n = self.bytes.read(buffer)?;
        self.read_bytes += n;
        Ok(n)
    }
}

impl Seek for RangeReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.bytes.seek(from)
    }
}

#[test]
fn selective_md5_reads_exactly_the_selected_range_in_bounded_chunks() {
    let payload = vec![0x91; 3 * 65536 + 7];
    let bytes = authenticated_members(&payload);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    let entry = &index.entries[1];
    let mut reader = RangeReader {
        bytes: Cursor::new(bytes),
        allowed: std::iter::once(entry.offset..entry.offset + entry.size).collect(),
        read_bytes: 0,
        largest_read: 0,
    };
    assert!(verify_member(&mut reader, entry).unwrap());
    assert_eq!(reader.read_bytes as u64, entry.size);
    assert_eq!(reader.bytes.position(), entry.offset + entry.size);
    assert_eq!(reader.largest_read, 65536);
    reader.bytes.get_mut()[entry.offset as usize + 65536] ^= 1;
    assert!(!verify_member(&mut reader, entry).unwrap());
    assert_eq!(reader.read_bytes as u64, 2 * entry.size);
}

#[test]
fn bounded_read_returns_bytes_only_on_hash_match_and_refuses_cap_before_reading() {
    let bytes = authenticated_members(b"second!!");
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    let entry = &index.entries[0];
    let mut reader = RangeReader {
        bytes: Cursor::new(bytes),
        allowed: std::iter::once(entry.offset..entry.offset + entry.size).collect(),
        read_bytes: 0,
        largest_read: 0,
    };
    assert!(read_member(&mut reader, entry, 2).is_err());
    assert_eq!(reader.read_bytes, 0);
    assert_eq!(read_member(&mut reader, entry, 3).unwrap(), b"one");
    reader.bytes.get_mut()[entry.offset as usize] ^= 1;
    assert_eq!(
        read_member(&mut reader, entry, 3).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn selected_truncation_is_io_error_and_empty_member_hash_is_checked() {
    let bytes = authenticated_members(b"");
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    assert!(verify_member(&mut Cursor::new(&bytes), &index.entries[1]).unwrap());
    assert!(read_member(&mut Cursor::new(&bytes), &index.entries[1], 0)
        .unwrap()
        .is_empty());
    let truncated = &bytes[..index.entries[0].offset as usize + 2];
    assert_eq!(
        verify_member(&mut Cursor::new(truncated), &index.entries[0])
            .unwrap_err()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
    let mut wrong = index.entries.into_iter().nth(1).unwrap();
    wrong.stored_md5 = [0; 16];
    assert!(!verify_member(&mut Cursor::new(&bytes), &wrong).unwrap());
    assert!(read_member(&mut Cursor::new(&bytes), &wrong, 0).is_err());
}

#[test]
fn rebuild_replaces_shorter_longer_and_empty_and_appends_in_requested_order() {
    check_rebuild_replacements_and_additions(3);
}

#[test]
fn godot_4_4_rebuild_preserves_engine_bytes_and_verifies_replacements_and_additions() {
    check_rebuild_replacements_and_additions(4);
}

fn check_rebuild_replacements_and_additions(minor: u32) {
    for replacement in [
        b"x".as_slice(),
        b"a much longer replacement".as_slice(),
        b"".as_slice(),
    ] {
        let bytes = authenticated_members_for_engine(b"second!!", minor);
        let mut source = Cursor::new(&bytes);
        let original = read_index(&mut source).unwrap();
        let mut output = Vec::new();
        let plan = rebuild(
            &mut source,
            &original,
            &[("res://aa", replacement)],
            &[
                ("res://new.dtl", b"new".as_slice()),
                ("res://empty.dch", b"".as_slice()),
            ],
            &mut output,
        )
        .unwrap();
        let mut generated = Cursor::new(&output);
        verify_rebuild(&mut generated, &plan).unwrap();
        assert_eq!(&output[8..20], &bytes[8..20]);
        let index = read_index(&mut generated).unwrap();
        assert_eq!(
            index
                .entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            ["res://aa", "res://b/b", "res://new.dtl", "res://empty.dch"]
        );
        assert_eq!(index.format_version, original.format_version);
        assert_eq!(index.engine_version, original.engine_version);
        assert_eq!(index.flags, original.flags);
        assert_eq!(index.reserved, original.reserved);
        assert_eq!(index.file_base % 32, 0);
        for (number, expected) in [replacement, b"second!!", b"new", b""].iter().enumerate() {
            let entry = &index.entries[number];
            assert_eq!(entry.offset % 32, 0);
            assert_eq!(entry.stored_offset + index.file_base, entry.offset);
            assert_eq!(entry.size, expected.len() as u64);
            assert_eq!(entry.stored_md5, <[u8; 16]>::from(Md5::digest(expected)));
            assert_eq!(read_member(&mut generated, entry, 100).unwrap(), *expected);
        }
        for (old, new) in original.entries.iter().zip(&index.entries) {
            assert_eq!(old.flags, new.flags);
            let raw = |e: &locust_formats::godot_pck::PckEntry, data: &[u8]| {
                data[e.path_position as usize
                    ..(e.path_position + u64::from(e.path_length)) as usize]
                    .to_vec()
            };
            assert_eq!(raw(old, &bytes), raw(new, &output));
        }
        assert_eq!(index.entries[1].stored_md5, original.entries[1].stored_md5);
        assert_eq!(index.entries[1].size, original.entries[1].size);
    }
}

#[test]
fn noop_rebuild_streams_and_preserves_all_member_fields_and_bytes() {
    let payload = vec![0xad; 200_003];
    let bytes = authenticated_members(&payload);
    let original = read_index(&mut Cursor::new(&bytes)).unwrap();
    let mut source = RangeReader {
        bytes: Cursor::new(bytes.clone()),
        allowed: vec![
            0..original.index_end,
            212..215,
            220..220 + payload.len() as u64,
        ],
        read_bytes: 0,
        largest_read: 0,
    };
    let mut output = Vec::new();
    let plan = rebuild(&mut source, &original, &[], &[], &mut output).unwrap();
    verify_rebuild(&mut Cursor::new(&output), &plan).unwrap();
    let generated = read_index(&mut Cursor::new(&output)).unwrap();
    assert_eq!(
        source.read_bytes as u64,
        original.index_end + 3 + payload.len() as u64
    );
    for (old, new) in original.entries.iter().zip(&generated.entries) {
        assert_eq!(old.path, new.path);
        assert_eq!(old.size, new.size);
        assert_eq!(old.flags, new.flags);
        assert_eq!(old.stored_md5, new.stored_md5);
        assert_eq!(
            &bytes[old.offset as usize..(old.offset + old.size) as usize],
            &output[new.offset as usize..(new.offset + new.size) as usize]
        );
    }
}

#[test]
fn replaced_source_payload_is_not_read_even_if_corrupted() {
    let mut bytes = authenticated_members(b"second!!");
    bytes[212] ^= 1;
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    let mut source = RangeReader {
        bytes: Cursor::new(bytes),
        allowed: vec![0..index.index_end, 220..228],
        read_bytes: 0,
        largest_read: 0,
    };
    let mut output = Vec::new();
    let plan = rebuild(
        &mut source,
        &index,
        &[("res://aa", b"new")],
        &[],
        &mut output,
    )
    .unwrap();
    verify_rebuild(&mut Cursor::new(output), &plan).unwrap();
}

#[test]
fn rebuild_refuses_unknown_duplicate_colliding_and_noncanonical_keys_before_output() {
    let bytes = authenticated_members(b"second!!");
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    type ChangePair<'a> = (Vec<MemberBytes<'a>>, Vec<MemberBytes<'a>>);
    let cases: Vec<ChangePair<'_>> = vec![
        (vec![("res://absent", b"x")], vec![]),
        (vec![("res://aa", b"x"), ("res://aa", b"y")], vec![]),
        (vec![], vec![("res://aa", b"x")]),
        (vec![], vec![("res://new", b"x"), ("res://new", b"y")]),
        (vec![("res://./aa", b"x")], vec![]),
    ];
    for (replacements, additions) in cases {
        let mut output = Vec::new();
        assert!(rebuild(
            &mut Cursor::new(&bytes),
            &index,
            &replacements,
            &additions,
            &mut output
        )
        .is_err());
        assert!(output.is_empty());
    }
    for path in [
        "res://../bad",
        "res://a/",
        "C:/bad",
        "res://a\\b",
        "res://a\nb",
        "res://./aa",
        "res://new\0",
        "res://a//b",
    ] {
        let mut output = Vec::new();
        assert!(
            rebuild(
                &mut Cursor::new(&bytes),
                &index,
                &[],
                &[(path, b"x")],
                &mut output
            )
            .is_err(),
            "{path:?}"
        );
        assert!(output.is_empty());
    }
}

#[test]
fn rebuild_refuses_path_count_and_total_index_bounds() {
    let bytes = authenticated_members(b"second!!");
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    let too_long = format!("res://{}", "x".repeat(16_379));
    let mut output = Vec::new();
    assert!(rebuild(
        &mut Cursor::new(&bytes),
        &index,
        &[],
        &[(&too_long, b"")],
        &mut output
    )
    .is_err());
    let names: Vec<_> = (0..100_000).map(|i| format!("res://{i}")).collect();
    let additions: Vec<_> = names.iter().map(|s| (s.as_str(), b"".as_slice())).collect();
    assert!(rebuild(
        &mut Cursor::new(&bytes),
        &index,
        &[],
        &additions,
        &mut output
    )
    .is_err());
    let names: Vec<_> = (0..1024)
        .map(|i| format!("res://{i:04}{}", "x".repeat(16_374)))
        .collect();
    let additions: Vec<_> = names.iter().map(|s| (s.as_str(), b"".as_slice())).collect();
    assert!(rebuild(
        &mut Cursor::new(&bytes),
        &index,
        &[],
        &additions,
        &mut output
    )
    .is_err());
    assert!(output.is_empty());
}

#[test]
fn corrupt_untouched_member_and_stale_index_abort_rebuild() {
    let mut bytes = authenticated_members(b"second!!");
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    bytes[220] ^= 1;
    let mut output = Vec::new();
    let error = rebuild(
        &mut Cursor::new(&bytes),
        &index,
        &[("res://aa", b"new")],
        &[],
        &mut output,
    )
    .unwrap_err();
    assert!(error.to_string().contains("MD5"));
    assert!(!output.is_empty()); // Single-pass copy may reach staging before failing.
    put32(&mut bytes, 32, 5);
    let mut output = Vec::new();
    assert!(rebuild(&mut Cursor::new(&bytes), &index, &[], &[], &mut output).is_err());
    assert!(output.is_empty());
}

#[test]
fn independent_verifier_rejects_payload_metadata_path_order_and_trailing_changes() {
    let bytes = authenticated_members(b"second!!");
    let mut source = Cursor::new(&bytes);
    let original = read_index(&mut source).unwrap();
    let mut output = Vec::new();
    let plan = rebuild(&mut source, &original, &[], &[], &mut output).unwrap();
    let index = read_index(&mut Cursor::new(&output)).unwrap();
    // Even a self-consistent substituted member must fail the independent plan.
    let mut substituted = output.clone();
    let first = &index.entries[0];
    substituted[first.offset as usize..first.offset as usize + 3].copy_from_slice(b"bad");
    substituted[first.md5_position as usize..first.md5_position as usize + 16]
        .copy_from_slice(&Md5::digest(b"bad"));
    assert!(verify_rebuild(&mut Cursor::new(substituted), &plan).is_err());
    for position in [
        index.entries[0].offset,
        16,
        20,
        24,
        32,
        120,
        128,
        104,
        index.entries[1].path_position + 7,
    ] {
        let mut changed = output.clone();
        changed[position as usize] ^= 1;
        assert!(
            verify_rebuild(&mut Cursor::new(changed), &plan).is_err(),
            "position {position}"
        );
    }
    let mut reordered = output.clone();
    let first = output[100..148].to_vec();
    reordered.splice(100..200, output[148..200].iter().chain(&first).copied());
    assert!(verify_rebuild(&mut Cursor::new(reordered), &plan).is_err());
    output.push(0);
    assert!(verify_rebuild(&mut Cursor::new(output), &plan).is_err());
}

#[test]
fn empty_archive_rebuild_and_addition_work() {
    let mut bytes = two_members();
    bytes.truncate(100);
    put32(&mut bytes, 96, 0);
    put64(&mut bytes, 24, 0);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    for additions in [vec![], vec![("res://new", b"".as_slice())]] {
        let mut output = Vec::new();
        let plan = rebuild(
            &mut Cursor::new(&bytes),
            &index,
            &[],
            &additions,
            &mut output,
        )
        .unwrap();
        verify_rebuild(&mut Cursor::new(output), &plan).unwrap();
    }
}

#[test]
fn rebuild_keeps_directory_order_with_unsorted_absolute_source_offsets() {
    let mut bytes = authenticated_members(b"second!!");
    // Payloads switch physical order; directory order and raw paths do not.
    bytes[212..220].copy_from_slice(b"second!!");
    bytes[220..223].copy_from_slice(b"one");
    put64(&mut bytes, 24, 0);
    put64(&mut bytes, 112, 220);
    put64(&mut bytes, 164, 212);
    let index = read_index(&mut Cursor::new(&bytes)).unwrap();
    let mut output = Vec::new();
    let plan = rebuild(&mut Cursor::new(&bytes), &index, &[], &[], &mut output).unwrap();
    let mut output = Cursor::new(output);
    verify_rebuild(&mut output, &plan).unwrap();
    let generated = read_index(&mut output).unwrap();
    assert_eq!(generated.entries[0].path, "res://aa");
    assert_eq!(generated.entries[1].path, "res://b/b");
    assert_eq!(
        read_member(&mut output, &generated.entries[0], 100).unwrap(),
        b"one"
    );
    assert_eq!(
        read_member(&mut output, &generated.entries[1], 100).unwrap(),
        b"second!!"
    );
}

#[test]
fn transaction_installs_verified_pck_keeps_exact_backup_and_honors_lock_scope() {
    use locust_core::patch::GameLock;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = root.path().join("owned.pck");
    let bytes = authenticated_members(b"second!!");
    std::fs::write(&file, &bytes).unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    assert!(GameLock::acquire(root.path()).is_err());
    let wrong_lock = GameLock::acquire(outside.path()).unwrap();
    assert!(replace_archive(&wrong_lock, &file, &[], &[]).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    let backup = replace_archive(
        &lock,
        &file,
        &[("res://aa", b"replacement")],
        &[("res://added", b"added")],
    )
    .unwrap();
    assert_eq!(std::fs::read(&backup).unwrap(), bytes);
    assert!(backup.extension().is_none());
    let mut installed = std::fs::File::open(&file).unwrap();
    let index = read_index(&mut installed).unwrap();
    assert_eq!(index.entries.len(), 3);
    for entry in &index.entries {
        assert!(verify_member(&mut installed, entry).unwrap());
    }
    assert_eq!(
        read_member(&mut installed, &index.entries[0], 100).unwrap(),
        b"replacement"
    );
    drop(installed);
    drop(wrong_lock);
    drop(lock);
}

#[test]
fn transaction_corrupt_untouched_payload_never_installs_partial_rebuild() {
    use locust_core::patch::GameLock;
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("owned.pck");
    let mut bytes = authenticated_members(b"second!!");
    bytes[220] ^= 1;
    std::fs::write(&file, &bytes).unwrap();
    let lock = GameLock::acquire(root.path()).unwrap();
    assert!(replace_archive(&lock, &file, &[("res://aa", b"replacement")], &[]).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    drop(lock);
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
fn godot_refuses_unsupported_pack_version() {
    for minor in [3, 4, 5] {
        let mut bytes = two_members_for_engine(minor);
        put32(&mut bytes, 4, 3);
        let error = read_index(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "PCK format version unsupported (requires v2)"
        );
    }
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
fn godot_bounds_engine_versions_but_does_not_invent_a_patch_limit() {
    for (major, minor) in [(0, 0), (3, 6), (4, 5), (4, u32::MAX), (5, 0)] {
        let mut bytes = two_members();
        put32(&mut bytes, 8, major);
        put32(&mut bytes, 12, minor);
        let error = read_index(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "PCK engine version unsupported (requires Godot 4.0–4.4)"
        );
    }
    for minor in 0..=4 {
        let mut bytes = two_members_for_engine(minor);
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
fn rebuild_refuses_relayout_overflow_before_writing_or_reading_payload() {
    let mut bytes = two_members();
    bytes.truncate(196);
    put64(&mut bytes, 168, u64::MAX - 216);
    let mut reader = MetadataOnly {
        bytes: Cursor::new(bytes),
        file_size: u64::MAX,
        read_bytes: 0,
    };
    let index = read_index(&mut reader).unwrap();
    let mut output = Vec::new();
    let error = rebuild(&mut reader, &index, &[], &[], &mut output).unwrap_err();
    assert!(error.to_string().contains("overflow"));
    assert!(output.is_empty());
    assert_eq!(reader.read_bytes, 2 * 196);
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
