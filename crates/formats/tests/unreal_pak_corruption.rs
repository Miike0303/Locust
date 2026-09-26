//! Deterministic parser corruption review. Fixture wire writer copied from
//! unreal_modern.rs at baseline-sha256.json; no downloaded game bytes.
use locust_formats::unreal_locres::{LocresFile, LocresNamespace, LocresString, LocresVersion};
use locust_formats::unreal_pak::*;
use std::io::{Cursor, Write};

fn u32b(v: &mut Vec<u8>, n: u32) {
    v.extend(n.to_le_bytes());
}
fn u64b(v: &mut Vec<u8>, n: u64) {
    v.extend(n.to_le_bytes());
}
fn string(v: &mut Vec<u8>, s: &str) {
    u32b(v, s.len() as u32 + 1);
    v.extend(s.as_bytes());
    v.push(0);
}
const DIR: &str = "Game/Content/Localization/Game/en/";
fn locres() -> Vec<u8> {
    LocresFile {
        version: LocresVersion::Compact,
        namespaces: vec![LocresNamespace {
            name: "Dialog".into(),
            name_hash: 0,
            strings: vec![LocresString {
                key: "Greeting".into(),
                value: "Hello traveler!".into(),
                source_string_hash: 42,
                key_hash: 0,
            }],
        }],
    }
    .serialize()
    .unwrap()
}

struct Fixture {
    bytes: Vec<u8>,
    primary: usize,
    directory: usize,
    phi: usize,
    encoded: usize,
    dir_location: usize,
}

fn fixture(
    version: u32,
    codec: &str,
    block_size: usize,
    ordinary: bool,
    path_hash: bool,
) -> Fixture {
    let data = locres();
    let compressed = !codec.is_empty();
    let method = if compressed { 1 } else { 0 };
    let mut blocks = Vec::new();
    let mut stored = Vec::new();
    if compressed {
        for chunk in data.chunks(block_size) {
            let bytes = if codec == "Gzip" {
                let mut w =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                w.write_all(chunk).unwrap();
                w.finish().unwrap()
            } else {
                let mut w =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                w.write_all(chunk).unwrap();
                w.finish().unwrap()
            };
            blocks.push(bytes.len());
            stored.extend(bytes);
        }
    } else {
        stored = data.clone();
    }
    let header_size = 53 + if compressed { 4 + 16 * blocks.len() } else { 0 };
    let mut header = Vec::new();
    u64b(&mut header, 0);
    u64b(&mut header, stored.len() as u64);
    u64b(&mut header, data.len() as u64);
    u32b(&mut header, method);
    header.extend(sha1_bytes(&stored));
    if compressed {
        u32b(&mut header, blocks.len() as u32);
        let mut start = header_size as u64;
        for size in &blocks {
            u64b(&mut header, start);
            start += *size as u64;
            u64b(&mut header, start);
        }
    }
    header.push(0);
    u32b(&mut header, if compressed { block_size as u32 } else { 0 });
    let mut bytes = header.clone();
    bytes.extend(stored);
    let mut encoded = Vec::new();
    if !ordinary {
        let bits = (1 << 31)
            | (1 << 30)
            | (1 << 29)
            | (method << 23)
            | ((blocks.len() as u32) << 6)
            | if compressed { 63 } else { 0 };
        u32b(&mut encoded, bits);
        if compressed {
            u32b(&mut encoded, block_size as u32);
        }
        u32b(&mut encoded, 0);
        u32b(&mut encoded, data.len() as u32);
        if compressed {
            u32b(&mut encoded, (bytes.len() - header_size) as u32);
            if blocks.len() > 1 {
                for n in &blocks {
                    u32b(&mut encoded, *n as u32);
                }
            }
        }
    }
    let mut directory_bytes = Vec::new();
    u32b(&mut directory_bytes, 1);
    string(&mut directory_bytes, DIR);
    u32b(&mut directory_bytes, 1);
    string(&mut directory_bytes, "Game.locres");
    let dir_location_relative = directory_bytes.len();
    u32b(&mut directory_bytes, if ordinary { u32::MAX } else { 0 });
    let directory = bytes.len();
    bytes.extend(&directory_bytes);
    let mut phi_bytes = Vec::new();
    u32b(&mut phi_bytes, 1);
    u64b(&mut phi_bytes, 1234);
    u32b(&mut phi_bytes, if ordinary { u32::MAX } else { 0 });
    u32b(&mut phi_bytes, 0);
    let phi = bytes.len();
    if path_hash {
        bytes.extend(&phi_bytes);
    }
    let primary = bytes.len();
    let mut index = Vec::new();
    string(&mut index, DEFAULT_MOUNT_POINT);
    u32b(&mut index, 1);
    u64b(&mut index, 0);
    u32b(&mut index, path_hash as u32);
    if path_hash {
        u64b(&mut index, phi as u64);
        u64b(&mut index, phi_bytes.len() as u64);
        index.extend(sha1_bytes(&phi_bytes));
    }
    u32b(&mut index, 1);
    u64b(&mut index, directory as u64);
    u64b(&mut index, directory_bytes.len() as u64);
    index.extend(sha1_bytes(&directory_bytes));
    u32b(&mut index, encoded.len() as u32);
    let encoded_offset = primary + index.len();
    index.extend(encoded);
    u32b(&mut index, ordinary as u32);
    if ordinary {
        index.extend(header);
    }
    bytes.extend(&index);
    bytes.extend([0; 17]);
    u32b(&mut bytes, PAK_MAGIC);
    u32b(&mut bytes, version);
    u64b(&mut bytes, primary as u64);
    u64b(&mut bytes, index.len() as u64);
    bytes.extend(sha1_bytes(&index));
    for method in [codec, "", "", "", ""] {
        let mut slot = [0; 32];
        slot[..method.len()].copy_from_slice(method.as_bytes());
        bytes.extend(slot);
    }
    Fixture {
        bytes,
        primary,
        directory,
        phi,
        encoded: encoded_offset,
        dir_location: directory + dir_location_relative,
    }
}

fn payload(bytes: &[u8]) -> Result<Vec<u8>, PakError> {
    let index = read_index(bytes, "fixture")?;
    read_payload(
        &mut Cursor::new(bytes),
        &index.records[0],
        index.footer.version,
        bytes.len() as u64,
        "fixture",
    )
}

fn rehash_primary(f: &mut Fixture) {
    let footer = read_footer(&f.bytes, "fixture").unwrap();
    let hash = sha1_bytes(&f.bytes[f.primary..f.primary + footer.index_size as usize]);
    f.bytes[footer.magic_offset + 24..footer.magic_offset + 44].copy_from_slice(&hash);
}
fn rehash_directory(f: &mut Fixture, path_hash: bool) {
    let end = if path_hash { f.phi } else { f.primary };
    let hash = sha1_bytes(&f.bytes[f.directory..end]);
    let hash_pos = f.primary
        + 4
        + DEFAULT_MOUNT_POINT.len()
        + 1
        + 4
        + 8
        + 4
        + if path_hash { 36 } else { 0 }
        + 4
        + 8
        + 8;
    f.bytes[hash_pos..hash_pos + 20].copy_from_slice(&hash);
    rehash_primary(f);
}

#[test]
fn modern_pak_authenticated_corruption_and_truncation_corpus() {
    let mut cases = 0usize;
    let mut rejected = 0usize;
    let mut accepted = 0usize;
    let mut inspect = |bytes: &[u8]| {
        let outcome = std::panic::catch_unwind(|| payload(bytes));
        assert!(
            outcome.is_ok(),
            "modern PAK parser panicked on mutation {cases}"
        );
        cases += 1;
        match outcome.unwrap() {
            Ok(data) => {
                assert!(data.len() as u64 <= MAX_LOCRES_PAYLOAD);
                LocresFile::parse(&data, "payload").unwrap();
                accepted += 1;
            }
            Err(_) => {
                rejected += 1;
            }
        }
    };
    for (version, codec, ordinary) in [(10, "Gzip", true), (11, "Zlib", false), (11, "", false)] {
        let original = fixture(version, codec, 32, ordinary, true);
        inspect(&original.bytes);
        for cut in 0..original.bytes.len() {
            inspect(&original.bytes[..cut]);
        }
        for position in original.directory..original.phi {
            for mask in [1, 0x80, 0xff] {
                let mut candidate = fixture(version, codec, 32, ordinary, true);
                candidate.bytes[position] ^= mask;
                rehash_directory(&mut candidate, true);
                inspect(&candidate.bytes);
            }
        }
        let end = read_footer(&original.bytes, "baseline")
            .unwrap()
            .magic_offset
            - 17;
        for position in original.primary..end {
            for mask in [1, 0x80, 0xff] {
                let mut candidate = fixture(version, codec, 32, ordinary, true);
                candidate.bytes[position] ^= mask;
                rehash_primary(&mut candidate);
                inspect(&candidate.bytes);
            }
        }
        // Full-width hostile location and encoded count/flags, authenticated.
        let mut candidate = fixture(version, codec, 32, ordinary, true);
        candidate.bytes[candidate.dir_location..candidate.dir_location + 4]
            .copy_from_slice(&i32::MAX.to_le_bytes());
        rehash_directory(&mut candidate, true);
        inspect(&candidate.bytes);
        if !ordinary {
            let mut candidate = fixture(version, codec, 32, ordinary, true);
            candidate.bytes[candidate.encoded..candidate.encoded + 4]
                .copy_from_slice(&u32::MAX.to_le_bytes());
            rehash_primary(&mut candidate);
            inspect(&candidate.bytes);
        }
    }
    eprintln!(
        "MODERN_PAK_CORPUS cases={cases} rejected={rejected} accepted_locres_payloads={accepted}"
    );
}
