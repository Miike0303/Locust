//! Unencrypted XP3 archive reader/writer for the KiriKiri plugin.
//!
//! # Spec sources
//! - Pack layout (magic, index flag, File/info/segm/adlr chunk nesting):
//!   https://github.com/arcusmaximus/KirikiriTools/blob/master/Xp3Pack/Xp3ArchiveWriter.cs
//!   https://github.com/arcusmaximus/KirikiriTools/blob/master/Xp3Pack/Xp3IndexBuilder.cs
//! - v2 extended header (`QWORD@11 == 0x17`, real index offset at absolute 0x20):
//!   project notes in `docs/VN_RPG_TRANSLATION.md` / common Krkr tooling.
//!
//! # Out of scope
//! CxDec / Hxv4 / name-stripped archives; rewriting base archives (inject writes
//! `patch.xp3` only).

use std::path::{Path, PathBuf};

use tracing::warn;

/// XP3 magic: `XP3\r\n \n\x1a\x8b\x67\x01`
pub const XP3_MAGIC: &[u8; 11] = &[
    0x58, 0x50, 0x33, 0x0D, 0x0A, 0x20, 0x0A, 0x1A, 0x8B, 0x67, 0x01,
];

/// When the first u64 after magic is this value, the archive uses the v2 header
/// and the real index offset is stored at absolute file offset 0x20.
const V2_INDEX_MARKER: u64 = 0x17;
const V2_REAL_INDEX_OFFSET_POS: usize = 0x20;

// Chunk type tags as little-endian u32 of ASCII "File", "info", "segm", "adlr"
const TAG_FILE: u32 = 0x656C_6946; // "File"
const TAG_INFO: u32 = 0x6F66_6E69; // "info"
const TAG_SEGM: u32 = 0x6D67_6573; // "segm"
const TAG_ADLR: u32 = 0x726C_6461; // "adlr"

const FLAG_PROTECTED: u32 = 0x8000_0000;
const SEGM_FLAG_ZLIB: u32 = 0x01;

const SEGM_ENTRY_SIZE: usize = 28; // u32 + 3×u64

/// One file entry listed in the XP3 index (payload not yet read).
#[derive(Clone, Debug)]
pub struct Xp3Entry {
    /// Archive-internal path (normalized to `/`).
    pub name: String,
    pub orig_size: u64,
    pub packed_size: u64,
    pub flags: u32,
    pub adler32: u32,
    pub segments: Vec<Xp3Segment>,
}

#[derive(Clone, Debug)]
pub struct Xp3Segment {
    /// bit0 = zlib-compressed payload
    pub flags: u32,
    pub offset: u64,
    pub orig_size: u64,
    pub packed_size: u64,
}

impl Xp3Segment {
    pub fn is_zlib(&self) -> bool {
        self.flags & SEGM_FLAG_ZLIB != 0
    }
}

/// Parsed archive with index entries; holds the raw bytes for segment reads.
#[derive(Clone, Debug)]
pub struct Xp3Archive {
    pub path: PathBuf,
    pub data: Vec<u8>,
    pub entries: Vec<Xp3Entry>,
}

#[derive(Debug)]
pub struct Xp3Error {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for Xp3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for Xp3Error {}

fn err(file: &str, message: impl Into<String>) -> Xp3Error {
    Xp3Error {
        file: file.into(),
        message: message.into(),
    }
}

// ─── Adler-32 (zlib) ───────────────────────────────────────────────────────

/// RFC 1950 Adler-32 (initial a=1, b=0).
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

// ─── LE helpers ────────────────────────────────────────────────────────────

fn read_u16(data: &[u8], off: usize) -> Result<u16, Xp3Error> {
    if off + 2 > data.len() {
        return Err(err("xp3", "truncated u16"));
    }
    Ok(u16::from_le_bytes([data[off], data[off + 1]]))
}

fn read_u32(data: &[u8], off: usize) -> Result<u32, Xp3Error> {
    if off + 4 > data.len() {
        return Err(err("xp3", "truncated u32"));
    }
    Ok(u32::from_le_bytes([
        data[off],
        data[off + 1],
        data[off + 2],
        data[off + 3],
    ]))
}

fn read_u64(data: &[u8], off: usize) -> Result<u64, Xp3Error> {
    if off + 8 > data.len() {
        return Err(err("xp3", "truncated u64"));
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&data[off..off + 8]);
    Ok(u64::from_le_bytes(b))
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn push_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

// ─── Reader ────────────────────────────────────────────────────────────────

/// Resolve index offset: v1 uses the u64 after magic; v2 uses QWORD@0x20 when
/// the first field is `0x17`.
pub fn resolve_index_offset(data: &[u8], label: &str) -> Result<u64, Xp3Error> {
    if data.len() < XP3_MAGIC.len() + 8 {
        return Err(err(label, "file too small for XP3 header"));
    }
    if &data[..XP3_MAGIC.len()] != XP3_MAGIC.as_slice() {
        return Err(err(label, "missing XP3 magic"));
    }
    let first = read_u64(data, XP3_MAGIC.len())?;
    if first == V2_INDEX_MARKER {
        if data.len() < V2_REAL_INDEX_OFFSET_POS + 8 {
            return Err(err(
                label,
                "XP3 v2 header incomplete (expected real index offset at 0x20)",
            ));
        }
        let real = read_u64(data, V2_REAL_INDEX_OFFSET_POS)?;
        if real < data.len() as u64 && real > V2_REAL_INDEX_OFFSET_POS as u64 {
            return Ok(real);
        }
        return Err(err(
            label,
            format!("XP3 v2 real index offset out of range: {real}"),
        ));
    }
    if first as usize >= data.len() {
        return Err(err(
            label,
            format!("XP3 index offset out of range: {first}"),
        ));
    }
    Ok(first)
}

/// Decode the index blob starting at `index_offset` (flag + sizes + payload).
pub fn decode_index_blob(data: &[u8], index_offset: u64, label: &str) -> Result<Vec<u8>, Xp3Error> {
    let off = index_offset as usize;
    if off >= data.len() {
        return Err(err(label, "index offset past EOF"));
    }
    let flag = data[off];
    match flag {
        0 => {
            // raw: u64 size + bytes
            let size = read_u64(data, off + 1)? as usize;
            let start = off + 1 + 8;
            let end = start
                .checked_add(size)
                .filter(|&e| e <= data.len())
                .ok_or_else(|| err(label, "raw index size exceeds file"))?;
            Ok(data[start..end].to_vec())
        }
        1 => {
            // zlib: u64 compressed + u64 uncompressed + stream
            let comp_size = read_u64(data, off + 1)? as usize;
            let uncomp_size = read_u64(data, off + 1 + 8)? as usize;
            let start = off + 1 + 16;
            if start.checked_add(comp_size).is_none_or(|e| e > data.len()) {
                return Err(err(label, "zlib index size exceeds file"));
            }
            let compressed = &data[start..start + comp_size];
            let plain = miniz_oxide::inflate::decompress_to_vec_zlib(compressed).map_err(|e| {
                err(label, format!("failed to inflate zlib index: {e:?}"))
            })?;
            if uncomp_size != 0 && plain.len() != uncomp_size {
                warn!(
                    file = label,
                    expected = uncomp_size,
                    got = plain.len(),
                    "XP3 index uncompressed size mismatch; using inflated length"
                );
            }
            Ok(plain)
        }
        other => Err(err(
            label,
            format!("unsupported XP3 index flag {other} (expected 0=raw or 1=zlib)"),
        )),
    }
}

fn parse_index_entries(index: &[u8], label: &str) -> Result<Vec<Xp3Entry>, Xp3Error> {
    let mut entries = Vec::new();
    let mut pos = 0usize;
    while pos + 12 <= index.len() {
        let tag = read_u32(index, pos)?;
        let size = read_u64(index, pos + 4)? as usize;
        let body_start = pos + 12;
        if body_start.checked_add(size).is_none_or(|e| e > index.len()) {
            return Err(err(label, "index chunk size exceeds index"));
        }
        let body = &index[body_start..body_start + size];
        if tag == TAG_FILE {
            entries.push(parse_file_chunk(body, label)?);
        }
        // Skip unknown top-level chunks by declared size.
        pos = body_start + size;
    }
    Ok(entries)
}

fn parse_file_chunk(body: &[u8], label: &str) -> Result<Xp3Entry, Xp3Error> {
    let mut name = String::new();
    let mut orig_size = 0u64;
    let mut packed_size = 0u64;
    let mut flags = 0u32;
    let mut adler = 0u32;
    let mut segments = Vec::new();

    let mut pos = 0usize;
    while pos + 12 <= body.len() {
        let tag = read_u32(body, pos)?;
        let size = read_u64(body, pos + 4)? as usize;
        let sub_start = pos + 12;
        if sub_start.checked_add(size).is_none_or(|e| e > body.len()) {
            return Err(err(label, "File sub-chunk size exceeds parent"));
        }
        let sub = &body[sub_start..sub_start + size];
        match tag {
            TAG_INFO => {
                if sub.len() < 4 + 8 + 8 + 2 {
                    return Err(err(label, "info chunk too short"));
                }
                flags = read_u32(sub, 0)?;
                // protected bit: tolerate, treat as normal
                let _protected = flags & FLAG_PROTECTED != 0;
                let _ = _protected;
                orig_size = read_u64(sub, 4)?;
                packed_size = read_u64(sub, 12)?;
                let name_units = read_u16(sub, 20)? as usize;
                let name_bytes = name_units * 2;
                if sub.len() < 22 + name_bytes {
                    return Err(err(label, "info name truncated"));
                }
                let units: Vec<u16> = sub[22..22 + name_bytes]
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                name = String::from_utf16_lossy(&units).replace('\\', "/");
            }
            TAG_SEGM => {
                if !sub.len().is_multiple_of(SEGM_ENTRY_SIZE) {
                    return Err(err(
                        label,
                        format!(
                            "segm size {} not multiple of {SEGM_ENTRY_SIZE}",
                            sub.len()
                        ),
                    ));
                }
                for chunk in sub.chunks_exact(SEGM_ENTRY_SIZE) {
                    let sflags = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    let mut o = [0u8; 8];
                    o.copy_from_slice(&chunk[4..12]);
                    let offset = u64::from_le_bytes(o);
                    o.copy_from_slice(&chunk[12..20]);
                    let osize = u64::from_le_bytes(o);
                    o.copy_from_slice(&chunk[20..28]);
                    let psize = u64::from_le_bytes(o);
                    segments.push(Xp3Segment {
                        flags: sflags,
                        offset,
                        orig_size: osize,
                        packed_size: psize,
                    });
                }
            }
            TAG_ADLR => {
                if sub.len() < 4 {
                    return Err(err(label, "adlr chunk too short"));
                }
                adler = read_u32(sub, 0)?;
            }
            _ => {
                // skip unknown sub-chunk
            }
        }
        pos = sub_start + size;
    }

    if name.is_empty() {
        return Err(err(label, "File chunk missing info/name"));
    }
    Ok(Xp3Entry {
        name,
        orig_size,
        packed_size,
        flags,
        adler32: adler,
        segments,
    })
}

impl Xp3Archive {
    pub fn open(path: &Path) -> Result<Self, Xp3Error> {
        let label = path.display().to_string();
        let data = std::fs::read(path).map_err(|e| err(&label, format!("read failed: {e}")))?;
        Self::from_bytes(path.to_path_buf(), data)
    }

    pub fn from_bytes(path: PathBuf, data: Vec<u8>) -> Result<Self, Xp3Error> {
        let label = path.display().to_string();
        let index_off = resolve_index_offset(&data, &label)?;
        let index = decode_index_blob(&data, index_off, &label)?;
        let entries = parse_index_entries(&index, &label)?;
        Ok(Self {
            path,
            data,
            entries,
        })
    }

    /// Read and concatenate all segments for an entry. Adler mismatch → warn only.
    pub fn read_entry(&self, entry: &Xp3Entry) -> Result<Vec<u8>, Xp3Error> {
        let label = self.path.display().to_string();
        // Cap pre-allocation at archive size: orig_size is untrusted input.
        let cap = (entry.orig_size as usize).min(self.data.len());
        let mut out = Vec::with_capacity(cap);
        for seg in &entry.segments {
            let start = seg.offset as usize;
            let packed = seg.packed_size as usize;
            if start.checked_add(packed).is_none_or(|e| e > self.data.len()) {
                return Err(err(
                    &label,
                    format!("segment for {} out of range", entry.name),
                ));
            }
            let slice = &self.data[start..start + packed];
            if seg.is_zlib() {
                let plain = miniz_oxide::inflate::decompress_to_vec_zlib(slice).map_err(|e| {
                    err(
                        &label,
                        format!("zlib segment inflate failed for {}: {e:?}", entry.name),
                    )
                })?;
                out.extend_from_slice(&plain);
            } else {
                out.extend_from_slice(slice);
            }
        }
        if entry.adler32 != 0 {
            let got = adler32(&out);
            if got != entry.adler32 {
                warn!(
                    archive = %self.path.display(),
                    entry = %entry.name,
                    expected = entry.adler32,
                    got,
                    "XP3 adler32 mismatch; using payload anyway"
                );
            }
        }
        Ok(out)
    }

    /// Entries whose name ends with `.ks` (case-insensitive).
    pub fn ks_entries(&self) -> impl Iterator<Item = &Xp3Entry> {
        self.entries.iter().filter(|e| {
            Path::new(&e.name)
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("ks"))
                .unwrap_or(false)
        })
    }
}

// ─── Writer (patch.xp3) ────────────────────────────────────────────────────

struct PlannedFile {
    name: String,
    offset: u64,
    size: u64,
    adler: u32,
}

/// Build a valid unencrypted XP3 (v1 header, zlib-compressed index, one raw
/// segment per file, correct adler32).
pub fn write_xp3(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, Xp3Error> {
    let mut out = Vec::new();
    out.extend_from_slice(XP3_MAGIC);
    push_u64(&mut out, 0); // index offset filled later

    let mut planned = Vec::with_capacity(files.len());
    for (name, payload) in files {
        let offset = out.len() as u64;
        out.extend_from_slice(payload);
        planned.push(PlannedFile {
            name: name.replace('/', "\\"),
            offset,
            size: payload.len() as u64,
            adler: adler32(payload),
        });
    }

    let index_body = build_index_body(&planned);
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&index_body, 6);

    let index_offset = out.len() as u64;
    out.push(1); // zlib index
    push_u64(&mut out, compressed.len() as u64);
    push_u64(&mut out, index_body.len() as u64);
    out.extend_from_slice(&compressed);

    let off_bytes = index_offset.to_le_bytes();
    out[XP3_MAGIC.len()..XP3_MAGIC.len() + 8].copy_from_slice(&off_bytes);

    Ok(out)
}

/// Build index body with a raw (uncompressed) index wrapper available for tests.
pub fn write_xp3_raw_index(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, Xp3Error> {
    let mut out = Vec::new();
    out.extend_from_slice(XP3_MAGIC);
    push_u64(&mut out, 0);

    let mut planned = Vec::with_capacity(files.len());
    for (name, payload) in files {
        let offset = out.len() as u64;
        out.extend_from_slice(payload);
        planned.push(PlannedFile {
            name: name.replace('/', "\\"),
            offset,
            size: payload.len() as u64,
            adler: adler32(payload),
        });
    }

    let index_body = build_index_body(&planned);
    let index_offset = out.len() as u64;
    out.push(0); // raw index
    push_u64(&mut out, index_body.len() as u64);
    out.extend_from_slice(&index_body);

    let off_bytes = index_offset.to_le_bytes();
    out[XP3_MAGIC.len()..XP3_MAGIC.len() + 8].copy_from_slice(&off_bytes);
    Ok(out)
}

/// Low-level writer used by tests to build multi-segment / zlib-segment archives.
pub fn write_xp3_custom(
    files: &[(String, Vec<u8>, /* zlib segment */ bool)],
) -> Result<Vec<u8>, Xp3Error> {
    let mut out = Vec::new();
    out.extend_from_slice(XP3_MAGIC);
    push_u64(&mut out, 0);

    struct Cust {
        name: String,
        offset: u64,
        orig: u64,
        packed: u64,
        zlib: bool,
        adler: u32,
    }
    let mut planned = Vec::new();
    for (name, payload, zlib) in files {
        let offset = out.len() as u64;
        let adler = adler32(payload);
        let (stored, packed_size) = if *zlib {
            let c = miniz_oxide::deflate::compress_to_vec_zlib(payload, 6);
            let len = c.len() as u64;
            out.extend_from_slice(&c);
            (payload.len() as u64, len)
        } else {
            out.extend_from_slice(payload);
            (payload.len() as u64, payload.len() as u64)
        };
        planned.push(Cust {
            name: name.replace('/', "\\"),
            offset,
            orig: stored,
            packed: packed_size,
            zlib: *zlib,
            adler,
        });
    }

    let mut index = Vec::new();
    let mut stack = Vec::new();
    for p in &planned {
        begin_chunk(&mut index, &mut stack, TAG_FILE);
        begin_chunk(&mut index, &mut stack, TAG_INFO);
        push_u32(&mut index, 0);
        push_u64(&mut index, p.orig);
        push_u64(&mut index, p.packed);
        let units: Vec<u16> = p.name.encode_utf16().collect();
        push_u16(&mut index, units.len() as u16);
        for u in units {
            push_u16(&mut index, u);
        }
        end_chunk(&mut index, &mut stack);
        begin_chunk(&mut index, &mut stack, TAG_SEGM);
        push_u32(&mut index, if p.zlib { SEGM_FLAG_ZLIB } else { 0 });
        push_u64(&mut index, p.offset);
        push_u64(&mut index, p.orig);
        push_u64(&mut index, p.packed);
        end_chunk(&mut index, &mut stack);
        begin_chunk(&mut index, &mut stack, TAG_ADLR);
        push_u32(&mut index, p.adler);
        end_chunk(&mut index, &mut stack);
        end_chunk(&mut index, &mut stack);
    }

    let index_offset = out.len() as u64;
    out.push(0);
    push_u64(&mut out, index.len() as u64);
    out.extend_from_slice(&index);
    let off_bytes = index_offset.to_le_bytes();
    out[XP3_MAGIC.len()..XP3_MAGIC.len() + 8].copy_from_slice(&off_bytes);
    Ok(out)
}

fn build_index_body(planned: &[PlannedFile]) -> Vec<u8> {
    let mut index = Vec::new();
    let mut stack = Vec::new();
    for p in planned {
        begin_chunk(&mut index, &mut stack, TAG_FILE);
        begin_chunk(&mut index, &mut stack, TAG_INFO);
        push_u32(&mut index, 0);
        push_u64(&mut index, p.size);
        push_u64(&mut index, p.size);
        let units: Vec<u16> = p.name.encode_utf16().collect();
        push_u16(&mut index, units.len() as u16);
        for u in units {
            push_u16(&mut index, u);
        }
        end_chunk(&mut index, &mut stack);
        begin_chunk(&mut index, &mut stack, TAG_SEGM);
        push_u32(&mut index, 0);
        push_u64(&mut index, p.offset);
        push_u64(&mut index, p.size);
        push_u64(&mut index, p.size);
        end_chunk(&mut index, &mut stack);
        begin_chunk(&mut index, &mut stack, TAG_ADLR);
        push_u32(&mut index, p.adler);
        end_chunk(&mut index, &mut stack);
        end_chunk(&mut index, &mut stack);
    }
    index
}

fn begin_chunk(buf: &mut Vec<u8>, stack: &mut Vec<usize>, tag: u32) {
    push_u32(buf, tag);
    push_u64(buf, 0);
    stack.push(buf.len() - 8);
}

fn end_chunk(buf: &mut [u8], stack: &mut Vec<usize>) {
    let size_pos = stack.pop().expect("chunk stack underflow");
    let body_start = size_pos + 8;
    let size = (buf.len() - body_start) as u64;
    buf[size_pos..size_pos + 8].copy_from_slice(&size.to_le_bytes());
}

/// Write archive with protected flag set on the first file (test helper).
pub fn write_xp3_protected(name: &str, payload: &[u8]) -> Result<Vec<u8>, Xp3Error> {
    let mut out = Vec::new();
    out.extend_from_slice(XP3_MAGIC);
    push_u64(&mut out, 0);
    let offset = out.len() as u64;
    out.extend_from_slice(payload);
    let size = payload.len() as u64;
    let adler = adler32(payload);
    let name_store = name.replace('/', "\\");

    let mut index = Vec::new();
    let mut stack = Vec::new();
    begin_chunk(&mut index, &mut stack, TAG_FILE);
    begin_chunk(&mut index, &mut stack, TAG_INFO);
    push_u32(&mut index, FLAG_PROTECTED);
    push_u64(&mut index, size);
    push_u64(&mut index, size);
    let units: Vec<u16> = name_store.encode_utf16().collect();
    push_u16(&mut index, units.len() as u16);
    for u in units {
        push_u16(&mut index, u);
    }
    end_chunk(&mut index, &mut stack);
    begin_chunk(&mut index, &mut stack, TAG_SEGM);
    push_u32(&mut index, 0);
    push_u64(&mut index, offset);
    push_u64(&mut index, size);
    push_u64(&mut index, size);
    end_chunk(&mut index, &mut stack);
    begin_chunk(&mut index, &mut stack, TAG_ADLR);
    push_u32(&mut index, adler);
    end_chunk(&mut index, &mut stack);
    end_chunk(&mut index, &mut stack);

    let index_offset = out.len() as u64;
    out.push(0);
    push_u64(&mut out, index.len() as u64);
    out.extend_from_slice(&index);
    let off_bytes = index_offset.to_le_bytes();
    out[XP3_MAGIC.len()..XP3_MAGIC.len() + 8].copy_from_slice(&off_bytes);
    Ok(out)
}

/// Corrupt adler32 in an otherwise valid single-file raw-index archive (test helper).
pub fn write_xp3_bad_adler(name: &str, payload: &[u8]) -> Result<Vec<u8>, Xp3Error> {
    let mut out = Vec::new();
    out.extend_from_slice(XP3_MAGIC);
    push_u64(&mut out, 0);
    let offset = out.len() as u64;
    out.extend_from_slice(payload);
    let size = payload.len() as u64;
    let name_store = name.replace('/', "\\");

    let mut index = Vec::new();
    let mut stack = Vec::new();
    begin_chunk(&mut index, &mut stack, TAG_FILE);
    begin_chunk(&mut index, &mut stack, TAG_INFO);
    push_u32(&mut index, 0);
    push_u64(&mut index, size);
    push_u64(&mut index, size);
    let units: Vec<u16> = name_store.encode_utf16().collect();
    push_u16(&mut index, units.len() as u16);
    for u in units {
        push_u16(&mut index, u);
    }
    end_chunk(&mut index, &mut stack);
    begin_chunk(&mut index, &mut stack, TAG_SEGM);
    push_u32(&mut index, 0);
    push_u64(&mut index, offset);
    push_u64(&mut index, size);
    push_u64(&mut index, size);
    end_chunk(&mut index, &mut stack);
    begin_chunk(&mut index, &mut stack, TAG_ADLR);
    push_u32(&mut index, 0xDEAD_BEEF);
    end_chunk(&mut index, &mut stack);
    end_chunk(&mut index, &mut stack);

    let index_offset = out.len() as u64;
    out.push(0);
    push_u64(&mut out, index.len() as u64);
    out.extend_from_slice(&index);
    let off_bytes = index_offset.to_le_bytes();
    out[XP3_MAGIC.len()..XP3_MAGIC.len() + 8].copy_from_slice(&off_bytes);
    Ok(out)
}

/// Multi-segment raw payload (two halves) for concatenation tests.
pub fn write_xp3_multi_segment(
    name: &str,
    part_a: &[u8],
    part_b: &[u8],
) -> Result<Vec<u8>, Xp3Error> {
    let mut out = Vec::new();
    out.extend_from_slice(XP3_MAGIC);
    push_u64(&mut out, 0);
    let off_a = out.len() as u64;
    out.extend_from_slice(part_a);
    let off_b = out.len() as u64;
    out.extend_from_slice(part_b);
    let full: Vec<u8> = part_a.iter().chain(part_b.iter()).copied().collect();
    let adler = adler32(&full);
    let name_store = name.replace('/', "\\");
    let size = full.len() as u64;

    let mut index = Vec::new();
    let mut stack = Vec::new();
    begin_chunk(&mut index, &mut stack, TAG_FILE);
    begin_chunk(&mut index, &mut stack, TAG_INFO);
    push_u32(&mut index, 0);
    push_u64(&mut index, size);
    push_u64(&mut index, size);
    let units: Vec<u16> = name_store.encode_utf16().collect();
    push_u16(&mut index, units.len() as u16);
    for u in units {
        push_u16(&mut index, u);
    }
    end_chunk(&mut index, &mut stack);
    begin_chunk(&mut index, &mut stack, TAG_SEGM);
    push_u32(&mut index, 0);
    push_u64(&mut index, off_a);
    push_u64(&mut index, part_a.len() as u64);
    push_u64(&mut index, part_a.len() as u64);
    push_u32(&mut index, 0);
    push_u64(&mut index, off_b);
    push_u64(&mut index, part_b.len() as u64);
    push_u64(&mut index, part_b.len() as u64);
    end_chunk(&mut index, &mut stack);
    begin_chunk(&mut index, &mut stack, TAG_ADLR);
    push_u32(&mut index, adler);
    end_chunk(&mut index, &mut stack);
    end_chunk(&mut index, &mut stack);

    let index_offset = out.len() as u64;
    out.push(0);
    push_u64(&mut out, index.len() as u64);
    out.extend_from_slice(&index);
    let off_bytes = index_offset.to_le_bytes();
    out[XP3_MAGIC.len()..XP3_MAGIC.len() + 8].copy_from_slice(&off_bytes);
    Ok(out)
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adler32_empty_and_known() {
        assert_eq!(adler32(b""), 1);
        // zlib adler32("Wikipedia") == 0x11E60398
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn test_index_raw_and_zlib() {
        let files = vec![("a.ks".into(), b"hello raw".to_vec())];
        let raw_arch = write_xp3_raw_index(&files).unwrap();
        let z_arch = write_xp3(&files).unwrap();

        // raw index flag
        let off_r = resolve_index_offset(&raw_arch, "raw").unwrap() as usize;
        assert_eq!(raw_arch[off_r], 0);
        // zlib index flag
        let off_z = resolve_index_offset(&z_arch, "z").unwrap() as usize;
        assert_eq!(z_arch[off_z], 1);

        let a = Xp3Archive::from_bytes(PathBuf::from("raw.xp3"), raw_arch).unwrap();
        let b = Xp3Archive::from_bytes(PathBuf::from("z.xp3"), z_arch).unwrap();
        assert_eq!(a.entries.len(), 1);
        assert_eq!(b.entries.len(), 1);
        assert_eq!(a.read_entry(&a.entries[0]).unwrap(), b"hello raw");
        assert_eq!(b.read_entry(&b.entries[0]).unwrap(), b"hello raw");
    }

    #[test]
    fn test_extract_raw_and_zlib_segments() {
        let files_raw = vec![("s.ks".into(), b"RAWSEG".to_vec(), false)];
        let files_z = vec![("s.ks".into(), b"ZLIBSEG".to_vec(), true)];
        let a = Xp3Archive::from_bytes(
            PathBuf::from("r.xp3"),
            write_xp3_custom(&files_raw).unwrap(),
        )
        .unwrap();
        let b = Xp3Archive::from_bytes(
            PathBuf::from("z.xp3"),
            write_xp3_custom(&files_z).unwrap(),
        )
        .unwrap();
        assert!(!a.entries[0].segments[0].is_zlib());
        assert!(b.entries[0].segments[0].is_zlib());
        assert_eq!(a.read_entry(&a.entries[0]).unwrap(), b"RAWSEG");
        assert_eq!(b.read_entry(&b.entries[0]).unwrap(), b"ZLIBSEG");
    }

    #[test]
    fn test_multi_segment_concat() {
        let arch = write_xp3_multi_segment("m.ks", b"AAA", b"BBB").unwrap();
        let a = Xp3Archive::from_bytes(PathBuf::from("m.xp3"), arch).unwrap();
        assert_eq!(a.entries[0].segments.len(), 2);
        assert_eq!(a.read_entry(&a.entries[0]).unwrap(), b"AAABBB");
    }

    #[test]
    fn test_adler_mismatch_still_extracts() {
        let arch = write_xp3_bad_adler("x.ks", b"payload").unwrap();
        let a = Xp3Archive::from_bytes(PathBuf::from("bad.xp3"), arch).unwrap();
        assert_eq!(a.entries[0].adler32, 0xDEAD_BEEF);
        // Still returns payload (warn only)
        assert_eq!(a.read_entry(&a.entries[0]).unwrap(), b"payload");
    }

    #[test]
    fn test_protected_flag_tolerated() {
        let arch = write_xp3_protected("p.ks", b"secret").unwrap();
        let a = Xp3Archive::from_bytes(PathBuf::from("p.xp3"), arch).unwrap();
        assert_ne!(a.entries[0].flags & FLAG_PROTECTED, 0);
        assert_eq!(a.read_entry(&a.entries[0]).unwrap(), b"secret");
    }

    #[test]
    fn test_write_read_roundtrip_byte_identical() {
        let files = vec![
            ("scenario/first.ks".into(), b"line one\nline two".to_vec()),
            ("other.ks".into(), vec![0xFF, 0xFE, 0x41, 0x00]),
        ];
        let bytes = write_xp3(&files).unwrap();
        let a = Xp3Archive::from_bytes(PathBuf::from("patch.xp3"), bytes).unwrap();
        assert_eq!(a.entries.len(), 2);
        for (name, payload) in &files {
            let e = a
                .entries
                .iter()
                .find(|e| e.name == name.replace('\\', "/"))
                .unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(&a.read_entry(e).unwrap(), payload);
        }
    }

    #[test]
    fn test_ks_entries_filter() {
        let files = vec![
            ("a.ks".into(), b"ks".to_vec()),
            ("b.txt".into(), b"txt".to_vec()),
            ("c.KS".into(), b"KS".to_vec()),
        ];
        let a = Xp3Archive::from_bytes(PathBuf::from("f.xp3"), write_xp3(&files).unwrap()).unwrap();
        let names: Vec<_> = a.ks_entries().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"a.ks"));
        assert!(names.contains(&"c.KS"));
        assert!(!names.iter().any(|n| n.ends_with(".txt")));
    }

    #[test]
    fn test_malformed_magic_errors() {
        let err = Xp3Archive::from_bytes(PathBuf::from("x.xp3"), b"not xp3".to_vec())
            .unwrap_err()
            .to_string();
        assert!(err.contains("magic") || err.contains("small"), "{err}");
    }
}
