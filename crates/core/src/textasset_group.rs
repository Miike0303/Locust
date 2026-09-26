//! Shared Unity TextAsset loc-line / CSV capacity.
//!
//! Localization lines and CSV cells are extracted as separate rows but inject
//! rebuilds one `m_Script` blob. Per-cell byte budgets are therefore wrong: a
//! longer cell can still fit when keys, separators, newlines, padding and
//! untouched cells leave enough room. Metadata recorded at extraction is the
//! source of truth for that reconstruction; it is not secret.
//!
//! Original blobs are immutable and shared in memory, and stored once by digest
//! in the project database. Legacy inline metadata remains readable.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{LocustError, Result};
use crate::models::{StringEntry, TextAssetOriginal, ValidationIssue, ValidationKind};

pub const GROUP_ID_KEY: &str = "textasset_group_id";
pub const GROUP_KIND_KEY: &str = "textasset_group_kind";
pub const GROUP_ORIGINAL_KEY: &str = "textasset_group_original";
pub const GROUP_ORIGINAL_REF_KEY: &str = "textasset_group_original_ref";
pub const GROUP_ORIGINAL_BYTES_KEY: &str = "textasset_group_original_bytes";
pub const GROUP_CAPACITY_KEY: &str = "textasset_group_capacity";
pub const GROUP_OVERHEAD_KEY: &str = "textasset_group_overhead";
pub const GROUP_MEMBER_COUNT_KEY: &str = "textasset_group_members";
pub const GROUP_ERROR_KEY: &str = "textasset_group_error";

/// Explicit resource bounds; the original is never duplicated per member.
pub const MAX_GROUP_MEMBERS: usize = 16_384;
pub const MAX_GROUP_ORIGINAL_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    LocLine,
    Csv,
}

impl GroupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocLine => "loc_line",
            Self::Csv => "csv",
        }
    }

    pub fn from_extraction_method(method: &str) -> Option<Self> {
        match method {
            "textasset_loc_line" => Some(Self::LocLine),
            "textasset_csv_cell" => Some(Self::Csv),
            _ => None,
        }
    }

    fn from_meta(value: &str) -> Option<Self> {
        match value {
            "loc_line" => Some(Self::LocLine),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GroupMeta {
    pub id: String,
    pub kind: GroupKind,
    pub original: Arc<str>,
    pub original_sha256: String,
    pub capacity: usize,
    pub overhead: usize,
    pub member_count: usize,
}

impl PartialEq for GroupMeta {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.kind == other.kind
            && self.original_sha256 == other.original_sha256
            && self.capacity == other.capacity
            && self.overhead == other.overhead
            && self.member_count == other.member_count
    }
}
impl Eq for GroupMeta {}

/// Exact capability granted only by the independently validating Unity writer.
pub fn structural_textasset_capacity(entry: &StringEntry) -> Option<usize> {
    let method = entry.metadata.get("extraction_method")?.as_str()?;
    let version = entry.metadata.get("unity_serialized_version")?.as_u64()?;
    (matches!(
        method,
        "textasset" | "textasset_loc_line" | "textasset_csv_cell"
    ) && (17..=22).contains(&version)
        && entry.metadata.get("textasset_rewrite")?.as_str()? == "serialized-v1")
        .then_some(MAX_GROUP_ORIGINAL_BYTES)
}

pub fn original_textasset(entry: &StringEntry) -> Option<&str> {
    entry
        .textasset_original
        .as_ref()
        .map(|original| original.text())
        .or_else(|| entry.metadata.get(GROUP_ORIGINAL_KEY)?.as_str())
}

/// Resolve and verify compact metadata against an immutable payload. A JSON-only
/// row with a reference must be hydrated by Database before reconstruction.
pub fn shared_original(
    entry: &StringEntry,
) -> std::result::Result<Option<Arc<TextAssetOriginal>>, String> {
    let original = match &entry.textasset_original {
        Some(original) => Some(Arc::clone(original)),
        None => match entry.metadata.get(GROUP_ORIGINAL_KEY) {
            Some(value) => Some(Arc::new(TextAssetOriginal::new(
                value.as_str().ok_or("malformed TextAsset original")?,
            )?)),
            None => None,
        },
    };
    if let Some(value) = entry.metadata.get(GROUP_ORIGINAL_REF_KEY) {
        let reference = value
            .as_str()
            .ok_or("malformed TextAsset original reference")?;
        let original = original
            .as_ref()
            .ok_or("missing shared TextAsset original; reload from its project database")?;
        if reference != original.sha256()
            || entry
                .metadata
                .get(GROUP_ORIGINAL_BYTES_KEY)
                .and_then(|value| value.as_u64())
                != Some(original.text().len() as u64)
        {
            return Err(
                "shared TextAsset original digest or length does not match metadata".into(),
            );
        }
    } else if entry.metadata.contains_key(GROUP_ORIGINAL_BYTES_KEY) {
        return Err("TextAsset original length has no digest reference".into());
    }
    if let (Some(original), Some(legacy)) = (&original, entry.metadata.get(GROUP_ORIGINAL_KEY)) {
        if legacy.as_str() != Some(original.text()) {
            return Err("conflicting inline and shared TextAsset originals".into());
        }
    }
    Ok(original)
}

pub(crate) fn attach_shared_original(entry: &mut StringEntry, original: Arc<TextAssetOriginal>) {
    entry.metadata.remove(GROUP_ORIGINAL_KEY);
    entry.metadata.insert(
        GROUP_ORIGINAL_REF_KEY.into(),
        serde_json::json!(original.sha256()),
    );
    entry.metadata.insert(
        GROUP_ORIGINAL_BYTES_KEY.into(),
        serde_json::json!(original.text().len()),
    );
    entry.textasset_original = Some(original);
}

#[derive(Clone, Debug)]
pub struct CellPatch<'a> {
    pub entry_id: &'a str,
    pub physical_source: &'a str,
    pub translation: &'a str,
    pub line_index: Option<usize>,
    pub loc_key: Option<&'a str>,
    pub csv_row: Option<usize>,
    pub csv_col: Option<usize>,
    pub csv_header: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedBlob {
    pub text: String,
    pub outcomes: Vec<(String, std::result::Result<(), &'static str>)>,
}

pub fn is_grouped_entry(entry: &StringEntry) -> bool {
    GroupKind::from_extraction_method(
        entry
            .metadata
            .get("extraction_method")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    )
    .is_some()
}

fn automatic_group_too_large_message() -> String {
    format!(
        "automatic group too large: automatic shared-budget reconstruction is unsupported \
         (at most {MAX_GROUP_MEMBERS} members, {MAX_GROUP_ORIGINAL_BYTES} original bytes). \
         This is not a per-cell budget."
    )
}

fn too_large_error(entry_id: &str) -> String {
    format!(
        "entry '{entry_id}': {}",
        automatic_group_too_large_message()
    )
}

fn group_exceeds_automatic_bounds(original_len: usize, member_count: usize) -> bool {
    member_count > MAX_GROUP_MEMBERS || original_len > MAX_GROUP_ORIGINAL_BYTES
}

pub fn parse_group_meta(entry: &StringEntry) -> std::result::Result<GroupMeta, String> {
    if !is_grouped_entry(entry) {
        return Err(format!(
            "entry '{}' is not a grouped Unity TextAsset row",
            entry.id
        ));
    }
    if entry.metadata.contains_key(GROUP_ERROR_KEY) {
        return Err(too_large_error(&entry.id));
    }
    let method = entry
        .metadata
        .get("extraction_method")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let Some(kind_from_method) = GroupKind::from_extraction_method(method) else {
        return Err(format!(
            "entry '{}' has unsupported grouped TextAsset layout '{method}'",
            entry.id
        ));
    };
    let kind = match entry.metadata.get(GROUP_KIND_KEY).and_then(|v| v.as_str()) {
        Some(value) => GroupKind::from_meta(value).ok_or_else(|| {
            format!(
                "entry '{}' has unsupported {} '{value}'",
                entry.id, GROUP_KIND_KEY
            )
        })?,
        None => {
            return Err(format!(
                "entry '{}' is missing {GROUP_KIND_KEY}; cannot verify shared TextAsset capacity",
                entry.id
            ))
        }
    };
    if kind != kind_from_method {
        return Err(format!(
            "entry '{}' has conflicting TextAsset group kinds",
            entry.id
        ));
    }
    let id = entry
        .metadata
        .get(GROUP_ID_KEY)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "entry '{}' is missing {GROUP_ID_KEY}; cannot verify shared TextAsset capacity",
                entry.id
            )
        })?
        .to_string();
    let capacity = meta_usize(entry, GROUP_CAPACITY_KEY).ok_or_else(|| {
        format!(
            "entry '{}' is missing {GROUP_CAPACITY_KEY}; cannot verify shared TextAsset capacity",
            entry.id
        )
    })?;
    if capacity == 0 {
        return Err(format!(
            "entry '{}' has invalid shared TextAsset capacity",
            entry.id
        ));
    }
    let member_count = meta_usize(entry, GROUP_MEMBER_COUNT_KEY)
        .filter(|count| *count > 0)
        .ok_or_else(|| format!("entry '{}' has invalid {GROUP_MEMBER_COUNT_KEY}", entry.id))?;
    if member_count > MAX_GROUP_MEMBERS {
        return Err(too_large_error(&entry.id));
    }
    if original_textasset(entry).is_some_and(|text| text.len() > MAX_GROUP_ORIGINAL_BYTES) {
        return Err(too_large_error(&entry.id));
    }
    let shared = shared_original(entry)?.ok_or_else(|| {
        format!(
            "entry '{}' is missing {GROUP_ORIGINAL_KEY}; cannot reconstruct the shared blob",
            entry.id
        )
    })?;
    let original_ref = shared.text();
    if group_exceeds_automatic_bounds(original_ref.len(), member_count) {
        return Err(too_large_error(&entry.id));
    }
    if original_ref.len() > capacity {
        return Err(format!(
            "entry '{}' shared TextAsset original length {} exceeds capacity {capacity}",
            entry.id,
            original_ref.len()
        ));
    }
    let overhead = meta_usize(entry, GROUP_OVERHEAD_KEY).ok_or_else(|| {
        format!(
            "entry '{}' is missing {GROUP_OVERHEAD_KEY}; cannot compute reconstruction overhead",
            entry.id
        )
    })?;
    if overhead > original_ref.len() {
        return Err(format!(
            "entry '{}' has reconstruction overhead larger than the original blob",
            entry.id
        ));
    }
    let original = shared.shared_text();
    Ok(GroupMeta {
        id,
        kind,
        original,
        original_sha256: shared.sha256().to_owned(),
        capacity: structural_textasset_capacity(entry).unwrap_or(capacity),
        overhead,
        member_count,
    })
}

/// Fail closed before provider work when grouped metadata is missing or
/// internally inconsistent. Non-grouped rows are ignored.
pub fn require_valid_metadata(entry: &StringEntry) -> Result<()> {
    if !is_grouped_entry(entry) {
        return Ok(());
    }
    parse_group_meta(entry)
        .map(|_| ())
        .map_err(|message| LocustError::Other(anyhow::anyhow!(message)))
}

pub fn attach_to_entries(
    entries: &mut [StringEntry],
    kind: GroupKind,
    original: &str,
    capacity: usize,
    group_id: impl Into<String>,
) {
    if entries.is_empty() {
        return;
    }
    let group_id = group_id.into();
    let member_count = entries.len();
    let refuse = group_exceeds_automatic_bounds(original.len(), member_count);
    let shared = if refuse {
        None
    } else {
        TextAssetOriginal::new(original).ok().map(Arc::new)
    };
    let diagnostic = if refuse {
        Some(automatic_group_too_large_message())
    } else {
        None
    };
    let overhead = if refuse {
        0
    } else {
        let value_bytes: usize = entries.iter().map(|e| e.source.len()).sum();
        original.len().saturating_sub(value_bytes)
    };
    for entry in entries {
        entry.metadata.insert(
            GROUP_ID_KEY.to_string(),
            serde_json::Value::String(group_id.clone()),
        );
        entry.metadata.insert(
            GROUP_KIND_KEY.to_string(),
            serde_json::Value::String(kind.as_str().to_string()),
        );
        entry
            .metadata
            .insert(GROUP_CAPACITY_KEY.to_string(), serde_json::json!(capacity));
        entry.metadata.insert(
            GROUP_MEMBER_COUNT_KEY.to_string(),
            serde_json::json!(member_count),
        );
        if let Some(message) = diagnostic.as_ref() {
            entry.textasset_original = None;
            entry.metadata.remove(GROUP_ORIGINAL_REF_KEY);
            entry.metadata.remove(GROUP_ORIGINAL_BYTES_KEY);
            entry.metadata.remove(GROUP_ORIGINAL_KEY);
            entry.metadata.remove(GROUP_OVERHEAD_KEY);
            entry.metadata.insert(
                GROUP_ERROR_KEY.to_string(),
                serde_json::Value::String(message.clone()),
            );
        } else {
            entry.metadata.remove(GROUP_ERROR_KEY);
            if let Some(shared) = &shared {
                attach_shared_original(entry, Arc::clone(shared));
            }
            entry
                .metadata
                .insert(GROUP_OVERHEAD_KEY.to_string(), serde_json::json!(overhead));
        }
    }
}

pub fn patch_from_entry<'a>(
    entry: &'a StringEntry,
    translation: &'a str,
) -> std::result::Result<CellPatch<'a>, String> {
    let physical = entry.injection_source()?;
    Ok(CellPatch {
        entry_id: &entry.id,
        physical_source: physical,
        translation,
        line_index: meta_usize(entry, "line_index"),
        loc_key: entry.metadata.get("loc_key").and_then(|v| v.as_str()),
        csv_row: meta_usize(entry, "csv_row"),
        csv_col: meta_usize(entry, "csv_col"),
        csv_header: entry.metadata.get("csv_header").and_then(|v| v.as_str()),
    })
}

/// Rebuild the original blob with the given cell replacements. Failed patches
/// leave that line untouched. This is the same layout inject writes.
pub fn apply_patches(original: &str, kind: GroupKind, patches: &[CellPatch<'_>]) -> AppliedBlob {
    let mut lines: Vec<String> = original.split_inclusive('\n').map(str::to_string).collect();
    let rows: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(i, _)| i)
        .collect();
    let headers: Vec<String> = rows
        .first()
        .map(|i| {
            line_body(&lines[*i])
                .split(',')
                .map(|h| h.trim().to_string())
                .collect()
        })
        .unwrap_or_default();
    let mut used_lines = std::collections::HashSet::new();
    let mut outcomes = Vec::with_capacity(patches.len());
    for patch in patches {
        let result: std::result::Result<usize, &'static str> = (|| {
            if patch.translation.contains(['\r', '\n']) {
                return Err("error");
            }
            match kind {
                GroupKind::Csv => {
                    if patch.translation.contains([',', '"']) {
                        return Err("error");
                    }
                    let row = patch.csv_row.ok_or("error")?;
                    let col = patch.csv_col.ok_or("error")?;
                    if row == 0 {
                        return Err("error");
                    }
                    let i = *rows.get(row).ok_or("target_missing")?;
                    if used_lines.contains(&i) {
                        return Err("error");
                    }
                    let header = headers.get(col).ok_or("target_missing")?;
                    if patch
                        .csv_header
                        .is_some_and(|expected| expected != header.as_str())
                    {
                        return Err("source_changed");
                    }
                    let mut cells: Vec<String> = line_body(&lines[i])
                        .split(',')
                        .map(str::to_string)
                        .collect();
                    if cells.len() != headers.len() {
                        return Err("source_changed");
                    }
                    let cell = cells.get_mut(col).ok_or("target_missing")?;
                    if cell.trim() != patch.physical_source {
                        return Err("source_changed");
                    }
                    let leading = &cell[..cell.len() - cell.trim_start().len()];
                    let trailing = &cell[cell.trim_end().len()..];
                    *cell = format!("{leading}{}{trailing}", patch.translation);
                    let ending = &lines[i][line_body(&lines[i]).len()..];
                    lines[i] = format!("{}{ending}", cells.join(","));
                    Ok(i)
                }
                GroupKind::LocLine => {
                    let i = find_loc_line(
                        &lines,
                        &used_lines,
                        patch.line_index,
                        patch.loc_key,
                        patch.physical_source,
                    )
                    .ok_or("target_missing")?;
                    let body = line_body(&lines[i]);
                    match (patch.loc_key, split_loc_kv(body)) {
                        (Some(key), Some((actual, _sep, value))) if actual == key => {
                            if value != patch.physical_source {
                                return Err("source_changed");
                            }
                            if !body.ends_with(value) {
                                return Err("source_changed");
                            }
                            let ending = &lines[i][body.len()..];
                            lines[i] = format!(
                                "{}{}{ending}",
                                &body[..body.len() - value.len()],
                                patch.translation
                            );
                            Ok(i)
                        }
                        (None, None) => {
                            if body != patch.physical_source {
                                return Err("source_changed");
                            }
                            let ending = &lines[i][body.len()..];
                            lines[i] = format!("{}{ending}", patch.translation);
                            Ok(i)
                        }
                        _ => Err("source_changed"),
                    }
                }
            }
        })();
        match result {
            Ok(i) => {
                used_lines.insert(i);
                outcomes.push((patch.entry_id.to_string(), Ok(())));
            }
            Err(reason) => outcomes.push((patch.entry_id.to_string(), Err(reason))),
        }
    }
    AppliedBlob {
        text: lines.concat(),
        outcomes,
    }
}

/// Consume trailing spaces the same way inject does, then compare to the
/// immutable whole-blob capacity. Never enlarges the slot or truncates values.
pub fn fit_reconstructed(rebuilt: String, capacity: usize) -> std::result::Result<String, usize> {
    let mut rebuilt = rebuilt;
    if rebuilt.len() > capacity {
        let excess = rebuilt.len() - capacity;
        let padding = rebuilt.len() - rebuilt.trim_end_matches(' ').len();
        if padding >= excess {
            rebuilt.truncate(capacity);
        }
    }
    if rebuilt.len() > capacity {
        Err(rebuilt.len())
    } else {
        Ok(rebuilt)
    }
}

pub fn reconstructed_fits(
    meta: &GroupMeta,
    patches: &[CellPatch<'_>],
) -> std::result::Result<bool, &'static str> {
    let applied = apply_patches(&meta.original, meta.kind, patches);
    if let Some((_, Err(reason))) = applied.outcomes.iter().find(|(_, result)| result.is_err()) {
        return Err(*reason);
    }
    Ok(fit_reconstructed(applied.text, meta.capacity).is_ok())
}

pub fn reconstructed_len(
    meta: &GroupMeta,
    patches: &[CellPatch<'_>],
) -> std::result::Result<usize, &'static str> {
    let applied = apply_patches(&meta.original, meta.kind, patches);
    if let Some((_, Err(reason))) = applied.outcomes.iter().find(|(_, result)| result.is_err()) {
        return Err(*reason);
    }
    match fit_reconstructed(applied.text, meta.capacity) {
        Ok(text) => Ok(text.len()),
        Err(actual) => Ok(actual),
    }
}

pub fn first_pass_hint(meta: &GroupMeta, selected_count: usize) -> String {
    let shared = meta.capacity.saturating_sub(meta.overhead);
    format!(
        "SHARED TEXTASSET BUDGET: reconstructed blob must be ≤ {} UTF-8 bytes. \
         Keys, separators, newlines, padding and other cells already use {} bytes \
         (not the sum of visible cell text). {} selected value(s) share the remaining {} bytes; \
         this is not a per-cell limit.",
        meta.capacity, meta.overhead, selected_count, shared
    )
}

pub fn retry_correction(capacity: usize, actual: usize, previous: &str) -> String {
    let excess = actual.saturating_sub(capacity);
    let prev_display = if previous.chars().count() > 80 {
        let truncated: String = previous.chars().take(80).collect();
        format!("{truncated}…")
    } else {
        previous.to_string()
    };
    format!(
        "PREVIOUS RECONSTRUCTED BLOB WAS {actual} BYTES — HARD LIMIT {capacity} BYTES (utf8). \
         Previous text: «{prev_display}». Remove at least {excess} byte(s) from the shared blob. \
         Shorten aggressively: drop articles/vowels/spaces, use abbreviations; \
         return ONLY the shortened translation."
    )
}

pub fn group_validation_issues(entries: &[StringEntry]) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut groups: HashMap<String, Vec<&StringEntry>> = HashMap::new();
    for entry in entries {
        if !is_grouped_entry(entry) {
            continue;
        }
        if let Ok(meta) = parse_group_meta(entry) {
            groups.entry(meta.id).or_default().push(entry);
        }
    }
    for members in groups.values() {
        let Some(head) = members.first() else {
            continue;
        };
        let Ok(meta) = parse_group_meta(head) else {
            continue;
        };
        if members
            .iter()
            .any(|e| parse_group_meta(e).ok().as_ref() != Some(&meta))
        {
            for entry in members {
                issues.push(ValidationIssue {
                    entry_id: entry.id.clone(),
                    kind: ValidationKind::InvalidInjectionProvenance,
                    message: format!(
                        "entry '{}' has conflicting shared TextAsset group metadata",
                        entry.id
                    ),
                    source: None,
                });
            }
            continue;
        }
        let mut patches = Vec::new();
        for entry in members {
            let Some(translation) = entry.translation.as_deref() else {
                continue;
            };
            if translation.is_empty() || translation == entry.source {
                continue;
            }
            match patch_from_entry(entry, translation) {
                Ok(patch) => patches.push(patch),
                Err(message) => issues.push(ValidationIssue {
                    entry_id: entry.id.clone(),
                    kind: ValidationKind::InvalidInjectionProvenance,
                    message,
                    source: None,
                }),
            }
        }
        if patches.is_empty() {
            continue;
        }
        let applied = apply_patches(&meta.original, meta.kind, &patches);
        for (id, result) in &applied.outcomes {
            if let Err(reason) = result {
                if *reason == "error" {
                    issues.push(ValidationIssue {
                        entry_id: id.clone(),
                        kind: ValidationKind::InvalidInjectionProvenance,
                        message: format!(
                            "entry '{id}' cannot be reconstructed into the supported TextAsset layout"
                        ),
                        source: None,
                    });
                }
            }
        }
        if applied.outcomes.iter().any(|(_, result)| result.is_err()) {
            continue;
        }
        if let Err(actual) = fit_reconstructed(applied.text, meta.capacity) {
            for entry in members {
                let Some(translation) = entry.translation.as_deref() else {
                    continue;
                };
                if translation.is_empty() || translation == entry.source {
                    continue;
                }
                issues.push(ValidationIssue {
                    entry_id: entry.id.clone(),
                    kind: ValidationKind::ExceedsBinarySlot {
                        encoding: "utf8".into(),
                        limit: meta.capacity,
                        actual,
                    },
                    message: format!(
                        "reconstructed TextAsset blob exceeds shared capacity (utf8): {actual} > {} bytes",
                        meta.capacity
                    ),
                    source: None,
                });
            }
        }
        if patches.is_empty() {
            continue;
        }
        let applied = apply_patches(&meta.original, meta.kind, &patches);
        for (id, result) in &applied.outcomes {
            if let Err(reason) = result {
                if *reason == "error" {
                    issues.push(ValidationIssue {
                        entry_id: id.clone(),
                        kind: ValidationKind::InvalidInjectionProvenance,
                        message: format!(
                            "entry '{id}' cannot be reconstructed into the supported TextAsset layout"
                        ),
                        source: None,
                    });
                }
            }
        }
        if applied.outcomes.iter().any(|(_, result)| result.is_err()) {
            continue;
        }
        if let Err(actual) = fit_reconstructed(applied.text, meta.capacity) {
            for entry in members {
                if entry
                    .translation
                    .as_deref()
                    .is_none_or(|t| t.is_empty() || t == entry.source)
                {
                    continue;
                }
                issues.push(ValidationIssue {
                    entry_id: entry.id.clone(),
                    kind: ValidationKind::ExceedsBinarySlot {
                        encoding: "utf8".into(),
                        limit: meta.capacity,
                        actual,
                    },
                    message: format!(
                        "reconstructed TextAsset blob exceeds shared capacity (utf8): {actual} > {} bytes",
                        meta.capacity
                    ),
                    source: None,
                });
            }
        }
    }
    issues
}

fn meta_usize(entry: &StringEntry, key: &str) -> Option<usize> {
    entry.metadata.get(key)?.as_u64()?.try_into().ok()
}

fn line_body(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn split_loc_kv(line: &str) -> Option<(&str, &str, &str)> {
    let line = line.trim_end_matches('\r');
    if let Some((k, v)) = line.split_once(": ") {
        let k = k.trim();
        if !k.is_empty() && !k.contains('\t') {
            return Some((k, ": ", v));
        }
    }
    if let Some((k, v)) = line.split_once('=') {
        let k = k.trim();
        if !k.is_empty() && !k.contains(' ') && !k.contains('\t') {
            return Some((k, "=", v));
        }
    }
    if let Some((k, v)) = line.split_once(':') {
        let k = k.trim();
        if !k.is_empty()
            && !k.contains(' ')
            && k.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Some((k, ":", v.trim_start()));
        }
    }
    None
}

fn find_loc_line(
    lines: &[String],
    used: &std::collections::HashSet<usize>,
    line_index: Option<usize>,
    loc_key: Option<&str>,
    physical: &str,
) -> Option<usize> {
    let mut matches = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if used.contains(&i) {
            continue;
        }
        let body = line_body(line);
        let ok = match (loc_key, split_loc_kv(body)) {
            (Some(key), Some((actual, _sep, value))) => actual == key && value == physical,
            (None, None) => body == physical,
            _ => false,
        };
        if ok {
            matches.push(i);
        }
    }
    if let Some(index) = line_index {
        if index < matches.len() {
            return Some(matches[index]);
        }
    }
    matches.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn loc_entry(id: &str, key: &str, value: &str, index: usize) -> StringEntry {
        let mut entry = StringEntry::new(id, value, PathBuf::from("sharedassets0.assets"));
        entry.metadata.insert(
            "extraction_method".into(),
            serde_json::json!("textasset_loc_line"),
        );
        entry
            .metadata
            .insert("loc_key".into(), serde_json::json!(key));
        entry
            .metadata
            .insert("line_index".into(), serde_json::json!(index));
        entry
            .metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
        entry
    }

    #[test]
    fn longer_cell_fits_when_sibling_shrinks() {
        let original = "Menu.A: Hi\nMenu.B: Hello\n";
        let mut members = vec![
            loc_entry("a", "Menu.A", "Hi", 0),
            loc_entry("b", "Menu.B", "Hello", 1),
        ];
        attach_to_entries(
            &mut members,
            GroupKind::LocLine,
            original,
            original.len(),
            "g1",
        );
        let meta = parse_group_meta(&members[0]).unwrap();
        assert_eq!(
            meta.overhead,
            original.len() - ("Hi".len() + "Hello".len()),
            "overhead must count keys/separators/newlines, not visible source sum"
        );
        let patches = [
            patch_from_entry(&members[0], "Hola").unwrap(),
            patch_from_entry(&members[1], "Hey").unwrap(),
        ];
        assert!(
            reconstructed_fits(&meta, &patches).unwrap(),
            "Hi→Hola is longer but Hello→Hey compensates"
        );
    }

    #[test]
    fn still_oversized_group_does_not_fit() {
        let original = "Menu.A: Hi\nMenu.B: Go\n";
        let mut members = vec![
            loc_entry("a", "Menu.A", "Hi", 0),
            loc_entry("b", "Menu.B", "Go", 1),
        ];
        attach_to_entries(
            &mut members,
            GroupKind::LocLine,
            original,
            original.len(),
            "g2",
        );
        let meta = parse_group_meta(&members[0]).unwrap();
        let patches = [
            patch_from_entry(&members[0], "Hola!!").unwrap(),
            patch_from_entry(&members[1], "Vamos").unwrap(),
        ];
        assert!(!reconstructed_fits(&meta, &patches).unwrap());
    }

    #[test]
    fn partial_selection_keeps_untouched_cells_in_overhead() {
        let original = "Menu.A: Hi\nMenu.B: Hello\n   ";
        let mut members = vec![
            loc_entry("a", "Menu.A", "Hi", 0),
            loc_entry("b", "Menu.B", "Hello", 1),
        ];
        attach_to_entries(
            &mut members,
            GroupKind::LocLine,
            original,
            original.len(),
            "g3",
        );
        let meta = parse_group_meta(&members[0]).unwrap();
        let patches = [patch_from_entry(&members[0], "Hola").unwrap()];
        assert!(
            reconstructed_fits(&meta, &patches).unwrap(),
            "untouched Hello + padding must absorb Hola"
        );
        let applied = apply_patches(&meta.original, meta.kind, &patches);
        assert!(applied.text.contains("Menu.B: Hello"));
        assert!(applied.text.contains("Menu.A: Hola"));
    }

    #[test]
    fn csv_rejects_comma_instead_of_inventing_quotes() {
        let original = "ID,NAME\n1,cat\n2,dog\n";
        let mut entry = StringEntry::new("c", "cat", PathBuf::from("items.assets"));
        entry.metadata.insert(
            "extraction_method".into(),
            serde_json::json!("textasset_csv_cell"),
        );
        entry
            .metadata
            .insert("csv_row".into(), serde_json::json!(1));
        entry
            .metadata
            .insert("csv_col".into(), serde_json::json!(1));
        entry
            .metadata
            .insert("csv_header".into(), serde_json::json!("NAME"));
        attach_to_entries(
            std::slice::from_mut(&mut entry),
            GroupKind::Csv,
            original,
            original.len(),
            "g-csv",
        );
        let patch = patch_from_entry(&entry, "a,b").unwrap();
        let applied = apply_patches(original, GroupKind::Csv, &[patch]);
        assert_eq!(applied.outcomes[0].1, Err("error"));
        assert_eq!(applied.text, original);
    }

    #[test]
    fn malformed_metadata_is_not_treated_as_fitting() {
        let mut entry = StringEntry::new("x", "Hi", PathBuf::from("x.assets"));
        entry.metadata.insert(
            "extraction_method".into(),
            serde_json::json!("textasset_loc_line"),
        );
        assert!(parse_group_meta(&entry).is_err());
        assert!(require_valid_metadata(&entry).is_err());
        // Parse failures are reported by validate_entry; this helper must not
        // invent a shared-blob fit/oversize verdict from incomplete metadata.
        let issues = group_validation_issues(&[entry]);
        assert!(
            !issues
                .iter()
                .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })),
            "malformed metadata must not be treated as a fitting blob: {issues:?}"
        );
    }

    #[test]
    fn pivot_physical_source_is_used_for_reconstruction() {
        let original = "Menu.A: 日本語\n";
        let mut members = vec![loc_entry("a", "Menu.A", "日本語", 0)];
        attach_to_entries(
            &mut members,
            GroupKind::LocLine,
            original,
            original.len(),
            "g-pivot",
        );
        members[0].source = "English".into();
        members[0].metadata.insert(
            crate::models::INJECTION_SOURCE_METADATA_KEY.into(),
            serde_json::json!("日本語"),
        );
        let meta = parse_group_meta(&members[0]).unwrap();
        assert_eq!(members[0].injection_source().unwrap(), "日本語");
        let patches = [patch_from_entry(&members[0], "Hola").unwrap()];
        let applied = apply_patches(&meta.original, meta.kind, &patches);
        assert!(applied.outcomes[0].1.is_ok());
        assert!(applied.text.starts_with("Menu.A: Hola"));
        assert_eq!(original_textasset(&members[0]), Some(original));
    }

    #[test]
    fn first_pass_hint_quotes_overhead_not_cell_sum() {
        let original = "Menu.A: Hi\nMenu.B: Go\n";
        let mut members = vec![
            loc_entry("a", "Menu.A", "Hi", 0),
            loc_entry("b", "Menu.B", "Go", 1),
        ];
        attach_to_entries(
            &mut members,
            GroupKind::LocLine,
            original,
            original.len(),
            "g-hint",
        );
        let meta = parse_group_meta(&members[0]).unwrap();
        let hint = first_pass_hint(&meta, 2);
        assert!(hint.contains("SHARED TEXTASSET BUDGET"), "{hint}");
        assert!(
            hint.contains(&format!("already use {} bytes", meta.overhead)),
            "{hint}"
        );
        assert!(hint.contains("not a per-cell limit"), "{hint}");
        assert_ne!(meta.overhead, "Hi".len() + "Go".len());
    }

    fn assert_lightweight_too_large(entry: &StringEntry, members: usize) {
        assert!(
            !entry.metadata.contains_key(GROUP_ORIGINAL_KEY),
            "oversized groups must not clone the original blob"
        );
        assert!(!entry.metadata.contains_key(GROUP_OVERHEAD_KEY));
        assert!(
            entry
                .metadata
                .get(GROUP_ERROR_KEY)
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains("automatic group too large")
                    && s.contains("not a per-cell budget")),
            "{:?}",
            entry.metadata.get(GROUP_ERROR_KEY)
        );
        assert!(entry
            .metadata
            .get(GROUP_ID_KEY)
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()));
        assert!(entry
            .metadata
            .get(GROUP_KIND_KEY)
            .and_then(|v| v.as_str())
            .is_some());
        assert!(
            meta_usize(entry, GROUP_CAPACITY_KEY).is_some_and(|c| c > 0),
            "lightweight metadata must still record shared capacity"
        );
        assert_eq!(meta_usize(entry, GROUP_MEMBER_COUNT_KEY), Some(members));
        let err = parse_group_meta(entry).unwrap_err();
        assert!(err.contains("automatic group too large"), "{err}");
        assert!(err.contains("not a per-cell budget"), "{err}");
        assert!(require_valid_metadata(entry).is_err());
        assert!(
            !group_validation_issues(std::slice::from_ref(entry))
                .iter()
                .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })),
            "oversize automatic groups must not invent a per-cell budget"
        );
    }

    #[test]
    fn one_small_group_still_works() {
        let original = "Menu.A: Hi\n";
        let mut members = vec![loc_entry("a", "Menu.A", "Hi", 0)];
        attach_to_entries(
            &mut members,
            GroupKind::LocLine,
            original,
            original.len(),
            "g-small",
        );
        assert_eq!(original_textasset(&members[0]), Some(original));
        assert!(!members[0].metadata.contains_key(GROUP_ERROR_KEY));
        let meta = parse_group_meta(&members[0]).unwrap();
        assert_eq!(meta.original.as_ref(), original);
        assert_eq!(meta.member_count, 1);
        assert!(require_valid_metadata(&members[0]).is_ok());
        let patches = [patch_from_entry(&members[0], "Hola").unwrap()];
        let applied = apply_patches(&meta.original, meta.kind, &patches);
        assert!(applied.outcomes[0].1.is_ok());
        assert!(applied.text.starts_with("Menu.A: Hola"));
    }

    #[test]
    fn group_member_limit_16384_refuses_original_blob() {
        let original = "Menu.A: Hi\n";
        let mut at_limit: Vec<_> = (0..MAX_GROUP_MEMBERS)
            .map(|i| loc_entry(&format!("ok{i}"), "Menu.A", "Hi", 0))
            .collect();
        attach_to_entries(
            &mut at_limit,
            GroupKind::LocLine,
            original,
            original.len(),
            "g-128",
        );
        assert_eq!(original_textasset(&at_limit[0]), Some(original));
        assert!(parse_group_meta(&at_limit[0]).is_ok());

        let mut over: Vec<_> = (0..MAX_GROUP_MEMBERS + 1)
            .map(|i| loc_entry(&format!("x{i}"), "Menu.A", "Hi", 0))
            .collect();
        over[0]
            .metadata
            .insert(GROUP_ORIGINAL_KEY.into(), serde_json::json!(original));
        over[0]
            .metadata
            .insert(GROUP_OVERHEAD_KEY.into(), serde_json::json!(1));
        attach_to_entries(
            &mut over,
            GroupKind::LocLine,
            original,
            original.len(),
            "g-129",
        );
        for entry in &over {
            assert_lightweight_too_large(entry, MAX_GROUP_MEMBERS + 1);
        }

        let mut hostile = loc_entry("h", "Menu.A", "Hi", 0);
        hostile
            .metadata
            .insert(GROUP_ID_KEY.into(), serde_json::json!("g-129-hostile"));
        hostile
            .metadata
            .insert(GROUP_KIND_KEY.into(), serde_json::json!("loc_line"));
        hostile
            .metadata
            .insert(GROUP_CAPACITY_KEY.into(), serde_json::json!(original.len()));
        hostile.metadata.insert(
            GROUP_MEMBER_COUNT_KEY.into(),
            serde_json::json!(MAX_GROUP_MEMBERS + 1),
        );
        hostile
            .metadata
            .insert(GROUP_OVERHEAD_KEY.into(), serde_json::json!(0));
        hostile
            .metadata
            .insert(GROUP_ORIGINAL_KEY.into(), serde_json::json!(original));
        let err = parse_group_meta(&hostile).unwrap_err();
        assert!(err.contains("automatic group too large"), "{err}");
    }

    #[test]
    fn group_original_1mib_limit() {
        let at_limit = "x".repeat(MAX_GROUP_ORIGINAL_BYTES);
        let mut ok = vec![loc_entry("a", "Menu.A", "Hi", 0)];
        attach_to_entries(
            &mut ok,
            GroupKind::LocLine,
            &at_limit,
            at_limit.len(),
            "g-32k",
        );
        assert_eq!(
            original_textasset(&ok[0]).map(|s| s.len()),
            Some(MAX_GROUP_ORIGINAL_BYTES)
        );
        assert!(parse_group_meta(&ok[0]).is_ok());

        let too_big = "x".repeat(MAX_GROUP_ORIGINAL_BYTES + 1);
        let mut over = vec![loc_entry("b", "Menu.A", "Hi", 0)];
        over[0].metadata.insert(
            GROUP_ORIGINAL_KEY.into(),
            serde_json::json!(too_big.clone()),
        );
        attach_to_entries(
            &mut over,
            GroupKind::LocLine,
            &too_big,
            too_big.len(),
            "g-32k-over",
        );
        assert_lightweight_too_large(&over[0], 1);

        let mut hostile = loc_entry("h", "Menu.A", "Hi", 0);
        hostile
            .metadata
            .insert(GROUP_ID_KEY.into(), serde_json::json!("g-32k-hostile"));
        hostile
            .metadata
            .insert(GROUP_KIND_KEY.into(), serde_json::json!("loc_line"));
        hostile
            .metadata
            .insert(GROUP_CAPACITY_KEY.into(), serde_json::json!(too_big.len()));
        hostile
            .metadata
            .insert(GROUP_MEMBER_COUNT_KEY.into(), serde_json::json!(1));
        hostile
            .metadata
            .insert(GROUP_OVERHEAD_KEY.into(), serde_json::json!(0));
        hostile.metadata.insert(
            GROUP_ORIGINAL_KEY.into(),
            serde_json::Value::String(too_big),
        );
        let err = parse_group_meta(&hostile).unwrap_err();
        assert!(err.contains("automatic group too large"), "{err}");
        assert!(err.contains("not a per-cell budget"), "{err}");
    }

    #[test]
    fn large_group_shares_one_original_without_product_limit() {
        let original = "x".repeat(MAX_GROUP_ORIGINAL_BYTES);
        let mut entries: Vec<_> = (0..512)
            .map(|i| loc_entry(&format!("row{i}"), "Menu.A", "Hi", 0))
            .collect();
        attach_to_entries(
            &mut entries,
            GroupKind::LocLine,
            &original,
            original.len(),
            "large",
        );
        let head = parse_group_meta(&entries[0]).unwrap();
        for entry in &entries {
            assert!(!entry.metadata.contains_key(GROUP_ORIGINAL_KEY));
            assert!(Arc::ptr_eq(
                entries[0].textasset_original.as_ref().unwrap(),
                entry.textasset_original.as_ref().unwrap()
            ));
            let meta = parse_group_meta(entry).unwrap();
            assert!(Arc::ptr_eq(&head.original, &meta.original));
            assert_eq!(head, meta);
            assert!(serde_json::to_string(entry).unwrap().len() < 2048);
        }
    }

    #[test]
    fn shared_payload_references_are_verified_without_trusting_json_flags() {
        let original = "Menu.A: 日本語\n";
        let mut entries = vec![loc_entry("a", "Menu.A", "日本語", 0)];
        attach_to_entries(
            &mut entries,
            GroupKind::LocLine,
            original,
            original.len(),
            "g",
        );
        let valid = entries[0].clone();
        let mut bad = valid.clone();
        bad.metadata.insert(
            GROUP_ORIGINAL_REF_KEY.into(),
            serde_json::json!("0".repeat(64)),
        );
        assert!(parse_group_meta(&bad).is_err());
        let mut bad = valid.clone();
        bad.metadata
            .insert(GROUP_ORIGINAL_BYTES_KEY.into(), serde_json::json!(1));
        assert!(parse_group_meta(&bad).is_err());
        let mut bad = valid.clone();
        bad.metadata
            .insert(GROUP_ORIGINAL_KEY.into(), serde_json::json!("different"));
        assert!(parse_group_meta(&bad).is_err());
        let detached: StringEntry =
            serde_json::from_value(serde_json::to_value(valid).unwrap()).unwrap();
        assert!(parse_group_meta(&detached).is_err());
    }

    #[test]
    fn structural_capacity_requires_exact_version_method_and_capability() {
        let original = "Menu.A: Hi\n";
        let mut rows = vec![loc_entry("a", "Menu.A", "Hi", 0)];
        attach_to_entries(&mut rows, GroupKind::LocLine, original, original.len(), "g");
        let mut entry = rows.remove(0);
        assert_eq!(parse_group_meta(&entry).unwrap().capacity, original.len());
        entry.metadata.insert(
            "textasset_rewrite".into(),
            serde_json::json!("serialized-v1"),
        );
        for version in [16, 23] {
            entry.metadata.insert(
                "unity_serialized_version".into(),
                serde_json::json!(version),
            );
            assert_eq!(parse_group_meta(&entry).unwrap().capacity, original.len());
        }
        for version in 17..=22 {
            entry.metadata.insert(
                "unity_serialized_version".into(),
                serde_json::json!(version),
            );
            assert_eq!(
                parse_group_meta(&entry).unwrap().capacity,
                MAX_GROUP_ORIGINAL_BYTES
            );
        }
        for method in ["mono_string", "heuristic_utf8", "textasset_unknown"] {
            entry
                .metadata
                .insert("extraction_method".into(), serde_json::json!(method));
            assert_eq!(structural_textasset_capacity(&entry), None);
        }
        entry
            .metadata
            .insert("extraction_method".into(), serde_json::json!("textasset"));
        assert_eq!(
            crate::validation::binary_slot_budget(&entry).unwrap(),
            Some(("utf8".into(), MAX_GROUP_ORIGINAL_BYTES))
        );
        entry
            .metadata
            .insert("textasset_rewrite".into(), serde_json::json!(true));
        assert_eq!(structural_textasset_capacity(&entry), None);
    }

    #[test]
    fn metadata_injection_missing_and_oversize_flags() {
        let mut missing = loc_entry("m", "Menu.A", "Hi", 0);
        missing
            .metadata
            .insert(GROUP_ID_KEY.into(), serde_json::json!("g-missing"));
        missing
            .metadata
            .insert(GROUP_KIND_KEY.into(), serde_json::json!("loc_line"));
        missing
            .metadata
            .insert(GROUP_CAPACITY_KEY.into(), serde_json::json!(12));
        missing
            .metadata
            .insert(GROUP_MEMBER_COUNT_KEY.into(), serde_json::json!(1));
        missing
            .metadata
            .insert(GROUP_OVERHEAD_KEY.into(), serde_json::json!(0));
        let missing_err = parse_group_meta(&missing).unwrap_err();
        assert!(
            missing_err.contains(GROUP_ORIGINAL_KEY) && missing_err.contains("missing"),
            "{missing_err}"
        );
        assert!(require_valid_metadata(&missing).is_err());
        assert!(
            !group_validation_issues(std::slice::from_ref(&missing))
                .iter()
                .any(|i| matches!(i.kind, ValidationKind::ExceedsBinarySlot { .. })),
            "missing original must not be treated as a fitting or per-cell budget"
        );

        let original = "Menu.A: Hi\n";
        let mut valid = vec![loc_entry("valid", "Menu.A", "Hi", 0)];
        attach_to_entries(
            &mut valid,
            GroupKind::LocLine,
            original,
            original.len(),
            "valid",
        );
        let mut zero_members = valid[0].clone();
        zero_members
            .metadata
            .insert(GROUP_MEMBER_COUNT_KEY.into(), serde_json::json!(0));
        assert!(parse_group_meta(&zero_members).is_err());
        zero_members.metadata.remove(GROUP_MEMBER_COUNT_KEY);
        assert!(parse_group_meta(&zero_members).is_err());
        valid[0]
            .metadata
            .insert(GROUP_ERROR_KEY.into(), serde_json::json!(false));
        assert!(parse_group_meta(&valid[0]).is_err());

        let too_big = "x".repeat(MAX_GROUP_ORIGINAL_BYTES + 1);
        let mut oversize = vec![loc_entry("o", "Menu.A", "Hi", 0)];
        attach_to_entries(
            &mut oversize,
            GroupKind::LocLine,
            &too_big,
            too_big.len(),
            "g-flag",
        );
        assert_lightweight_too_large(&oversize[0], 1);
        let oversize_err = parse_group_meta(&oversize[0]).unwrap_err();
        assert!(
            oversize_err.contains("automatic group too large"),
            "{oversize_err}"
        );
        assert!(
            !oversize_err.contains("per-cell limit of"),
            "{oversize_err}"
        );
    }
}
