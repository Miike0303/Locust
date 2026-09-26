//! Bounded reader for explicitly named UE5 IoStore ExternalFile LocRes chunks.
//!
//! Supports TOC versions 5, 7 and 8, unsigned/unencrypted directory indexes,
//! and None/Zlib/LZ4 blocks. It never guesses filenames or parses Zen assets.
//! Layout references: retoc 885a8dae, `lib.rs` TOC/directory readers and
//! CUE4Parse `IoStoreReader.cs` partition/block addressing. See native support doc.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::unreal_iostore::TOC_MAGIC;
use crate::unreal_pak::{canonical_inner_name, is_locres_record_name, mounted_resource_path};

pub const MAX_TOC_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_LOCRES_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_BLOCKS: usize = 4_000_000;
const MAX_BLOCK_BYTES: usize = 16 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 16 * 1024;
const MAX_TOTAL_PATH_BYTES: usize = 16 * 1024 * 1024;
const MAX_OPEN_PARTITIONS: usize = 16;
const INVALID: u32 = u32::MAX;

#[derive(Debug)]
pub struct IoStoreError {
    pub message: String,
    /// Container-level unavailable capability may coexist with readable PAKs.
    /// Malformed supported tables must never be silently ignored.
    pub unsupported: bool,
}

impl std::fmt::Display for IoStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for IoStoreError {}
type Result<T> = std::result::Result<T, IoStoreError>;

fn invalid(message: impl Into<String>) -> IoStoreError {
    IoStoreError {
        message: message.into(),
        unsupported: false,
    }
}
fn unsupported(message: impl Into<String>) -> IoStoreError {
    IoStoreError {
        message: message.into(),
        unsupported: true,
    }
}
fn io(error: std::io::Error) -> IoStoreError {
    invalid(error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IoStoreRecord {
    /// Name relative to the recorded mount point, preserving case.
    pub name: String,
    pub virtual_path: String,
    pub chunk_index: u32,
    pub chunk_type: u8,
}

pub struct IoStoreIndex {
    pub toc_path: PathBuf,
    pub version: u8,
    pub mount_point: String,
    pub records: Vec<IoStoreRecord>,
    // Immutable directory authority: public enumeration can be reordered or
    // edited by a caller without authorizing a forged record for extraction.
    records_by_name: HashMap<String, IoStoreRecord>,
    data: Vec<u8>,
    chunk_offsets: usize,
    entry_count: usize,
    blocks: usize,
    block_count: usize,
    metas: usize,
    meta_width: usize,
    block_size: u64,
    partition_count: u32,
    partition_size: u64,
    methods: Vec<String>,
}

fn u32_at(data: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(data[off..off + 4].try_into().unwrap())
}
fn u64_at(data: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap())
}
fn be40(data: &[u8]) -> u64 {
    data.iter().fold(0, |n, &byte| (n << 8) | byte as u64)
}
fn le40(data: &[u8]) -> u64 {
    data.iter().rev().fold(0, |n, &byte| (n << 8) | byte as u64)
}
fn le24(data: &[u8]) -> usize {
    data[0] as usize | ((data[1] as usize) << 8) | ((data[2] as usize) << 16)
}
fn advance(pos: &mut u64, count: u64, width: u64) -> Result<usize> {
    let start = *pos;
    *pos = count
        .checked_mul(width)
        .and_then(|n| pos.checked_add(n))
        .ok_or_else(|| invalid("IoStore table offset overflow"))?;
    if *pos > MAX_TOC_BYTES {
        return Err(invalid("IoStore table metadata exceeds 128 MiB limit"));
    }
    Ok(start as usize)
}

/// Reads only bounded TOC metadata. UCAS is opened only for a selected LocRes.
pub fn read_index(path: &Path) -> Result<IoStoreIndex> {
    let mut file = File::open(path).map_err(io)?;
    let file_len = file.metadata().map_err(io)?.len();
    let mut header = [0u8; 144];
    file.read_exact(&mut header)
        .map_err(|_| invalid("truncated IoStore header"))?;
    if &header[..16] != TOC_MAGIC {
        return Err(invalid("invalid IoStore TOC magic"));
    }
    let version = header[16];
    if !matches!(version, 5 | 7 | 8) {
        return Err(unsupported(format!("IoStore TOC version {version} is unsupported for native LocRes (supported: 5, 7, 8; version 6 on-demand metadata is not decoded)")));
    }
    if u32_at(&header, 20) != 144 {
        return Err(invalid("unsupported IoStore header size (expected 144)"));
    }
    let flags = header[80];
    if flags & 2 != 0 {
        return Err(unsupported(
            "encrypted IoStore containers are unsupported; export .locres with a compatible tool",
        ));
    }
    if flags & 4 != 0 {
        return Err(unsupported(
            "signed IoStore containers are unsupported; signatures are not verified",
        ));
    }
    if flags & !15 != 0 {
        return Err(unsupported("unknown IoStore container flags"));
    }
    if flags & 8 == 0 || u32_at(&header, 48) == 0 {
        return Err(unsupported(
            "IoStore directory index is absent; native LocRes paths cannot be established",
        ));
    }
    let count = u32_at(&header, 24) as usize;
    let block_count = u32_at(&header, 28) as usize;
    if count > MAX_ENTRIES || block_count > MAX_BLOCKS {
        return Err(invalid("IoStore entry/block count exceeds limit"));
    }
    if u32_at(&header, 32) != 12 {
        return Err(invalid(
            "unsupported IoStore compressed block entry size (expected 12)",
        ));
    }
    let methods_count = u32_at(&header, 36) as usize;
    let method_width = u32_at(&header, 40) as usize;
    if methods_count > 255 || (methods_count > 0 && method_width != 32) {
        return Err(invalid("invalid IoStore compression method table"));
    }
    let block_size = u32_at(&header, 44) as u64;
    if block_size == 0 || block_size > MAX_BLOCK_BYTES as u64 || !block_size.is_power_of_two() {
        return Err(invalid("invalid IoStore compression block size"));
    }
    let partition_count = u32_at(&header, 52);
    let partition_size = u64_at(&header, 88);
    if partition_count == 0 || partition_count > 1024 || partition_size == 0 {
        return Err(invalid("invalid IoStore partition count/size"));
    }
    let seeds = u32_at(&header, 84);
    let overflow = u32_at(&header, 96);
    if seeds as usize > MAX_ENTRIES || overflow as usize > count {
        return Err(invalid("IoStore perfect-hash table count exceeds limit"));
    }
    let mut pos = 144u64;
    let ids = advance(&mut pos, count as u64, 12)?;
    let chunk_offsets = advance(&mut pos, count as u64, 10)?;
    advance(&mut pos, seeds as u64, 4)?;
    let overflow_pos = advance(&mut pos, overflow as u64, 4)?;
    let blocks = advance(&mut pos, block_count as u64, 12)?;
    let methods_pos = advance(&mut pos, methods_count as u64, method_width as u64)?;
    let directory_size = u32_at(&header, 48) as u64;
    let directory_pos = advance(&mut pos, directory_size, 1)?;
    // Meta layout changes from 32-byte chunk hash + flags to IoHash20 + flags + pad.
    let meta_width = if version == 8 { 24 } else { 33 };
    let metas = advance(&mut pos, count as u64, meta_width as u64)?;
    if pos > file_len {
        return Err(invalid("truncated IoStore tables/directory/entry metadata"));
    }
    let mut data = vec![0u8; pos as usize];
    data[..144].copy_from_slice(&header);
    file.read_exact(&mut data[144..]).map_err(io)?;
    for i in 0..overflow as usize {
        if u32_at(&data, overflow_pos + i * 4) as usize >= count {
            return Err(invalid(
                "IoStore perfect-hash overflow index is out of range",
            ));
        }
    }
    let mut methods = Vec::with_capacity(methods_count);
    for i in 0..methods_count {
        let slot = &data[methods_pos + i * method_width..methods_pos + (i + 1) * method_width];
        let end = slot
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| invalid("unterminated IoStore compression method name"))?;
        let name = std::str::from_utf8(&slot[..end])
            .map_err(|_| invalid("invalid IoStore compression method UTF-8"))?;
        if name.is_empty() {
            return Err(invalid("empty IoStore compression method name"));
        }
        methods.push(name.to_string());
    }
    let (mount_point, names) = read_directory(
        &data[directory_pos..directory_pos + directory_size as usize],
        count,
    )?;
    let mut records = Vec::new();
    let mut total_virtual_paths = 0usize;
    for (name, chunk_index) in names {
        if !is_locres_record_name(&name) {
            continue;
        }
        let virtual_path =
            mounted_resource_path(&mount_point, &name).map_err(|e| invalid(e.message))?;
        total_virtual_paths = total_virtual_paths
            .checked_add(virtual_path.len())
            .filter(|&total| total <= MAX_TOTAL_PATH_BYTES)
            .ok_or_else(|| invalid("IoStore virtual paths exceed aggregate limit"))?;
        let chunk_type = data[ids + chunk_index as usize * 12 + 11];
        records.push(IoStoreRecord {
            name,
            virtual_path,
            chunk_index,
            chunk_type,
        });
    }
    records.sort_by(|a, b| a.virtual_path.cmp(&b.virtual_path));
    let records_by_name = records
        .iter()
        .map(|r| (r.name.clone(), r.clone()))
        .collect();
    Ok(IoStoreIndex {
        toc_path: path.into(),
        version,
        mount_point,
        records,
        records_by_name,
        data,
        chunk_offsets,
        entry_count: count,
        blocks,
        block_count,
        metas,
        meta_width,
        block_size,
        partition_count,
        partition_size,
        methods,
    })
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| invalid("truncated IoStore directory index"))?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn count(&mut self, width: usize) -> Result<usize> {
        let count = self.u32()? as usize;
        if count > MAX_ENTRIES || count > (self.data.len() - self.pos) / width {
            return Err(invalid("IoStore directory table count exceeds bounds"));
        }
        Ok(count)
    }
    fn string(&mut self) -> Result<String> {
        let len = self.u32()? as i32;
        if len == 0 {
            return Ok(String::new());
        }
        let count = len.unsigned_abs() as usize;
        if count > MAX_PATH_BYTES {
            return Err(invalid("IoStore FString exceeds length limit"));
        }
        if len > 0 {
            let bytes = self.take(count)?;
            if bytes.last() != Some(&0) || bytes[..count - 1].contains(&0) {
                return Err(invalid("invalid IoStore FString terminator"));
            }
            std::str::from_utf8(&bytes[..count - 1])
                .map(str::to_string)
                .map_err(|_| invalid("invalid IoStore FString UTF-8"))
        } else {
            let bytes = self.take(count * 2)?;
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            if units.last() != Some(&0) || units[..count - 1].contains(&0) {
                return Err(invalid("invalid IoStore UTF-16 terminator"));
            }
            String::from_utf16(&units[..count - 1])
                .map_err(|_| invalid("invalid IoStore FString UTF-16"))
        }
    }
}

fn component(strings: &[String], index: u32) -> Result<&str> {
    let name = strings
        .get(index as usize)
        .ok_or_else(|| invalid("IoStore directory name index out of range"))?;
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        return Err(invalid("unsafe IoStore directory/file path component"));
    }
    Ok(name)
}

fn read_directory(data: &[u8], chunk_count: usize) -> Result<(String, Vec<(String, u32)>)> {
    let mut r = Reader { data, pos: 0 };
    let mount = r.string()?;
    // Reuse the PAK virtual-root rules before any directory path can be exposed.
    mounted_resource_path(&mount, "probe.locres").map_err(|e| invalid(e.message))?;
    let dir_count = r.count(16)?;
    let mut dirs = Vec::with_capacity(dir_count);
    for _ in 0..dir_count {
        dirs.push([r.u32()?, r.u32()?, r.u32()?, r.u32()?]);
    }
    let file_count = r.count(12)?;
    let mut files = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        files.push([r.u32()?, r.u32()?, r.u32()?]);
    }
    let str_count = r.count(4)?;
    let mut strings = Vec::with_capacity(str_count);
    for _ in 0..str_count {
        strings.push(r.string()?);
    }
    if r.pos != data.len() {
        return Err(invalid(
            "trailing bytes in unencrypted IoStore directory index",
        ));
    }
    if dirs.is_empty() {
        if files.is_empty() {
            return Ok((mount, Vec::new()));
        }
        return Err(invalid("IoStore directory root is missing"));
    }
    if dirs[0][0] != INVALID || dirs[0][2] != INVALID {
        return Err(invalid("invalid IoStore directory root"));
    }
    let mut seen_dirs = vec![false; dir_count];
    let mut seen_files = vec![false; file_count];
    let mut stack = vec![(0usize, String::new(), 0usize)];
    seen_dirs[0] = true;
    let mut names = Vec::new();
    let mut unique = HashSet::new();
    let mut total_paths = 0usize;
    while let Some((dir_index, prefix, depth)) = stack.pop() {
        let dir = dirs[dir_index];
        let mut file_id = dir[3];
        while file_id != INVALID {
            let index = file_id as usize;
            let file = files
                .get(index)
                .ok_or_else(|| invalid("IoStore file link out of range"))?;
            if seen_files[index] {
                return Err(invalid("cyclic or shared IoStore file link"));
            }
            seen_files[index] = true;
            if file[2] as usize >= chunk_count {
                return Err(invalid("IoStore directory chunk index out of range"));
            }
            let part = component(&strings, file[0])?;
            let name = if prefix.is_empty() {
                part.to_string()
            } else {
                format!("{prefix}/{part}")
            };
            if name.len() > MAX_PATH_BYTES {
                return Err(invalid("IoStore resource path exceeds length limit"));
            }
            total_paths += name.len();
            if total_paths > MAX_TOTAL_PATH_BYTES {
                return Err(invalid("IoStore directory paths exceed aggregate limit"));
            }
            canonical_inner_name(&name).map_err(|e| invalid(e.message))?;
            if !unique.insert(name.to_ascii_lowercase()) {
                return Err(invalid("duplicate IoStore resource path"));
            }
            names.push((name, file[2]));
            file_id = file[1];
        }
        let mut child_id = dir[1];
        while child_id != INVALID {
            let index = child_id as usize;
            let child = dirs
                .get(index)
                .ok_or_else(|| invalid("IoStore directory link out of range"))?;
            if seen_dirs[index] {
                return Err(invalid("cyclic or shared IoStore directory link"));
            }
            seen_dirs[index] = true;
            if depth >= 128 {
                return Err(invalid("IoStore directory nesting exceeds limit"));
            }
            let part = component(&strings, child[0])?;
            let name = if prefix.is_empty() {
                part.to_string()
            } else {
                format!("{prefix}/{part}")
            };
            if name.len() > MAX_PATH_BYTES {
                return Err(invalid("IoStore directory path exceeds length limit"));
            }
            total_paths += name.len();
            if total_paths > MAX_TOTAL_PATH_BYTES {
                return Err(invalid("IoStore directory paths exceed aggregate limit"));
            }
            stack.push((index, name, depth + 1));
            child_id = child[2];
        }
    }
    if seen_dirs.contains(&false) || seen_files.contains(&false) {
        return Err(invalid("unreachable IoStore directory/file entries"));
    }
    Ok((mount, names))
}

fn case_insensitive_file(path: PathBuf) -> PathBuf {
    if path.is_file() {
        return path;
    }
    let Some(name) = path.file_name() else {
        return path;
    };
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::read_dir(parent)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .find(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        })
        .unwrap_or(path)
}

impl IoStoreIndex {
    /// Exact native resource lookup backed by the validated directory snapshot.
    pub fn find_record(&self, name: &str) -> Option<&IoStoreRecord> {
        self.records_by_name.get(name)
    }

    /// A chunk may start/end inside a logical block. Read/decode only its intersecting
    /// blocks and slice precisely, validating physical partition and output bounds.
    pub fn read_locres(&self, record: &IoStoreRecord) -> Result<Vec<u8>> {
        if record.chunk_index as usize >= self.entry_count {
            return Err(invalid("IoStore record chunk index out of range"));
        }
        if self.find_record(&record.name) != Some(record) {
            return Err(invalid(
                "IoStore record does not belong to this directory index",
            ));
        }
        if record.chunk_type != 7 {
            return Err(unsupported(format!(
                "{} is not an ExternalFile chunk (type {})",
                record.name, record.chunk_type
            )));
        }
        let off = self.chunk_offsets + record.chunk_index as usize * 10;
        let start = be40(&self.data[off..off + 5]);
        let len = be40(&self.data[off + 5..off + 10]);
        if len == 0 || len > MAX_LOCRES_BYTES {
            return Err(invalid(
                "IoStore LocRes size is zero or exceeds 64 MiB limit",
            ));
        }
        let end = start
            .checked_add(len)
            .ok_or_else(|| invalid("IoStore chunk end overflow"))?;
        let first = start / self.block_size;
        let last = (end - 1) / self.block_size;
        if last >= self.block_count as u64 {
            return Err(invalid("IoStore chunk block range is out of bounds"));
        }
        if last - first + 1 > 65_536 {
            return Err(invalid(
                "IoStore LocRes block count exceeds per-resource work limit",
            ));
        }
        let mut output = Vec::with_capacity(len as usize);
        let mut compressed_bytes = 0u64;
        // Reuse handles across blocks of this resource, with a hard descriptor
        // bound. Do not retain handles between reads: a renamed/replaced UCAS
        // must be re-opened and checked against this TOC's chunk hash on the
        // next call, rather than silently returning an old cached file.
        let mut partitions = BTreeMap::<u64, (File, u64, PathBuf)>::new();
        for i in first..=last {
            let block =
                &self.data[self.blocks + i as usize * 12..self.blocks + (i as usize + 1) * 12];
            let physical = le40(&block[..5]);
            let compressed = le24(&block[5..8]);
            let uncompressed = le24(&block[8..11]);
            let method_id = block[11] as usize;
            if compressed == 0
                || compressed > MAX_BLOCK_BYTES
                || uncompressed == 0
                || uncompressed as u64 > self.block_size
            {
                return Err(invalid(
                    "IoStore compressed/uncompressed block size is invalid",
                ));
            }
            compressed_bytes += compressed as u64;
            if compressed_bytes > MAX_LOCRES_BYTES * 4 {
                return Err(invalid(
                    "IoStore LocRes compressed input exceeds 256 MiB read budget",
                ));
            }
            let method = if method_id == 0 {
                "None"
            } else {
                self.methods
                    .get(method_id - 1)
                    .ok_or_else(|| invalid("IoStore block compression method index out of range"))?
                    .as_str()
            };
            if !["none", "zlib", "lz4"]
                .iter()
                .any(|m| method.eq_ignore_ascii_case(m))
            {
                return Err(unsupported(format!(
                    "IoStore codec {method} is unsupported for {} (supported: None, Zlib, LZ4)",
                    record.name
                )));
            }
            if method_id != 0 && method.eq_ignore_ascii_case("none") {
                return Err(invalid(
                    "IoStore None compression must use method index zero",
                ));
            }
            let partition = physical / self.partition_size;
            let offset = physical % self.partition_size;
            let block_end = offset
                .checked_add(compressed as u64)
                .ok_or_else(|| invalid("IoStore physical block end overflow"))?;
            if partition >= self.partition_count as u64 || block_end > self.partition_size {
                return Err(invalid("IoStore physical block exceeds partition bounds"));
            }
            if !partitions.contains_key(&partition) {
                let path = if partition == 0 {
                    self.toc_path.with_extension("ucas")
                } else {
                    let stem = self
                        .toc_path
                        .file_stem()
                        .ok_or_else(|| invalid("IoStore TOC filename missing"))?
                        .to_string_lossy();
                    self.toc_path
                        .with_file_name(format!("{stem}_s{partition}.ucas"))
                };
                let path = case_insensitive_file(path);
                let file = File::open(&path).map_err(|e| {
                    invalid(format!(
                        "cannot open IoStore partition {}: {e}",
                        path.display()
                    ))
                })?;
                let metadata = file.metadata().map_err(io)?;
                if !metadata.is_file() {
                    return Err(invalid(format!(
                        "IoStore partition is not a regular file {}",
                        path.display()
                    )));
                }
                if partitions.len() == MAX_OPEN_PARTITIONS {
                    partitions.pop_first();
                }
                partitions.insert(partition, (file, metadata.len(), path));
            }
            let (file, file_len, path) = partitions
                .get_mut(&partition)
                .ok_or_else(|| invalid("IoStore partition cache entry missing"))?;
            if block_end > *file_len {
                return Err(invalid(format!(
                    "IoStore block exceeds partition file {}",
                    path.display()
                )));
            }
            file.seek(SeekFrom::Start(offset)).map_err(io)?;
            let mut input = vec![0u8; compressed];
            file.read_exact(&mut input).map_err(io)?;
            let decoded = if method_id == 0 {
                if compressed != uncompressed {
                    return Err(invalid("uncompressed IoStore block sizes differ"));
                }
                input
            } else if method.eq_ignore_ascii_case("zlib") {
                let mut decoded = vec![0u8; uncompressed];
                let mut decoder = flate2::Decompress::new(true);
                let status = decoder
                    .decompress(&input, &mut decoded, flate2::FlushDecompress::Finish)
                    .map_err(|e| invalid(format!("IoStore Zlib decompression failed: {e}")))?;
                if status != flate2::Status::StreamEnd
                    || decoder.total_in() != compressed as u64
                    || decoder.total_out() != uncompressed as u64
                {
                    return Err(invalid("IoStore Zlib stream length/checksum mismatch"));
                }
                decoded
            } else {
                let mut decoded = vec![0u8; uncompressed];
                let written = lz4_flex::block::decompress_into(&input, &mut decoded)
                    .map_err(|e| invalid(format!("IoStore LZ4 decompression failed: {e}")))?;
                if written != uncompressed {
                    return Err(invalid("IoStore LZ4 output length mismatch"));
                }
                decoded
            };
            let logical = i * self.block_size;
            let from = start.saturating_sub(logical) as usize;
            let to = (end - logical).min(self.block_size) as usize;
            let selected = decoded
                .get(from..to)
                .ok_or_else(|| invalid("IoStore chunk exceeds decoded block length"))?;
            output.extend_from_slice(selected);
        }
        if output.len() != len as usize {
            return Err(invalid("IoStore chunk decoded length mismatch"));
        }
        let hash = blake3::hash(&output);
        let meta = self.metas + record.chunk_index as usize * self.meta_width;
        if self.data[meta..meta + 20] != hash.as_bytes()[..20] {
            return Err(invalid("IoStore LocRes BLAKE3 chunk hash mismatch"));
        }
        Ok(output)
    }
}

/// Native extraction only enumerates explicit TOC files, bounded like PAK scanning.
pub fn find_tocs(path: &Path) -> Vec<PathBuf> {
    if !path.is_dir() {
        return Vec::new();
    }
    let mut paths: Vec<_> = walkdir::WalkDir::new(path)
        .max_depth(5)
        .follow_links(false)
        .into_iter()
        .filter_entry(crate::discovery::is_game_entry)
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("utoc"))
                && crate::unreal_iostore::is_toc(p)
        })
        .collect();
    paths.sort();
    paths
}
