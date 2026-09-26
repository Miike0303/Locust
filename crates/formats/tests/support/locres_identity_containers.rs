//! Independent wire fixtures for identity tests; modern PAK writer follows the
//! already reviewed unreal_modern.rs fixture layout, accepting arbitrary LocRes.
use locust_formats::unreal_pak::*;
use std::io::Write;
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
pub fn native(root: &std::path::Path, payload: &[u8]) -> std::path::PathBuf {
    let mut directory = Vec::new();
    string(
        &mut directory,
        "../../../Game/Content/Localization/Game/en/",
    );
    for n in [1, u32::MAX, u32::MAX, u32::MAX, 0, 1, 0, u32::MAX, 0, 1] {
        u32b(&mut directory, n);
    }
    string(&mut directory, "Game.locres");
    let mut blocks = Vec::new();
    let mut ucas = Vec::new();
    for raw in payload.chunks(64) {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(raw).unwrap();
        let compressed = z.finish().unwrap();
        blocks.extend_from_slice(&(ucas.len() as u64).to_le_bytes()[..5]);
        blocks.extend_from_slice(&(compressed.len() as u32).to_le_bytes()[..3]);
        blocks.extend_from_slice(&(raw.len() as u32).to_le_bytes()[..3]);
        blocks.push(1);
        ucas.extend(compressed);
    }
    let mut toc = vec![0; 144];
    toc[..16].copy_from_slice(b"-==--==--==--==-");
    toc[16] = 8;
    toc[80] = 9;
    for (at, n) in [
        (20, 144),
        (24, 1),
        (28, (blocks.len() / 12) as u32),
        (32, 12),
        (36, 1),
        (40, 32),
        (44, 64),
        (48, directory.len() as u32),
        (52, 1),
    ] {
        toc[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    toc[88..96].copy_from_slice(&u64::MAX.to_le_bytes());
    toc.extend([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7]);
    toc.extend([0; 5]);
    toc.extend_from_slice(&(payload.len() as u64).to_be_bytes()[3..]);
    toc.extend(blocks);
    let mut method = [0; 32];
    method[..4].copy_from_slice(b"Zlib");
    toc.extend(method);
    toc.extend(directory);
    toc.extend_from_slice(&blake3::hash(payload).as_bytes()[..20]);
    toc.extend([0; 4]);
    let path = root.join("game.utoc");
    std::fs::write(&path, toc).unwrap();
    std::fs::write(path.with_extension("ucas"), ucas).unwrap();
    path
}

pub fn container(root: &std::path::Path, kind: &str, payload: Vec<u8>) -> std::path::PathBuf {
    if kind == "native" {
        return native(root, &payload);
    }
    let path = root.join("game.pak");
    let data = if kind == "modern" {
        modern(payload)
    } else {
        write_pak(
            DEFAULT_MOUNT_POINT,
            8,
            &[PakWriteFile {
                name: format!("{DIR}Game.locres"),
                data: payload,
            }],
            "fixture",
        )
        .unwrap()
    };
    std::fs::write(&path, data).unwrap();
    path
}
pub fn modern(data: Vec<u8>) -> Vec<u8> {
    let version = 11;
    let codec = "Zlib";
    let block_size = 32;
    let ordinary = false;
    let path_hash = true;

    let compressed = true;
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
    bytes
}
