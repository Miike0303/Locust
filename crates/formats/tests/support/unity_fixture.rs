use super::CLASS_ID_TEXT_ASSET;

/// Build a minimal v17 SerializedFile with one TextAsset and one dummy object.
pub fn write_v17_fixture(text_name: &str, text_script: &str) -> Vec<u8> {
    write_v17_fixture_ex(
        text_name,
        text_script,
        Some(("Dummy", "not a text asset body")),
    )
}

pub fn write_v17_fixture_ex(
    text_name: &str,
    text_script: &str,
    extra_gameobject_like: Option<(&str, &str)>,
) -> Vec<u8> {
    fn align4(n: usize) -> usize {
        (n + 3) & !3
    }
    fn write_aligned_string(buf: &mut Vec<u8>, s: &str) {
        let b = s.as_bytes();
        buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
        buf.extend_from_slice(b);
        let pad = align4(b.len()) - b.len();
        buf.extend(std::iter::repeat_n(0u8, pad));
    }
    fn write_be_u32(buf: &mut Vec<u8>, v: u32) {
        buf.extend_from_slice(&v.to_be_bytes());
    }

    // --- object payloads (little-endian) ---
    let mut text_payload = Vec::new();
    write_aligned_string(&mut text_payload, text_name);
    write_aligned_string(&mut text_payload, text_script);

    let mut other_payload = Vec::new();
    if let Some((n, s)) = extra_gameobject_like {
        // Fake "aligned strings" so we have a non-TextAsset blob of nonzero size.
        write_aligned_string(&mut other_payload, n);
        write_aligned_string(&mut other_payload, s);
    } else {
        other_payload.extend_from_slice(&[0u8; 16]);
    }

    // --- metadata (little-endian) ---
    let mut meta = Vec::new();
    // unity version cstr
    meta.extend_from_slice(b"2019.4.0f1\0");
    meta.extend_from_slice(&1u32.to_le_bytes()); // target platform
    meta.push(0); // enable_type_tree = false

    // 2 types: TextAsset (49), GameObject (1)
    meta.extend_from_slice(&2i32.to_le_bytes());
    // type 0: TextAsset
    meta.extend_from_slice(&CLASS_ID_TEXT_ASSET.to_le_bytes());
    meta.push(0); // stripped
    meta.extend_from_slice(&(-1i16).to_le_bytes()); // script_type_index
    meta.extend_from_slice(&[0u8; 16]); // old_type_hash
                                        // type 1: GameObject class 1
    meta.extend_from_slice(&1i32.to_le_bytes());
    meta.push(0);
    meta.extend_from_slice(&(-1i16).to_le_bytes());
    meta.extend_from_slice(&[0u8; 16]);

    // objects: 2
    meta.extend_from_slice(&2i32.to_le_bytes());
    // obj0 TextAsset path_id=1
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    let text_byte_start = 0u32;
    let text_byte_size = text_payload.len() as u32;
    meta.extend_from_slice(&1i64.to_le_bytes()); // path_id
    meta.extend_from_slice(&text_byte_start.to_le_bytes());
    meta.extend_from_slice(&text_byte_size.to_le_bytes());
    meta.extend_from_slice(&0i32.to_le_bytes()); // type index 0

    // obj1 other path_id=2
    while meta.len() % 4 != 0 {
        meta.push(0);
    }
    let other_byte_start = text_byte_size; // packed sequentially
    let other_byte_size = other_payload.len() as u32;
    meta.extend_from_slice(&2i64.to_le_bytes());
    meta.extend_from_slice(&other_byte_start.to_le_bytes());
    meta.extend_from_slice(&other_byte_size.to_le_bytes());
    meta.extend_from_slice(&1i32.to_le_bytes()); // type index 1

    // Header (big-endian) + metadata + data
    // data_offset aligned to 16 for cleanliness
    let header_len = 20usize; // v17: metadataSize,fileSize,version,dataOffset,endian+reserved
    let mut data_offset = header_len + meta.len();
    data_offset = (data_offset + 15) & !15;

    let file_size = data_offset + text_payload.len() + other_payload.len();
    let metadata_size = meta.len() as u32;

    let mut out = Vec::new();
    write_be_u32(&mut out, metadata_size);
    write_be_u32(&mut out, file_size as u32);
    write_be_u32(&mut out, 17); // version
    write_be_u32(&mut out, data_offset as u32);
    out.push(0); // little endian
    out.extend_from_slice(&[0, 0, 0]);

    out.extend_from_slice(&meta);
    while out.len() < data_offset {
        out.push(0);
    }
    out.extend_from_slice(&text_payload);
    out.extend_from_slice(&other_payload);
    out
}
