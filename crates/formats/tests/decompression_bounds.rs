use std::io::{Cursor, Write};
use std::path::PathBuf;

use locust_formats::{kirikiri_xp3, tyrano_nw::NwArchive, yuris_ypf};

// Exercise the private shared implementation without expanding the public API.
#[path = "../src/util.rs"]
mod util;

fn compressed(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

#[test]
fn zlib_bomb_stops_at_small_limit() {
    let bomb = compressed(&vec![0; 64 * 1024 * 1024]);
    assert!(util::inflate_zlib_bounded(&bomb, None, 1024).is_err());
    assert!(util::inflate_zlib_bounded(&bomb, Some(1), 1024).is_err());
}

#[test]
fn zlib_declared_size_mismatch_is_rejected() {
    let data = compressed(b"payload");
    for expected in [0, 6, 8, u64::MAX] {
        assert!(util::inflate_zlib_bounded(&data, Some(expected), 16).is_err());
    }
    assert_eq!(
        util::inflate_zlib_bounded(&data, Some(7), 7).unwrap(),
        b"payload"
    );
    assert_eq!(
        util::inflate_zlib_bounded(&compressed(b""), Some(0), 0).unwrap(),
        b""
    );
}

#[test]
fn bounded_reader_stops_after_one_probe_byte() {
    let mut reader = Cursor::new(vec![0; 4096]);
    assert!(util::read_bounded(&mut reader, None, 32).is_err());
    assert_eq!(reader.position(), 33);
    reader.set_position(0);
    assert!(util::read_bounded(&mut reader, Some(4), 32).is_err());
    assert_eq!(reader.position(), 5);
    assert!(util::read_bounded(std::io::empty(), Some(33), 32).is_err());
}

#[test]
fn aggregate_budget_rejects_entries_that_fit_individually() {
    assert_eq!(util::MAX_ENTRY_DECODED, 512 * 1024 * 1024);
    assert_eq!(util::MAX_ARCHIVE_DECODED, 1024 * 1024 * 1024);
    let mut budget = util::DecodeBudget::new(10);
    budget.consume(6).unwrap();
    assert!(budget.consume(5).is_err());
    budget.consume(4).unwrap();
    assert!(budget.consume(u64::MAX).is_err());
}

#[test]
fn lzstring_bomb_is_bounded_and_valid_unicode_is_preserved() {
    let bomb = lz_str::compress_to_base64(&"x".repeat(1024 * 1024));
    assert!(util::decode_lzstring_base64_bounded(&bomb, 64).is_err());
    for text in ["", "a", "abababababab", "日本語 🌍 café", "abcabcabcabc"] {
        let encoded = lz_str::compress_to_base64(text);
        assert_eq!(
            util::decode_lzstring_base64_bounded(&encoded, 1024).unwrap(),
            text
        );
    }
    assert!(util::decode_lzstring_base64_bounded(&lz_str::compress_to_base64("a"), 0).is_err());
}

#[test]
fn valid_small_xp3_and_ypf_archives_extract() {
    let files = vec![("scenario/first.ks".into(), b"hello".to_vec(), true)];
    let xp3 = kirikiri_xp3::Xp3Archive::from_bytes(
        PathBuf::from("small.xp3"),
        kirikiri_xp3::write_xp3_custom(&files).unwrap(),
    )
    .unwrap();
    assert_eq!(xp3.read_entry(&xp3.entries[0]).unwrap(), b"hello");
    let ypf = yuris_ypf::YpfArchive::from_bytes(
        PathBuf::from("small.ypf"),
        yuris_ypf::write_ypf(0x1e4, 0xff, &files).unwrap(),
    )
    .unwrap();
    assert_eq!(ypf.read_entry(&ypf.entries[0]).unwrap(), b"hello");
}

fn nw_fixture() -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "data/scenario/first.ks",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
    writer.write_all(b"hello").unwrap();
    writer.finish().unwrap().into_inner()
}

#[test]
fn valid_small_nw_archive_extracts() {
    let nw = NwArchive::from_bytes(PathBuf::from("package.nw"), nw_fixture()).unwrap();
    assert_eq!(nw.read_entry(&nw.entries[0]).unwrap(), b"hello");
}

#[test]
fn xp3_index_size_mismatches_still_open() {
    let original = kirikiri_xp3::write_xp3(&[("first.ks".into(), b"hello".to_vec())]).unwrap();
    let off = kirikiri_xp3::resolve_index_offset(&original, "fixture").unwrap() as usize;
    let actual = u64::from_le_bytes(original[off + 9..off + 17].try_into().unwrap());
    for declared in [0, 1, actual - 1, actual + 1, u64::MAX] {
        let mut data = original.clone();
        data[off + 9..off + 17].copy_from_slice(&declared.to_le_bytes());
        let xp3 =
            kirikiri_xp3::Xp3Archive::from_bytes(PathBuf::from("mismatch.xp3"), data).unwrap();
        assert_eq!(xp3.read_entry(&xp3.entries[0]).unwrap(), b"hello");
    }
}

#[test]
fn xp3_segment_and_entry_size_mismatches_use_decoded_length() {
    for zlib in [false, true] {
        let xp3 = kirikiri_xp3::Xp3Archive::from_bytes(
            PathBuf::from("small.xp3"),
            kirikiri_xp3::write_xp3_custom(&[("first.ks".into(), b"hello".to_vec(), zlib)])
                .unwrap(),
        )
        .unwrap();
        for declared in [0, 1, 6] {
            for (entry_size, segment_size) in [(5, declared), (declared, 5), (declared, declared)] {
                let mut entry = xp3.entries[0].clone();
                entry.orig_size = entry_size;
                entry.segments[0].orig_size = segment_size;
                assert_eq!(xp3.read_entry(&entry).unwrap(), b"hello");
            }
        }
        let mut entry = xp3.entries[0].clone();
        entry.segments[0].orig_size = util::MAX_ENTRY_DECODED + 1;
        assert!(xp3
            .read_entry(&entry)
            .unwrap_err()
            .message
            .contains("limit"));
    }
}

#[test]
fn ypf_rejects_declared_size_mismatch() {
    let ypf = yuris_ypf::YpfArchive::from_bytes(
        PathBuf::from("small.ypf"),
        yuris_ypf::write_ypf(
            0x1e4,
            0xff,
            &[("first.ybn".into(), b"hello".to_vec(), true)],
        )
        .unwrap(),
    )
    .unwrap();
    let mut entry = ypf.entries[0].clone();
    entry.unpacked_size = 1;
    assert!(ypf.read_entry(&entry).is_err());
}

#[test]
fn shared_adler32_preserves_known_values() {
    assert_eq!(util::adler32(b""), 1);
    assert_eq!(util::adler32(b"Wikipedia"), 0x11e6_0398);
    assert_eq!(
        kirikiri_xp3::adler32(b"Wikipedia"),
        yuris_ypf::adler32(b"Wikipedia")
    );
}

#[test]
fn xp3_large_declared_totals_open_and_only_oversized_reads_fail() {
    let mut files: Vec<_> = ["a.movie", "b.movie", "c.movie"]
        .into_iter()
        .map(|name| (name.to_owned(), Vec::new(), false))
        .collect();
    files.push(("first.ks".into(), b"hello".to_vec(), true));
    let original = kirikiri_xp3::write_xp3_custom(&files).unwrap();
    for declared in [400 * 1024 * 1024u64, 513 * 1024 * 1024] {
        let mut data = original.clone();
        let off = kirikiri_xp3::resolve_index_offset(&data, "fixture").unwrap() as usize;
        let mut file_pos = off + 9;
        for _ in 0..3 {
            assert_eq!(&data[file_pos..file_pos + 4], b"File");
            let file_size =
                u64::from_le_bytes(data[file_pos + 4..file_pos + 12].try_into().unwrap()) as usize;
            let end = file_pos + 12 + file_size;
            let mut pos = file_pos + 12;
            while pos < end {
                let size = u64::from_le_bytes(data[pos + 4..pos + 12].try_into().unwrap()) as usize;
                if &data[pos..pos + 4] == b"info" {
                    data[pos + 16..pos + 24].copy_from_slice(&declared.to_le_bytes());
                } else if &data[pos..pos + 4] == b"segm" {
                    data[pos + 24..pos + 32].copy_from_slice(&declared.to_le_bytes());
                }
                pos += 12 + size;
            }
            file_pos = end;
        }
        let archive =
            kirikiri_xp3::Xp3Archive::from_bytes(PathBuf::from("large.xp3"), data).unwrap();
        assert_eq!(archive.entries.len(), 4);
        assert!(
            archive.entries.iter().map(|e| e.orig_size).sum::<u64>() > util::MAX_ARCHIVE_DECODED
        );
        assert_eq!(
            archive
                .read_entry(archive.ks_entries().next().unwrap())
                .unwrap(),
            b"hello"
        );
        if declared > util::MAX_ENTRY_DECODED {
            assert!(archive
                .read_entry(&archive.entries[0])
                .unwrap_err()
                .message
                .contains("limit"));
        }
    }
}

#[test]
fn ypf_large_declared_totals_open_and_only_oversized_reads_fail() {
    let mut files: Vec<_> = ["a.movie", "b.movie", "c.movie"]
        .into_iter()
        .map(|name| (name.to_owned(), Vec::new(), false))
        .collect();
    files.push(("first.ybn".into(), b"hello".to_vec(), true));
    let original = yuris_ypf::write_ypf(0x1e4, 0xff, &files).unwrap();
    let parsed =
        yuris_ypf::YpfArchive::from_bytes(PathBuf::from("small.ypf"), original.clone()).unwrap();
    for declared in [400 * 1024 * 1024u32, 513 * 1024 * 1024] {
        let mut data = original.clone();
        let mut pos = 0x20;
        for entry in &parsed.entries {
            let after_name = pos + 5 + entry.index_name.len();
            if entry.name.ends_with(".movie") {
                data[after_name + 2..after_name + 6].copy_from_slice(&declared.to_le_bytes());
            }
            pos = after_name + 18 + entry.extra.len();
        }
        let archive = yuris_ypf::YpfArchive::from_bytes(PathBuf::from("large.ypf"), data).unwrap();
        assert_eq!(archive.entries.len(), 4);
        assert!(
            archive
                .entries
                .iter()
                .map(|e| u64::from(e.unpacked_size))
                .sum::<u64>()
                > util::MAX_ARCHIVE_DECODED
        );
        assert_eq!(
            archive
                .read_entry(archive.ybn_entries().next().unwrap())
                .unwrap(),
            b"hello"
        );
        if u64::from(declared) > util::MAX_ENTRY_DECODED {
            let movie = archive
                .entries
                .iter()
                .find(|e| e.name.ends_with(".movie"))
                .unwrap();
            assert!(archive
                .read_entry(movie)
                .unwrap_err()
                .message
                .contains("limit"));
        }
    }
}

#[test]
fn lzstring_seeded_random_strings_match_existing_decoder() {
    let mut state = 0x8f31_a207u32;
    let mut next = || {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        state
    };
    let alphabet: Vec<char> = "abcXYZ0123 \n\t\0éàñüøß漢字日本語中文😀🌍🚀🦀"
        .chars()
        .collect();
    for case in 0..320 {
        let len = match case {
            0 => 0,
            1 => 1,
            2 => 5000,
            _ => next() as usize % 5001,
        };
        let text: String = (0..len)
            .map(|_| alphabet[next() as usize % alphabet.len()])
            .collect();
        let encoded = lz_str::compress_to_base64(&text);
        let expected = String::from_utf16_lossy(&lz_str::decompress_from_base64(&encoded).unwrap());
        assert_eq!(
            util::decode_lzstring_base64_bounded(&encoded, 4 * 1024 * 1024).unwrap(),
            expected,
            "case {case}, length {len}"
        );
    }
}

#[test]
fn nw_rejects_oversized_entry_before_reading() {
    let mut data = nw_fixture();
    let pos = data.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    data[pos + 24..pos + 28].copy_from_slice(&(513 * 1024 * 1024u32).to_le_bytes());
    assert!(NwArchive::from_bytes(PathBuf::from("large.nw"), data).is_err());
}

#[test]
fn lzstring_exact_limits_and_utf16_lossy_behavior_match() {
    assert_eq!(
        util::decode_lzstring_base64_bounded(&lz_str::compress_to_base64("a"), 2).unwrap(),
        "a"
    );
    assert!(util::decode_lzstring_base64_bounded(&lz_str::compress_to_base64("a"), 1).is_err());
    // UTF-8 can be larger than the intermediate UTF-16 representation.
    assert!(
        util::decode_lzstring_base64_bounded(&lz_str::compress_to_base64("\u{800}"), 2).is_err()
    );
    let invalid_utf16 = [0xd800u16, 0x61, 0xdc00];
    let encoded = lz_str::compress_to_base64(invalid_utf16.as_slice());
    assert_eq!(
        util::decode_lzstring_base64_bounded(&encoded, 1024).unwrap(),
        String::from_utf16_lossy(&invalid_utf16)
    );
}

#[test]
fn lzstring_dictionary_width_changes_match_existing_decoder() {
    let mut state = 1234567u32;
    let units: Vec<u16> = (0..20000)
        .map(|_| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            (state >> 16) as u16
        })
        .collect();
    for data in [units.as_slice(), &[0x61u16; 20000][..]] {
        let encoded = lz_str::compress_to_base64(data);
        let expected = String::from_utf16_lossy(&lz_str::decompress_from_base64(&encoded).unwrap());
        assert_eq!(
            util::decode_lzstring_base64_bounded(&encoded, 4 * 1024 * 1024).unwrap(),
            expected
        );
    }
}

#[test]
fn nw_rejects_aggregate_overflow_and_false_declared_size() {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["a.ks", "b.ks", "c.ks"] {
        writer
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(b"hello").unwrap();
    }
    let mut data = writer.finish().unwrap().into_inner();
    let positions: Vec<_> = data
        .windows(4)
        .enumerate()
        .filter_map(|(pos, w)| (w == b"PK\x01\x02").then_some(pos))
        .collect();
    for pos in positions {
        data[pos + 24..pos + 28].copy_from_slice(&(400 * 1024 * 1024u32).to_le_bytes());
    }
    assert!(NwArchive::from_bytes(PathBuf::from("large.nw"), data).is_err());
    let mut data = nw_fixture();
    let pos = data.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    data[pos + 24..pos + 28].copy_from_slice(&1u32.to_le_bytes());
    let archive = NwArchive::from_bytes(PathBuf::from("false.nw"), data).unwrap();
    assert!(archive.read_entry(&archive.entries[0]).is_err());
}
