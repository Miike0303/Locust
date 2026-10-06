//! UnityFS bundle container (signature `UnityFS`) — reader + node relocation writer.
//!
//! Used by player builds that pack SerializedFiles into `*_Data/data.unity3d`.
//!
//! # Layout (AssetStudio `BundleFile` / Unity 5+ archive flags)
//! Header (big-endian after the signature c-string):
//! signature, version u32, unity version c-string, unity revision c-string,
//! size i64, compressedBlocksInfoSize u32, uncompressedBlocksInfoSize u32, flags u32.
//!
//! Flags:
//! - `0x3F` compression of the blocks-info blob (0 none, 1 LZMA, 2 LZ4, 3 LZ4HC)
//! - `0x40` blocks-info and directory combined (always set for UnityFS)
//! - `0x80` blocks-info stored at EOF
//! - `0x200` 16-byte pad after header/blocks-info before data (`BlockInfoNeedPaddingAtStart`)
//!
//! Version ≥ 7: 16-byte align after the header before blocks-info (or data if info-at-end).
//!
//! Blocks-info (after decompress, big-endian): 16-byte hash, block count i32,
//! per block `uncompressedSize u32, compressedSize u32, flags u16`, then node
//! count i32, per node `offset i64, size i64, flags u32, path c-string`.
//!
//! Data blocks concatenate into one uncompressed blob; directory nodes slice it.
//!
//! Writer supports validated node resizing while preserving opaque nodes and
//! recompresses storage blocks with LZ4 (type 2) or stores them uncompressed.

use std::borrow::Cow;
use std::io::Cursor;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const COMPRESSION_MASK: u32 = 0x3F;
const FLAG_COMBINED: u32 = 0x40;
const FLAG_BLOCKS_INFO_AT_END: u32 = 0x80;
const FLAG_BLOCK_INFO_PAD_START: u32 = 0x200;

const COMPRESSION_NONE: u32 = 0;
const COMPRESSION_LZMA: u32 = 1;
const COMPRESSION_LZ4: u32 = 2;
const COMPRESSION_LZ4HC: u32 = 3;

pub const UNITY_FS_SIGNATURE: &str = "UnityFS";
const MAX_UNCOMPRESSED_BYTES: usize = 1024 * 1024 * 1024;
const MAX_INFO_BYTES: usize = 64 * 1024 * 1024;
const MAX_GROWTH_HEADROOM: usize = 8 * 1024 * 1024;
#[cfg(test)]
const MAX_STAGED_COMPRESSED_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct UnityFsError {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for UnityFsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.message)
    }
}

impl std::error::Error for UnityFsError {}

fn err(file: &str, message: impl Into<String>) -> UnityFsError {
    UnityFsError {
        file: file.into(),
        message: message.into(),
    }
}

#[derive(Debug, Clone)]
pub struct UnityFsHeader {
    pub signature: String,
    pub version: u32,
    pub unity_version: String,
    pub unity_revision: String,
    pub size: i64,
    pub compressed_blocks_info_size: u32,
    pub uncompressed_blocks_info_size: u32,
    pub flags: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageBlock {
    pub uncompressed_size: u32,
    pub compressed_size: u32,
    pub flags: u16,
}

impl StorageBlock {
    pub fn compression(&self) -> u32 {
        u32::from(self.flags) & COMPRESSION_MASK
    }
}

#[derive(Debug, Clone)]
pub struct DirectoryNode {
    pub offset: i64,
    pub size: i64,
    pub flags: u32,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct UnityFsArchive {
    pub path: PathBuf,
    pub header: UnityFsHeader,
    pub blocks: Vec<StorageBlock>,
    pub nodes: Vec<DirectoryNode>,
    /// Concatenated uncompressed storage (directory offsets are into this).
    pub uncompressed: Vec<u8>,
    info_hash: [u8; 16],
    /// Compressed backing is shared across clones; it never duplicates the
    /// uncompressed archive. Entries follow the current block plan.
    storage_cache: Vec<Option<CachedBlock>>,
}

#[derive(Debug, Clone)]
struct CachedBlock {
    source: Arc<Vec<u8>>,
    encoded: Range<usize>,
    block: StorageBlock,
}

#[derive(Debug)]
struct CopiedSpan {
    original: Range<usize>,
    output_start: usize,
}

fn record_copy_span(copied: &mut Vec<CopiedSpan>, span: Range<usize>, output_start: usize) {
    if span.is_empty() {
        return;
    }
    if let Some(last) = copied.last_mut().filter(|last| {
        last.original.end == span.start && last.output_start + last.original.len() == output_start
    }) {
        last.original.end = span.end;
    } else {
        copied.push(CopiedSpan {
            original: span,
            output_start,
        });
    }
}

impl UnityFsArchive {
    pub fn parse_path(path: &Path) -> Result<Self, UnityFsError> {
        let label = path.display().to_string();
        let data = std::fs::read(path).map_err(|e| err(&label, format!("read failed: {e}")))?;
        Self::parse(data, path)
    }

    pub fn parse(data: Vec<u8>, path: impl Into<PathBuf>) -> Result<Self, UnityFsError> {
        let path = path.into();
        let label = path.display().to_string();
        parse_archive(data, path, &label)
    }

    /// Release retained compressed input for read-only consumers. Writing still
    /// works after this call, but every storage block will be recompressed.
    /// Archive clones share the backing until their own caches are dropped.
    pub fn discard_storage_cache(&mut self) {
        self.storage_cache.clear();
    }

    pub fn node(&self, node_path: &str) -> Option<&DirectoryNode> {
        let want = node_path.replace('\\', "/");
        self.nodes
            .iter()
            .find(|n| n.path.replace('\\', "/") == want)
    }

    pub fn node_bytes(&self, node: &DirectoryNode) -> Result<&[u8], UnityFsError> {
        let label = self.path.display().to_string();
        let start = usize::try_from(node.offset)
            .map_err(|_| err(&label, format!("node '{}' offset overflow", node.path)))?;
        let size = usize::try_from(node.size)
            .map_err(|_| err(&label, format!("node '{}' size overflow", node.path)))?;
        let end = start
            .checked_add(size)
            .ok_or_else(|| err(&label, format!("node '{}' range overflow", node.path)))?;
        if end > self.uncompressed.len() {
            return Err(err(
                &label,
                format!(
                    "node '{}' range {start}..{end} past uncompressed {}",
                    node.path,
                    self.uncompressed.len()
                ),
            ));
        }
        Ok(&self.uncompressed[start..end])
    }

    /// Edit fixed slots directly in storage, accepting a new buffer only when
    /// the SerializedFile writer must rebuild its object layout.
    pub(crate) fn edit_node(
        &mut self,
        node: &DirectoryNode,
        edit: impl FnOnce(&mut [u8]) -> (bool, Option<Vec<u8>>),
    ) -> Result<bool, UnityFsError> {
        self.node_bytes(node)?;
        let start = node.offset as usize;
        let end = start + node.size as usize;
        let (modified, rebuilt) = edit(&mut self.uncompressed[start..end]);
        if let Some(bytes) = rebuilt {
            if bytes.len() == node.size as usize {
                self.replace_node(&node.path, &bytes)?;
            } else {
                self.resize_node(&node.path, &bytes)?;
            }
        }
        if modified {
            self.info_hash = [0; 16];
        }
        Ok(modified)
    }

    /// Overwrite a node's payload. Size must match (in-place SerializedFile padding).
    pub fn replace_node(&mut self, node_path: &str, data: &[u8]) -> Result<(), UnityFsError> {
        let label = self.path.display().to_string();
        let want = node_path.replace('\\', "/");
        let node = self
            .nodes
            .iter()
            .find(|n| n.path.replace('\\', "/") == want)
            .ok_or_else(|| err(&label, format!("no UnityFS node '{node_path}'")))?;
        let start = usize::try_from(node.offset)
            .map_err(|_| err(&label, format!("node '{node_path}' offset overflow")))?;
        let size = usize::try_from(node.size)
            .map_err(|_| err(&label, format!("node '{node_path}' size overflow")))?;
        if data.len() != size {
            return Err(err(
                &label,
                format!(
                    "node '{node_path}' size changed ({} -> {}); in-place UnityFS rewrite requires identical size",
                    size,
                    data.len()
                ),
            ));
        }
        let end = start + size;
        if end > self.uncompressed.len() {
            return Err(err(
                &label,
                format!("node '{node_path}' past uncompressed blob"),
            ));
        }
        self.uncompressed[start..end].copy_from_slice(data);
        self.info_hash = [0; 16];
        Ok(())
    }

    /// A directory with aliased or overlapping spans cannot be relocated safely.
    pub fn supports_relayout(&self) -> bool {
        self.relayout_order().is_ok()
    }

    fn relayout_order(&self) -> Result<Vec<usize>, UnityFsError> {
        let label = self.path.display().to_string();
        if !(6..=8).contains(&self.header.version)
            || self.header.flags & FLAG_COMBINED == 0
            || self.header.flags
                & !(COMPRESSION_MASK
                    | FLAG_COMBINED
                    | FLAG_BLOCKS_INFO_AT_END
                    | FLAG_BLOCK_INFO_PAD_START)
                != 0
            || self.uncompressed.len() > MAX_UNCOMPRESSED_BYTES
        {
            return Err(err(
                &label,
                "unsupported UnityFS relayout version, flags, or size",
            ));
        }
        let mut paths = std::collections::HashSet::new();
        let mut order: Vec<_> = (0..self.nodes.len()).collect();
        order.sort_by_key(|&i| (self.nodes[i].offset, self.nodes[i].size));
        let mut cursor = 0i64;
        for &i in &order {
            let n = &self.nodes[i];
            self.node_bytes(n)?;
            if n.offset < cursor || !paths.insert(n.path.replace('\\', "/")) {
                return Err(err(
                    &label,
                    "overlapping UnityFS nodes or duplicate node paths",
                ));
            }
            cursor = n
                .offset
                .checked_add(n.size)
                .ok_or_else(|| err(&label, "node span overflow"))?;
        }
        Ok(order)
    }

    /// Replace a node and update every directory span and storage block. Opaque
    /// .resS/.resource nodes, gaps, and trailing storage survive byte-for-byte.
    pub fn resize_node(&mut self, node_path: &str, data: &[u8]) -> Result<(), UnityFsError> {
        let label = self.path.display().to_string();
        let order = self.relayout_order()?;
        let want = node_path.replace('\\', "/");
        let index = self
            .nodes
            .iter()
            .position(|n| n.path.replace('\\', "/") == want)
            .ok_or_else(|| err(&label, format!("no UnityFS node '{node_path}'")))?;
        let target = &self.nodes[index];
        let start = target.offset as usize;
        let end = start + target.size as usize;
        let old_len = self.uncompressed.len();
        let position = order
            .iter()
            .position(|&i| i == index)
            .expect("validated node order");
        let following = order
            .get(position + 1)
            .map(|&i| self.nodes[i].offset as usize);
        let next = following.unwrap_or(old_len);
        let padding = if following.is_some() {
            (target.size as usize).wrapping_sub(data.len()) & 15
        } else {
            0
        };
        let output_len =
            checked_resize_len(old_len, target.size as usize, data.len(), padding, &label)?;
        let new_end = start
            .checked_add(data.len())
            .ok_or_else(|| err(&label, "replacement end overflow"))?;
        let gap_end = new_end
            .checked_add(next - end)
            .ok_or_else(|| err(&label, "node gap overflow"))?;
        let new_next = gap_end
            .checked_add(padding)
            .ok_or_else(|| err(&label, "node alignment overflow"))?;
        if new_next.checked_add(old_len - next) != Some(output_len) {
            return Err(err(&label, "invalid relocation span plan"));
        }
        let mut nodes = self.nodes.clone();
        nodes[index].size = data.len() as i64;
        for &i in &order[position + 1..] {
            let offset = new_next
                .checked_add(self.nodes[i].offset as usize - next)
                .filter(|&offset| offset <= output_len)
                .ok_or_else(|| err(&label, "relocated node offset overflow"))?;
            nodes[i].offset = offset as i64;
        }
        let mut copied = Vec::with_capacity(3);
        record_copy_span(&mut copied, 0..start, 0);
        record_copy_span(&mut copied, end..next, new_end);
        record_copy_span(&mut copied, next..old_len, new_next);
        let (blocks, cache) = self.relocated_blocks(&copied, output_len, &label)?;

        // All validation and metadata allocations precede the first byte edit.
        // A failed reservation preserves the original data and metadata. Once
        // reserved, copy_within handles overlap without a second archive buffer.
        self.uncompressed
            .try_reserve_exact(output_len.saturating_sub(old_len))
            .map_err(|e| err(&label, format!("cannot reserve UnityFS growth: {e}")))?;
        if output_len > old_len {
            self.uncompressed.resize(output_len, 0);
        }
        if data.len() >= target.size as usize {
            self.uncompressed.copy_within(next..old_len, new_next);
            self.uncompressed.copy_within(end..next, new_end);
        } else {
            self.uncompressed.copy_within(end..next, new_end);
            self.uncompressed.copy_within(next..old_len, new_next);
        }
        self.uncompressed[start..new_end].copy_from_slice(data);
        self.uncompressed[gap_end..new_next].fill(0);
        self.uncompressed.truncate(output_len);
        self.storage_cache = cache;
        self.nodes = nodes;
        self.blocks = blocks;
        // A stale original content hash would describe different bytes. UnityFS
        // accepts an unspecified all-zero hash (also used by our fixture writer).
        self.info_hash = [0; 16];
        Ok(())
    }

    fn relocated_blocks(
        &self,
        copied: &[CopiedSpan],
        output_len: usize,
        label: &str,
    ) -> Result<(Vec<StorageBlock>, Vec<Option<CachedBlock>>), UnityFsError> {
        let mut blocks = Vec::new();
        let mut cache = Vec::new();
        let mut old_start = 0usize;
        let mut new_cursor = 0usize;
        let mut run = 0usize;
        for (index, block) in self.blocks.iter().enumerate() {
            let old_end = old_start
                .checked_add(block.uncompressed_size as usize)
                .filter(|&end| end <= self.uncompressed.len())
                .ok_or_else(|| err(label, "storage block plan exceeds uncompressed data"))?;
            while run < copied.len() && copied[run].original.end <= old_start {
                run += 1;
            }
            if block.uncompressed_size != 0 {
                if let Some(span) = copied
                    .get(run)
                    .filter(|span| span.original.start <= old_start && old_end <= span.original.end)
                {
                    let new_start = span.output_start + old_start - span.original.start;
                    append_changed_blocks(new_start - new_cursor, &mut blocks, &mut cache);
                    blocks.push(block.clone());
                    cache.push(self.storage_cache.get(index).cloned().flatten());
                    new_cursor = new_start + block.uncompressed_size as usize;
                }
            }
            old_start = old_end;
        }
        if old_start != self.uncompressed.len() {
            return Err(err(
                label,
                "storage block plan does not cover uncompressed data",
            ));
        }
        append_changed_blocks(output_len - new_cursor, &mut blocks, &mut cache);
        Ok((blocks, cache))
    }

    /// Rebuild bundle bytes from the current directory and storage block plan.
    /// Proven unchanged blocks retain their original compressed bytes. Other
    /// blocks are LZ4-compressed when useful, otherwise stored raw.
    pub fn write_bytes(&self) -> Result<Vec<u8>, UnityFsError> {
        let label = self.path.display().to_string();
        write_archive(self, &label)
    }
}

fn checked_resize_len(
    old_len: usize,
    old_node_size: usize,
    replacement_size: usize,
    padding: usize,
    label: &str,
) -> Result<usize, UnityFsError> {
    old_len
        .checked_sub(old_node_size)
        .and_then(|n| n.checked_add(replacement_size))
        .and_then(|n| n.checked_add(padding))
        .filter(|&n| n <= MAX_UNCOMPRESSED_BYTES)
        .ok_or_else(|| err(label, "UnityFS rebuild exceeds 1 GiB or has invalid sizes"))
}

fn storage_capacity_hint(expected: usize) -> usize {
    expected
        .saturating_add((expected / 64).min(MAX_GROWTH_HEADROOM))
        .min(MAX_UNCOMPRESSED_BYTES)
}

fn append_changed_blocks(
    mut bytes: usize,
    blocks: &mut Vec<StorageBlock>,
    cache: &mut Vec<Option<CachedBlock>>,
) {
    while bytes != 0 {
        let n = bytes.min(128 * 1024);
        blocks.push(StorageBlock {
            uncompressed_size: n as u32,
            compressed_size: 0,
            flags: COMPRESSION_LZ4 as u16,
        });
        cache.push(None);
        bytes -= n;
    }
}

fn cached_encoded<'a>(
    archive: &'a UnityFsArchive,
    index: usize,
    block: &StorageBlock,
    plain: &[u8],
    label: &str,
) -> Option<&'a [u8]> {
    let cached = archive.storage_cache.get(index)?.as_ref()?;
    if &cached.block != block {
        return None;
    }
    let encoded = cached.source.get(cached.encoded.clone())?;
    // Public fields allow external edits without calling replace_node. Exact
    // comparison prevents stale compressed bytes from hiding those edits; no
    // hash heuristic or second whole-archive plaintext snapshot is involved.
    let unchanged = if block.compression() == COMPRESSION_NONE {
        encoded == plain
    } else {
        decompress_blob(
            block.compression(),
            encoded,
            plain.len(),
            label,
            "cached storage block",
        )
        .is_ok_and(|original| original == plain)
    };
    unchanged.then_some(encoded)
}

/// True when `path` is an existing file whose first bytes are `UnityFS\0`.
pub fn is_unity_fs_file(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::Read;
    let mut magic = [0u8; 8];
    matches!(f.read(&mut magic), Ok(n) if n == 8 && &magic == b"UnityFS\0")
}

pub fn is_unity_fs_bytes(data: &[u8]) -> bool {
    data.len() >= 8 && &data[..8] == b"UnityFS\0"
}

/// Directory nodes that are SerializedFiles (not `.resS` / `.resource` blobs).
pub fn is_serialized_bundle_node(node_path: &str) -> bool {
    let lower = node_path.to_ascii_lowercase();
    !(lower.ends_with(".ress") || lower.ends_with(".resource"))
}

fn parse_archive(
    data: Vec<u8>,
    path: PathBuf,
    label: &str,
) -> Result<UnityFsArchive, UnityFsError> {
    if !is_unity_fs_bytes(&data) {
        return Err(err(
            label,
            "not a UnityFS bundle (missing UnityFS signature)",
        ));
    }
    let mut pos = 0usize;
    let signature = read_cstring(&data, &mut pos, label)?;
    if signature != UNITY_FS_SIGNATURE {
        return Err(err(label, format!("unexpected signature {signature:?}")));
    }
    let version = read_u32_be(&data, &mut pos, label)?;
    let unity_version = read_cstring(&data, &mut pos, label)?;
    let unity_revision = read_cstring(&data, &mut pos, label)?;
    let size = read_i64_be(&data, &mut pos, label)?;
    let compressed_blocks_info_size = read_u32_be(&data, &mut pos, label)?;
    let uncompressed_blocks_info_size = read_u32_be(&data, &mut pos, label)?;
    let flags = read_u32_be(&data, &mut pos, label)?;

    if version >= 7 {
        pos = align_up(pos, 16);
    }

    let info_at_end = flags & FLAG_BLOCKS_INFO_AT_END != 0;
    let csize = compressed_blocks_info_size as usize;
    let usize_info = uncompressed_blocks_info_size as usize;
    if usize_info > MAX_INFO_BYTES {
        return Err(err(label, "UnityFS blocks-info exceeds 64 MiB"));
    }
    if csize == 0 || usize_info == 0 {
        return Err(err(label, "blocks-info size is zero"));
    }

    let blocks_info_compr = if info_at_end {
        if data.len() < csize {
            return Err(err(label, "truncated blocks-info at end"));
        }
        &data[data.len() - csize..]
    } else {
        let end = pos
            .checked_add(csize)
            .ok_or_else(|| err(label, "blocks-info range overflow"))?;
        if end > data.len() {
            return Err(err(label, "truncated blocks-info"));
        }
        let slice = &data[pos..end];
        pos = end;
        slice
    };

    let blocks_info = decompress_blob(
        flags & COMPRESSION_MASK,
        blocks_info_compr,
        usize_info,
        label,
        "blocks-info",
    )?;

    if flags & FLAG_BLOCK_INFO_PAD_START != 0 {
        pos = align_up(pos, 16);
    }

    let (info_hash, blocks, nodes) = parse_blocks_info(&blocks_info, label)?;

    let data_limit = if info_at_end {
        data.len().saturating_sub(csize)
    } else {
        data.len()
    };

    let expected_uncomp = blocks
        .iter()
        .try_fold(0usize, |total, b| {
            total
                .checked_add(b.uncompressed_size as usize)
                .filter(|&n| n <= MAX_UNCOMPRESSED_BYTES)
        })
        .ok_or_else(|| err(label, "UnityFS storage exceeds 1 GiB"))?;
    blocks
        .iter()
        .try_fold(pos, |at, b| at.checked_add(b.compressed_size as usize))
        .filter(|&end| end <= data_limit)
        .ok_or_else(|| err(label, "storage block spans exceed bundle data"))?;
    // Allocate bounded growth headroom once while decoding. Without it, even a
    // tiny later insertion may force the allocator to copy the entire archive.
    // Spare capacity is optional; an allocation failure retries the exact size.
    let mut uncompressed = Vec::new();
    uncompressed
        .try_reserve_exact(storage_capacity_hint(expected_uncomp))
        .or_else(|_| uncompressed.try_reserve_exact(expected_uncomp))
        .map_err(|e| err(label, format!("cannot reserve UnityFS storage: {e}")))?;
    let mut encoded_spans = Vec::with_capacity(blocks.len());
    for (i, block) in blocks.iter().enumerate() {
        let csz = block.compressed_size as usize;
        let end = pos
            .checked_add(csz)
            .ok_or_else(|| err(label, format!("storage block {i} range overflow")))?;
        if end > data_limit {
            return Err(err(label, format!("truncated storage block {i}")));
        }
        decompress_blob_into(
            block.compression(),
            &data[pos..end],
            block.uncompressed_size as usize,
            label,
            &format!("storage block {i}"),
            &mut uncompressed,
        )?;
        encoded_spans.push(pos..end);
        pos = end;
    }

    let source = Arc::new(data);
    let storage_cache = blocks
        .iter()
        .zip(encoded_spans)
        .map(|(block, encoded)| {
            Some(CachedBlock {
                source: Arc::clone(&source),
                encoded,
                block: block.clone(),
            })
        })
        .collect();
    Ok(UnityFsArchive {
        path,
        header: UnityFsHeader {
            signature,
            version,
            unity_version,
            unity_revision,
            size,
            compressed_blocks_info_size,
            uncompressed_blocks_info_size,
            flags,
        },
        blocks,
        nodes,
        uncompressed,
        info_hash,
        storage_cache,
    })
}

type ParsedBlocksInfo = ([u8; 16], Vec<StorageBlock>, Vec<DirectoryNode>);

fn parse_blocks_info(info: &[u8], label: &str) -> Result<ParsedBlocksInfo, UnityFsError> {
    if info.len() < 16 {
        return Err(err(label, "blocks-info shorter than 16-byte hash"));
    }
    let mut info_hash = [0u8; 16];
    info_hash.copy_from_slice(&info[..16]);
    let mut pos = 16usize;
    let block_count = read_i32_be(info, &mut pos, label)?;
    if block_count < 0 {
        return Err(err(label, format!("negative block count {block_count}")));
    }
    if block_count as usize > info.len().saturating_sub(pos) / 10 {
        return Err(err(label, "storage block count exceeds remaining info"));
    }
    let mut blocks = Vec::with_capacity(block_count as usize);
    for _ in 0..block_count {
        let uncompressed_size = read_u32_be(info, &mut pos, label)?;
        let compressed_size = read_u32_be(info, &mut pos, label)?;
        let flags = read_u16_be(info, &mut pos, label)?;
        blocks.push(StorageBlock {
            uncompressed_size,
            compressed_size,
            flags,
        });
    }
    let node_count = read_i32_be(info, &mut pos, label)?;
    if node_count < 0 {
        return Err(err(label, format!("negative node count {node_count}")));
    }
    if node_count as usize > info.len().saturating_sub(pos) / 21 {
        return Err(err(label, "directory node count exceeds remaining info"));
    }
    let mut nodes = Vec::with_capacity(node_count as usize);
    for _ in 0..node_count {
        let offset = read_i64_be(info, &mut pos, label)?;
        let size = read_i64_be(info, &mut pos, label)?;
        let flags = read_u32_be(info, &mut pos, label)?;
        let path = read_cstring(info, &mut pos, label)?;
        nodes.push(DirectoryNode {
            offset,
            size,
            flags,
            path,
        });
    }
    Ok((info_hash, blocks, nodes))
}

fn write_archive(archive: &UnityFsArchive, label: &str) -> Result<Vec<u8>, UnityFsError> {
    write_archive_single_pass(archive, label, false)
}

fn reserve_output(
    out: &mut Vec<u8>,
    additional: usize,
    upper_bound: usize,
    label: &str,
) -> Result<bool, UnityFsError> {
    let needed = out
        .len()
        .checked_add(additional)
        .filter(|&n| n <= upper_bound)
        .ok_or_else(|| err(label, "output exceeds checked storage bound"))?;
    if needed <= out.capacity() {
        return Ok(false);
    }
    let wanted = out
        .capacity()
        .saturating_add((out.capacity() / 4).max(8 * 1024 * 1024))
        .max(needed)
        .min(upper_bound);
    out.try_reserve_exact(wanted - out.len())
        .map_err(|e| err(label, format!("cannot grow UnityFS output: {e}")))?;
    Ok(true)
}

/// Encode each changed block once. A front directory initially occupies its
/// uncompressed upper bound, then compacts in the same output allocation.
fn write_archive_single_pass(
    archive: &UnityFsArchive,
    label: &str,
    reserve_upper_bound: bool,
) -> Result<Vec<u8>, UnityFsError> {
    let block_count =
        i32::try_from(archive.blocks.len()).map_err(|_| err(label, "too many storage blocks"))?;
    let node_count =
        i32::try_from(archive.nodes.len()).map_err(|_| err(label, "too many directory nodes"))?;
    let expected = archive
        .blocks
        .iter()
        .try_fold(0usize, |n, b| n.checked_add(b.uncompressed_size as usize))
        .filter(|&n| n == archive.uncompressed.len() && n <= MAX_UNCOMPRESSED_BYTES)
        .ok_or_else(|| err(label, "invalid storage block sizes or uncompressed length"))?;
    let mut info_size = archive
        .blocks
        .len()
        .checked_mul(10)
        .and_then(|n| n.checked_add(24))
        .filter(|&n| n <= MAX_INFO_BYTES)
        .ok_or_else(|| err(label, "blocks-info exceeds 64 MiB"))?;
    for node in &archive.nodes {
        archive.node_bytes(node)?;
        if node.path.contains('\0') {
            return Err(err(label, "NUL in directory node path"));
        }
        info_size = info_size
            .checked_add(21)
            .and_then(|n| n.checked_add(node.path.len()))
            .filter(|&n| n <= MAX_INFO_BYTES)
            .ok_or_else(|| err(label, "blocks-info exceeds 64 MiB"))?;
    }
    if archive.header.unity_version.contains('\0') || archive.header.unity_revision.contains('\0') {
        return Err(err(label, "NUL in UnityFS header string"));
    }
    // Cache decisions are verified once before reserving output. Changed blocks
    // cannot grow when encoded: incompressible data is emitted as raw storage.
    let mut cached = Vec::with_capacity(archive.blocks.len());
    let mut encoded_bound = 0usize;
    let mut original_encoded = 0usize;
    let mut cursor = 0usize;
    for (index, block) in archive.blocks.iter().enumerate() {
        let n = block.uncompressed_size as usize;
        let bytes = cached_encoded(
            archive,
            index,
            block,
            &archive.uncompressed[cursor..cursor + n],
            label,
        );
        encoded_bound = encoded_bound
            .checked_add(bytes.map_or(n, |b| b.len()))
            .ok_or_else(|| err(label, "encoded storage bound overflow"))?;
        original_encoded = original_encoded
            .checked_add(block.compressed_size as usize)
            .ok_or_else(|| err(label, "original compressed size overflow"))?;
        cached.push(bytes);
        cursor += n;
    }
    debug_assert_eq!(cursor, expected);
    let mut out = Vec::new();
    write_cstring(&mut out, UNITY_FS_SIGNATURE);
    write_u32_be(&mut out, archive.header.version);
    write_cstring(&mut out, &archive.header.unity_version);
    write_cstring(&mut out, &archive.header.unity_revision);
    let size_pos = out.len();
    write_i64_be(&mut out, 0);
    write_u32_be(&mut out, 0);
    write_u32_be(&mut out, 0);
    write_u32_be(&mut out, 0);
    if archive.header.version >= 7 {
        pad_to(&mut out, 16);
    }
    let header_end = out.len();
    let at_end = archive.header.flags & FLAG_BLOCKS_INFO_AT_END != 0;
    let padded = archive.header.flags & FLAG_BLOCK_INFO_PAD_START != 0;
    let payload_start = header_end
        .checked_add(if at_end { 0 } else { info_size })
        .and_then(|n| {
            if padded {
                n.checked_add(15).map(|v| v & !15)
            } else {
                Some(n)
            }
        })
        .ok_or_else(|| err(label, "directory reservation overflow"))?;
    let suffix_bound = if at_end { info_size } else { 0 };
    let output_bound = payload_start
        .checked_add(encoded_bound)
        .and_then(|n| n.checked_add(suffix_bound))
        .ok_or_else(|| err(label, "output bound overflow"))?;
    i64::try_from(output_bound).map_err(|_| err(label, "output bound exceeds i64"))?;
    // The conservative hint affects allocation only. Every append remains
    // checked against the exact raw-storage upper bound and can grow safely.
    let payload_hint = if reserve_upper_bound {
        encoded_bound
    } else {
        original_encoded
            .saturating_add(original_encoded / 4)
            .saturating_add(8 * 1024 * 1024)
            .min(encoded_bound)
    };
    let initial_capacity = payload_start + payload_hint + suffix_bound;
    out.try_reserve_exact(initial_capacity - out.len())
        .map_err(|e| err(label, format!("cannot reserve UnityFS output: {e}")))?;
    out.resize(payload_start, 0);
    let mut info = Vec::with_capacity(info_size);
    info.extend_from_slice(&archive.info_hash);
    write_i32_be(&mut info, block_count);
    let mut reused = 0usize;
    let mut reallocations = 0usize;
    cursor = 0;
    for (block, saved) in archive.blocks.iter().zip(cached) {
        let n = block.uncompressed_size as usize;
        let (bytes, flags) = if let Some(encoded) = saved {
            reused += 1;
            (Cow::Borrowed(encoded), block.flags)
        } else {
            pack_block(&archive.uncompressed[cursor..cursor + n], block)
        };
        let len =
            u32::try_from(bytes.len()).map_err(|_| err(label, "compressed block exceeds u32"))?;
        reallocations += usize::from(reserve_output(&mut out, bytes.len(), output_bound, label)?);
        out.extend_from_slice(&bytes);
        write_u32_be(&mut info, block.uncompressed_size);
        write_u32_be(&mut info, len);
        write_u16_be(&mut info, flags);
        cursor += n;
    }
    write_i32_be(&mut info, node_count);
    for node in &archive.nodes {
        write_i64_be(&mut info, node.offset);
        write_i64_be(&mut info, node.size);
        write_u32_be(&mut info, node.flags);
        write_cstring(&mut info, &node.path);
    }
    if info.len() != info_size {
        return Err(err(label, "directory size changed during encoding"));
    }
    let (encoded_info, compression) = pack_blob(&info);
    if encoded_info.len() > info_size {
        return Err(err(label, "encoded directory exceeds reserved bound"));
    }
    let compacted_bytes = if at_end {
        reallocations += usize::from(reserve_output(
            &mut out,
            encoded_info.len(),
            output_bound,
            label,
        )?);
        out.extend_from_slice(&encoded_info);
        0
    } else {
        let info_end = header_end + encoded_info.len();
        let final_start = if padded {
            align_up(info_end, 16)
        } else {
            info_end
        };
        let payload_len = out.len() - payload_start;
        if final_start != payload_start {
            out.copy_within(payload_start.., final_start);
        }
        out.truncate(final_start + payload_len);
        out[header_end..info_end].copy_from_slice(&encoded_info);
        out[info_end..final_start].fill(0);
        if final_start != payload_start {
            payload_len
        } else {
            0
        }
    };
    let size = i64::try_from(out.len()).map_err(|_| err(label, "output size exceeds i64"))?;
    let flags = (archive.header.flags & !COMPRESSION_MASK) | compression | FLAG_COMBINED;
    out[size_pos..size_pos + 8].copy_from_slice(&size.to_be_bytes());
    out[size_pos + 8..size_pos + 12].copy_from_slice(&(encoded_info.len() as u32).to_be_bytes());
    out[size_pos + 12..size_pos + 16].copy_from_slice(&(info.len() as u32).to_be_bytes());
    out[size_pos + 16..size_pos + 20].copy_from_slice(&flags.to_be_bytes());
    tracing::debug!(
        reused_blocks = reused,
        rebuilt_blocks = archive.blocks.len() - reused,
        initial_capacity,
        final_capacity = out.capacity(),
        output_bound,
        reallocations,
        compacted_bytes,
        reserve_upper_bound,
        "UnityFS single pass block assembly"
    );
    Ok(out)
}

fn pack_block<'a>(plain: &'a [u8], original: &StorageBlock) -> (Cow<'a, [u8]>, u16) {
    let extra = original.flags & !(COMPRESSION_MASK as u16);
    if original.compression() == COMPRESSION_NONE {
        return (Cow::Borrowed(plain), extra | COMPRESSION_NONE as u16);
    }
    let packed = lz4_flex::compress(plain);
    if packed.len() < plain.len() {
        (Cow::Owned(packed), extra | COMPRESSION_LZ4 as u16)
    } else {
        (Cow::Borrowed(plain), extra | COMPRESSION_NONE as u16)
    }
}

fn pack_blob(plain: &[u8]) -> (Vec<u8>, u32) {
    let packed = lz4_flex::compress(plain);
    if packed.len() < plain.len() {
        (packed, COMPRESSION_LZ4)
    } else {
        (plain.to_vec(), COMPRESSION_NONE)
    }
}

fn decompress_blob(
    kind: u32,
    src: &[u8],
    uncompressed: usize,
    label: &str,
    what: &str,
) -> Result<Vec<u8>, UnityFsError> {
    let mut out = Vec::new();
    decompress_blob_into(kind, src, uncompressed, label, what, &mut out)?;
    Ok(out)
}

// Decode directly into the archive's destination allocation. A single storage
// block can span the entire archive; staging it would duplicate that buffer.
fn decompress_blob_into(
    kind: u32,
    src: &[u8],
    uncompressed: usize,
    label: &str,
    what: &str,
    out: &mut Vec<u8>,
) -> Result<(), UnityFsError> {
    match kind {
        COMPRESSION_NONE => {
            if src.len() != uncompressed {
                return Err(err(
                    label,
                    format!(
                        "{what}: uncompressed blob {} bytes, expected {uncompressed}",
                        src.len()
                    ),
                ));
            }
            out.extend_from_slice(src);
            Ok(())
        }
        COMPRESSION_LZ4 | COMPRESSION_LZ4HC => {
            let start = out.len();
            out.resize(start + uncompressed, 0);
            let written = lz4_flex::decompress_into(src, &mut out[start..])
                .map_err(|e| err(label, format!("{what}: LZ4 decompress failed: {e}")))?;
            out.truncate(start + written);
            Ok(())
        }
        COMPRESSION_LZMA => decompress_lzma_into(src, uncompressed, label, what, out),
        other => Err(err(
            label,
            format!("{what}: unsupported compression type {other}"),
        )),
    }
}

/// Unity LZMA storage: 5-byte properties + payload (no 8-byte unpacked-size field).
fn decompress_lzma_into(
    src: &[u8],
    uncompressed: usize,
    label: &str,
    what: &str,
    out: &mut Vec<u8>,
) -> Result<(), UnityFsError> {
    use std::io::Read;
    if src.len() < 5 {
        return Err(err(
            label,
            format!("{what}: LZMA blob shorter than 5-byte properties"),
        ));
    }
    let mut header = [0; 13];
    header[..5].copy_from_slice(&src[..5]);
    header[5..].copy_from_slice(&(uncompressed as u64).to_le_bytes());
    let mut input = Cursor::new(header).chain(&src[5..]);
    let start = out.len();
    out.reserve(uncompressed);
    lzma_rs::lzma_decompress(&mut input, out)
        .map_err(|e| err(label, format!("{what}: LZMA decompress failed: {e}")))?;
    let written = out.len() - start;
    if written != uncompressed {
        return Err(err(
            label,
            format!("{what}: LZMA produced {written} bytes, expected {uncompressed}"),
        ));
    }
    Ok(())
}

fn align_up(pos: usize, n: usize) -> usize {
    debug_assert!(n.is_power_of_two());
    let mask = n - 1;
    if pos & mask == 0 {
        pos
    } else {
        (pos + mask) & !mask
    }
}

fn pad_to(buf: &mut Vec<u8>, n: usize) {
    let aligned = align_up(buf.len(), n);
    buf.resize(aligned, 0);
}

fn need<'a>(data: &'a [u8], pos: usize, n: usize, label: &str) -> Result<&'a [u8], UnityFsError> {
    let end = pos
        .checked_add(n)
        .ok_or_else(|| err(label, "offset overflow"))?;
    if end > data.len() {
        return Err(err(label, format!("truncated read ({n} bytes at {pos})")));
    }
    Ok(&data[pos..end])
}

fn read_u16_be(data: &[u8], pos: &mut usize, label: &str) -> Result<u16, UnityFsError> {
    let b = need(data, *pos, 2, label)?;
    *pos += 2;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

fn read_u32_be(data: &[u8], pos: &mut usize, label: &str) -> Result<u32, UnityFsError> {
    let b = need(data, *pos, 4, label)?;
    *pos += 4;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_i32_be(data: &[u8], pos: &mut usize, label: &str) -> Result<i32, UnityFsError> {
    Ok(read_u32_be(data, pos, label)? as i32)
}

fn read_i64_be(data: &[u8], pos: &mut usize, label: &str) -> Result<i64, UnityFsError> {
    let b = need(data, *pos, 8, label)?;
    *pos += 8;
    Ok(i64::from_be_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

fn read_cstring(data: &[u8], pos: &mut usize, label: &str) -> Result<String, UnityFsError> {
    let start = *pos;
    while *pos < data.len() && data[*pos] != 0 {
        *pos += 1;
    }
    if *pos >= data.len() {
        return Err(err(label, "unterminated c-string"));
    }
    let s = String::from_utf8_lossy(&data[start..*pos]).into_owned();
    *pos += 1;
    Ok(s)
}

fn write_u16_be(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn write_u32_be(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn write_i32_be(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn write_i64_be(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn write_cstring(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
}

/// Build a synthetic UnityFS (unit tests).
///
/// `lz4` compresses the storage block. `version` 7/8 emit the 16-byte header
/// align; `pad_start` sets `0x200`; `info_at_end` sets `0x80`.
#[cfg(test)]
pub fn build_test_bundle(
    files: &[(&str, &[u8])],
    lz4: bool,
    version: u32,
    pad_start: bool,
    info_at_end: bool,
) -> Vec<u8> {
    let mut uncompressed = Vec::new();
    let mut nodes = Vec::new();
    for &(name, data) in files {
        let offset = uncompressed.len() as i64;
        uncompressed.extend_from_slice(data);
        nodes.push(DirectoryNode {
            offset,
            size: data.len() as i64,
            flags: 4,
            path: name.to_string(),
        });
    }
    let block_flags = if lz4 {
        COMPRESSION_LZ4 as u16
    } else {
        COMPRESSION_NONE as u16
    };
    let packed_block = if lz4 {
        lz4_flex::compress(&uncompressed)
    } else {
        uncompressed.clone()
    };
    let blocks = vec![StorageBlock {
        uncompressed_size: uncompressed.len() as u32,
        compressed_size: packed_block.len() as u32,
        flags: block_flags,
    }];
    let mut flags = FLAG_COMBINED;
    if pad_start {
        flags |= FLAG_BLOCK_INFO_PAD_START;
    }
    if info_at_end {
        flags |= FLAG_BLOCKS_INFO_AT_END;
    }
    let archive = UnityFsArchive {
        path: PathBuf::from("test.unity3d"),
        header: UnityFsHeader {
            signature: UNITY_FS_SIGNATURE.into(),
            version,
            unity_version: "5.x.x".into(),
            unity_revision: "6000.0.24f1".into(),
            size: 0,
            compressed_blocks_info_size: 0,
            uncompressed_blocks_info_size: 0,
            flags,
        },
        blocks,
        nodes,
        uncompressed,
        info_hash: [0u8; 16],
        storage_cache: Vec::new(),
    };
    archive.write_bytes().expect("test bundle write")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unity_serialized::{write_v17_fixture, SerializedFile};

    fn cab_bytes() -> Vec<u8> {
        write_v17_fixture("Speaker", "Hello from the bundle.")
    }

    #[test]
    fn parse_uncompressed_bundle_exposes_serialized_node() {
        let cab = cab_bytes();
        let bytes = build_test_bundle(&[("CAB-test", &cab)], false, 6, false, false);
        assert!(is_unity_fs_bytes(&bytes));
        let archive = UnityFsArchive::parse(bytes, "t.unity3d").unwrap();
        assert_eq!(archive.nodes.len(), 1);
        assert_eq!(archive.nodes[0].path, "CAB-test");
        let got = archive.node_bytes(&archive.nodes[0]).unwrap();
        assert_eq!(got, cab.as_slice());
        let sf = SerializedFile::parse(got.to_vec(), "CAB-test").unwrap();
        let ta = sf.read_text_asset(1).unwrap();
        assert_eq!(ta.script, "Hello from the bundle.");
    }

    #[test]
    fn parse_lz4_bundle_roundtrip() {
        let cab = cab_bytes();
        let bytes = build_test_bundle(&[("CAB-lz4", &cab)], true, 8, true, false);
        let archive = UnityFsArchive::parse(bytes, "lz4.unity3d").unwrap();
        assert_eq!(archive.header.version, 8);
        assert_eq!(
            archive.node_bytes(&archive.nodes[0]).unwrap(),
            cab.as_slice()
        );

        let rewritten = archive.write_bytes().unwrap();
        let again = UnityFsArchive::parse(rewritten, "lz4-out.unity3d").unwrap();
        assert_eq!(again.node_bytes(&again.nodes[0]).unwrap(), cab.as_slice());
        assert_eq!(
            again.header.size as usize,
            again.write_bytes().unwrap().len()
        );
    }

    #[test]
    fn parse_lz4_info_at_end_and_v7_align() {
        let cab = cab_bytes();
        let bytes = build_test_bundle(&[("CAB-end", &cab)], true, 8, true, true);
        let archive = UnityFsArchive::parse(bytes, "end.unity3d").unwrap();
        assert!(archive.header.flags & FLAG_BLOCKS_INFO_AT_END != 0);
        assert_eq!(
            archive.node_bytes(&archive.nodes[0]).unwrap(),
            cab.as_slice()
        );
    }

    #[test]
    fn replace_node_same_size_rewrite() {
        let cab = cab_bytes();
        let bytes = build_test_bundle(&[("CAB-a", &cab)], true, 8, true, false);
        let mut archive = UnityFsArchive::parse(bytes, "rep.unity3d").unwrap();
        let mut patched = archive.node_bytes(&archive.nodes[0]).unwrap().to_vec();
        let i = patched
            .windows(b"Hello".len())
            .position(|w| w == b"Hello")
            .expect("fixture contains Hello");
        patched[i] = b'J';
        archive.replace_node("CAB-a", &patched).unwrap();
        let out = archive.write_bytes().unwrap();
        let again = UnityFsArchive::parse(out, "rep2.unity3d").unwrap();
        let got = again.node_bytes(&again.nodes[0]).unwrap();
        assert_eq!(got.len(), cab.len());
        assert_eq!(&got[i..i + 5], b"Jello");
    }

    #[test]
    fn replace_node_rejects_size_change() {
        let cab = cab_bytes();
        let bytes = build_test_bundle(&[("CAB-a", &cab)], false, 6, false, false);
        let mut archive = UnityFsArchive::parse(bytes, "sz.unity3d").unwrap();
        let e = archive.replace_node("CAB-a", b"short").unwrap_err();
        assert!(e.message.contains("size changed"), "{}", e.message);
    }

    #[test]
    fn two_nodes_uncompressed() {
        let a = cab_bytes();
        let b = write_v17_fixture("Other", "Second cab body.");
        let bytes = build_test_bundle(&[("CAB-1", &a), ("CAB-2", &b)], false, 8, true, false);
        let archive = UnityFsArchive::parse(bytes, "two.unity3d").unwrap();
        assert_eq!(archive.nodes.len(), 2);
        assert_eq!(archive.node_bytes(&archive.nodes[0]).unwrap(), a.as_slice());
        assert_eq!(archive.node_bytes(&archive.nodes[1]).unwrap(), b.as_slice());
        assert!(is_serialized_bundle_node("CAB-1"));
        assert!(!is_serialized_bundle_node("CAB-1.resS"));
        assert!(!is_serialized_bundle_node("shared.resource"));
    }

    #[test]
    fn lzma_storage_block_decompresses() {
        let payload = b"UnityFS LZMA storage block payload for tests.";
        let mut lzma_std = Vec::new();
        lzma_rs::lzma_compress(&mut Cursor::new(&payload[..]), &mut lzma_std).unwrap();
        assert!(lzma_std.len() > 13);
        let unity_lzma: Vec<u8> = lzma_std[..5]
            .iter()
            .copied()
            .chain(lzma_std[13..].iter().copied())
            .collect();
        let got = decompress_blob(
            COMPRESSION_LZMA,
            &unity_lzma,
            payload.len(),
            "lzma-test",
            "block",
        )
        .unwrap();
        assert_eq!(got, payload);
    }
}

#[cfg(test)]
#[path = "unity_block_reuse_tests.rs"]
mod block_reuse_tests;

#[cfg(test)]
#[path = "unity_inplace_tests.rs"]
mod inplace_tests;

#[cfg(test)]
#[path = "unity_writer_tests.rs"]
mod writer_tests;

#[cfg(test)]
#[path = "unity_singlepass_tests.rs"]
mod singlepass_tests;

#[cfg(test)]
include!("unity_writer_staging_reference.rs");
