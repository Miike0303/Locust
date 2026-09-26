//! IoStore discovery and companion-PAK localization routing, not a chunk reader.
//!
//! UE's normal Zen layout stores package/bulk/shader data in UTOC/UCAS and loose
//! files in companion PAKs. This lets localization extraction work in that layout
//! without pretending to decode Zen assets. ExternalFile chunks exist, so absence
//! of LocRes in PAKs does NOT prove absence of text inside an IoStore container.
//! Sources and exact support boundary: docs/UNREAL-IOSTORE.md.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub const TOC_MAGIC: &[u8; 16] = b"-==--==--==--==-";
const HEADER_SIZE: usize = 144;

/// Identification only: no chunk tables, directory index or UCAS data are read.
/// Even unknown TOC versions can be identified to route to separate PAKs.
pub fn is_toc(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(meta) = file.metadata() else {
        return false;
    };
    let mut header = [0u8; HEADER_SIZE];
    if file.read_exact(&mut header).is_err() || &header[..16] != TOC_MAGIC {
        return false;
    }
    let declared_size = u32::from_le_bytes(header[20..24].try_into().unwrap()) as u64;
    header[16] != 0 && declared_size >= HEADER_SIZE as u64 && declared_size <= meta.len()
}

pub fn is_container_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("utoc") || e.eq_ignore_ascii_case("ucas"))
}

fn sibling_toc(path: &Path) -> Option<PathBuf> {
    if is_toc(path) {
        return Some(path.to_path_buf());
    }
    // Case-insensitive filename matching also works on case-sensitive hosts.
    let wanted = path.file_name()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::read_dir(parent)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(wanted))
                && is_toc(p)
        })
}

/// UCAS has no standalone container header. Identify it only via a matching TOC,
/// including UE's `<container>_sN.ucas` partition naming convention.
pub fn toc_for_container(path: &Path) -> Option<PathBuf> {
    if !path.is_file() {
        return None;
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("utoc"))
    {
        return is_toc(path).then(|| path.to_path_buf());
    }
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ucas"))
    {
        return None;
    }
    if let Some(toc) = sibling_toc(&path.with_extension("utoc")) {
        return Some(toc);
    }
    let stem = path.file_stem()?.to_str()?;
    let lower = stem.to_ascii_lowercase();
    let (base, partition) = lower.rsplit_once("_s")?;
    if base.is_empty() || partition.is_empty() || !partition.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    sibling_toc(&path.with_file_name(format!("{base}.utoc")))
}

/// Depth agrees with Unreal PAK discovery. Never read a UCAS payload or follow
/// directory symlinks; one fixed-size TOC header is enough for identification.
pub fn find_toc(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pak"))
        {
            return sibling_toc(&path.with_extension("utoc"));
        }
        return toc_for_container(path);
    }
    walkdir::WalkDir::new(path)
        .max_depth(5)
        .follow_links(false)
        .into_iter()
        .filter_entry(crate::discovery::is_game_entry)
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .find(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("utoc"))
                && is_toc(p)
        })
}

pub fn no_localization_message(toc: &Path) -> String {
    format!(
        "IoStore container detected: {}. No readable localization strings were found in \
         supported native ExternalFile chunks, companion .pak or loose .locres files. Open the complete game folder including \
         its localization PAKs, or export .locres using UnrealPak/a compatible IoStore tool \
         and open the exported files. Native support is limited to explicitly indexed \
         ExternalFile .locres chunks in unencrypted/unsigned TOC v5/v7/v8 using None/Zlib/LZ4; \
         text embedded in Zen assets is not decoded. This result does not mean the game contains no text.",
        toc.display()
    )
}
