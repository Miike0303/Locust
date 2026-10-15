//! Unregistered experimental adapter for explicitly selected standalone PCK v2.
//!
//! Positive profile: stock Dialogic syntax understood by `dialogic`, with unique
//! `.dch` file stems in this archive as character identifiers. Display names are
//! never identifiers. Custom/dynamic indexers, identifier overrides and the
//! installed addon version are not proven. Replace produces an archive-only copy.
//! Direct revisions require core-verified prior output and pristine provenance.
//! Each touched member is rebuilt from authenticated original bytes after an
//! exact codec reconstruction proves the current member's compatible structure.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::path::{Component, Path, PathBuf};

use locust_core::backup::RevisionOriginal;
use locust_core::error::{LocustError, Result};
use locust_core::extraction::{Extraction, FormatPlugin, FormatStability, InjectionReport};
use locust_core::models::StringEntry;
use locust_core::patch::{zipsec, GameLock};
use serde::{Deserialize, Serialize};

use crate::dialogic::{self, CharacterSet, Span, SpanId, SpanKind};
use crate::godot_pck::{self, PckIndex};

#[derive(Default)]
pub struct GodotDialogicPlugin;

impl GodotDialogicPlugin {
    pub fn new() -> Self {
        Self
    }
}

const LOCATOR_KEY: &str = "godot_dialogic";

fn refused(message: impl Into<String>) -> LocustError {
    LocustError::UnsupportedFormat(format!("Godot/Dialogic: {}", message.into()))
}

fn warn(warnings: &mut Vec<String>, member: &str, reason: impl std::fmt::Display) {
    if warnings.len() < 128 {
        // Bound each part separately so a long virtual path cannot hide why
        // the member was refused. Keep both the path prefix and its filename.
        let member = if member.chars().count() > 768 {
            let head = member.chars().take(384).collect::<String>();
            let tail = member.chars().rev().take(384).collect::<Vec<_>>();
            format!("{head}…{}", tail.into_iter().rev().collect::<String>())
        } else {
            member.to_owned()
        };
        let reason = reason.to_string().chars().take(1024).collect::<String>();
        warnings.push(format!("{member}: {reason}"));
    }
}

fn no_links(path: &Path) -> Result<()> {
    for ancestor in std::path::absolute(path)?.ancestors() {
        zipsec::ensure_no_links(ancestor, Path::new(""))?;
    }
    Ok(())
}

fn archive_name(path: &Path) -> Result<String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| refused("archive name must be a UTF-8 filename"))?;
    if !Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pck"))
    {
        return Err(refused(
            "select a standalone .pck file; executables/embedded packs are unsupported",
        ));
    }
    zipsec::safe_entry_path(name, name)?;
    Ok(name.to_owned())
}

fn source_index(path: &Path) -> Result<(File, PckIndex)> {
    no_links(path)?;
    let metadata = std::fs::symlink_metadata(path).map_err(|e| {
        refused(format!(
            "select an existing regular standalone .pck file: {e}"
        ))
    })?;
    if !metadata.is_file() {
        return Err(refused(
            "select a regular standalone .pck FILE, not a directory or other file type",
        ));
    }
    archive_name(path)?;
    let mut file = File::open(path)?;
    let index = godot_pck::read_index(&mut file)
        .map_err(|e| refused(format!("invalid or unsupported standalone PCK: {e}")))?;
    if !index.entries.iter().any(|e| e.path.ends_with(".dtl")) {
        return Err(refused("archive contains no Dialogic .dtl timelines"));
    }
    Ok((file, index))
}

fn kind_name(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Text => "text",
        SpanKind::TextContinued => "text-continued",
        SpanKind::Choice => "choice",
    }
}

fn md5_hex(md5: &[u8; 16]) -> String {
    md5.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Locator {
    locator_version: u32,
    archive: String,
    member: String,
    original_md5: String,
    kind: String,
    line: usize,
    ordinal: usize,
    translation_id: Option<String>,
    speaker: Option<String>,
}

impl Locator {
    fn id(&self) -> String {
        // JSON tuple framing makes separator-bearing filenames and #ids
        // unambiguous. MD5 and display context are provenance, not identity.
        format!(
            "godot-dialogic:v1:{}",
            serde_json::json!([
                self.archive,
                self.member,
                self.line,
                self.kind,
                self.ordinal,
                self.translation_id
            ])
        )
    }

    fn span_id(&self) -> Result<SpanId> {
        let kind = match self.kind.as_str() {
            "text" => SpanKind::Text,
            "text-continued" => SpanKind::TextContinued,
            "choice" => SpanKind::Choice,
            _ => return Err(refused("unknown span kind in locator")),
        };
        Ok(SpanId {
            line: self.line,
            kind,
            ordinal: self.ordinal,
            translation_id: self.translation_id.clone(),
        })
    }

    fn row(archive: &str, member: &str, md5: &[u8; 16], span: &Span) -> Result<StringEntry> {
        let locator = Self {
            locator_version: 1,
            archive: archive.into(),
            member: member.into(),
            original_md5: md5_hex(md5),
            kind: kind_name(span.kind).into(),
            line: span.id.line,
            ordinal: span.id.ordinal,
            translation_id: span.id.translation_id.clone(),
            speaker: span.speaker.clone(),
        };
        let mut row = StringEntry::new(
            locator.id(),
            span.literal_text(),
            Path::new(archive).join(member.strip_prefix("res://").expect("validated PCK path")),
        )
        .with_format_id("godot-dialogic");
        row.context = Some(format!(
            "{} — {member}",
            span.speaker.as_deref().unwrap_or("Narration/choice")
        ));
        row.metadata
            .insert(LOCATOR_KEY.into(), serde_json::to_value(locator)?);
        Ok(row)
    }
}

/// Read and authenticate every character resource, even if another fails.
/// Duplicate stems quarantine all timelines rather than inventing authority.
fn characters(
    file: &mut File,
    index: &PckIndex,
    warnings: &mut Vec<String>,
) -> (CharacterSet, bool) {
    let mut characters = CharacterSet::default();
    let mut stems = HashSet::new();
    let mut duplicate = false;
    for entry in index.entries.iter().filter(|e| e.path.ends_with(".dch")) {
        let stem = entry
            .path
            .rsplit('/')
            .next()
            .unwrap()
            .strip_suffix(".dch")
            .unwrap();
        if !stems.insert(stem) {
            duplicate = true;
            warn(
                warnings,
                &entry.path,
                format!("duplicate .dch stem '{stem}'; character authority is ambiguous"),
            );
        }
        let parsed = godot_pck::read_member(file, entry, dialogic::MAX_INPUT_BYTES as u64)
            .and_then(|bytes| {
                let text = std::str::from_utf8(&bytes).map_err(std::io::Error::other)?;
                dialogic::parse_character(text)
            });
        match parsed {
            Ok(_) if !duplicate => {
                if let Err(e) = characters.insert_filename(&entry.path) {
                    warn(warnings, &entry.path, e);
                }
            }
            Ok(_) => {}
            Err(e) => warn(warnings, &entry.path, e),
        }
    }
    (characters, duplicate)
}

struct Target<'a> {
    path: PathBuf,
    rows: Vec<(&'a StringEntry, Locator)>,
}

/// Accept only exact relative virtual paths or their exact core-remapped
/// absolute equivalents under this destination. Never search for an archive.
fn target<'a>(path: &Path, entries: &'a [StringEntry]) -> Result<Option<Target<'a>>> {
    if entries.is_empty() {
        return Ok(None);
    }
    no_links(path)?;
    let absolute = std::path::absolute(path)?;
    let canonical = path.canonicalize()?;
    let root = if path.is_file() {
        absolute.parent().unwrap()
    } else if path.is_dir() {
        absolute.as_path()
    } else {
        return Err(refused(
            "injection destination must be an existing archive or directory",
        ));
    };
    let canonical_root = if path.is_file() {
        canonical.parent().unwrap()
    } else {
        canonical.as_path()
    };
    let mut rows = Vec::new();
    let mut ids = HashSet::new();
    let mut archive = None;
    let mut member_md5s = HashMap::new();
    for row in entries {
        if row
            .metadata
            .get("format_id")
            .and_then(|value| value.as_str())
            != Some("godot-dialogic")
        {
            return Err(refused(
                "row format metadata disagrees with Godot/Dialogic locator",
            ));
        }
        let locator: Locator = serde_json::from_value(
            row.metadata
                .get(LOCATOR_KEY)
                .cloned()
                .ok_or_else(|| refused("missing versioned row locator"))?,
        )
        .map_err(|e| refused(format!("invalid row locator metadata: {e}")))?;
        if locator.locator_version != 1 || row.id != locator.id() || !ids.insert(row.id.as_str()) {
            return Err(refused(
                "unsupported locator version, id disagreement or duplicate locator",
            ));
        }
        locator.span_id()?;
        if archive_name(Path::new(&locator.archive))? != locator.archive
            || Path::new(&locator.archive).components().count() != 1
        {
            return Err(refused("locator archive must be one exact filename"));
        }
        let tail = locator
            .member
            .strip_prefix("res://")
            .ok_or_else(|| refused("locator member must be canonical res:// path"))?;
        if !tail.ends_with(".dtl")
            || tail.contains('\\')
            || tail
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == "..")
        {
            return Err(refused("locator member must be a canonical .dtl path"));
        }
        zipsec::safe_entry_path(tail, tail)?;
        let relative = Path::new(&locator.archive).join(tail);
        let actual = if row.file_path.is_absolute() {
            row.file_path
                .strip_prefix(root)
                .or_else(|_| row.file_path.strip_prefix(canonical_root))
                .map_err(|_| refused("row path is outside injection destination"))?
        } else {
            &row.file_path
        };
        if actual
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || actual != relative
        {
            return Err(refused(
                "row path disagrees with exact archive/member locator",
            ));
        }
        if locator.original_md5.len() != 32
            || !locator
                .original_md5
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(refused("invalid original member MD5 in locator"));
        }
        if let Some(previous) =
            member_md5s.insert(locator.member.clone(), locator.original_md5.clone())
        {
            if previous != locator.original_md5 {
                return Err(refused("conflicting original MD5s for one member"));
            }
        }
        if archive.as_ref().is_some_and(|a| a != &locator.archive) {
            return Err(refused("rows must name exactly one archive"));
        }
        archive = Some(locator.archive.clone());
        rows.push((row, locator));
    }
    let archive_path = root.join(archive.unwrap());
    if path.is_file() && archive_path != absolute {
        return Err(refused("wrong selected archive name"));
    }
    let (_, index) = source_index(&archive_path)?;
    for (_, locator) in &rows {
        if !index.entries.iter().any(|e| e.path == locator.member) {
            return Err(refused(format!("unknown exact member: {}", locator.member)));
        }
    }
    Ok(Some(Target {
        path: archive_path,
        rows,
    }))
}

fn empty_report() -> InjectionReport {
    InjectionReport {
        files_modified: 0,
        strings_written: 0,
        strings_skipped: 0,
        warnings: Vec::new(),
        files_written: Vec::new(),
        skip_reasons: BTreeMap::new(),
    }
}

impl FormatPlugin for GodotDialogicPlugin {
    fn id(&self) -> &str {
        "godot-dialogic"
    }
    fn name(&self) -> &str {
        "Godot / Dialogic (standalone PCK)"
    }
    fn description(&self) -> &str {
        "Experimental stock Dialogic timelines in a selected standalone Godot 4.0–4.3 PCK v2; archive-only Replace."
    }
    fn supported_extensions(&self) -> &[&str] {
        &["pck"]
    }
    fn stability(&self) -> FormatStability {
        FormatStability::Experimental
    }
    fn detect(&self, path: &Path) -> bool {
        source_index(path).is_ok()
    }
    fn validate_source(&self, source: &Path) -> Result<()> {
        source_index(source).map(|_| ())
    }
    fn extract(&self, path: &Path) -> Result<Vec<StringEntry>> {
        Ok(self.extract_with_diagnostics(path)?.entries)
    }

    fn extract_with_diagnostics(&self, path: &Path) -> Result<Extraction> {
        let (mut file, index) = source_index(path)?;
        let archive = archive_name(path)?;
        let mut warnings = Vec::new();
        let mut entries = Vec::new();
        let (characters, duplicate) = characters(&mut file, &index, &mut warnings);
        for entry in index.entries.iter().filter(|e| e.path.ends_with(".dtl")) {
            let timeline =
                godot_pck::read_member(&mut file, entry, dialogic::MAX_INPUT_BYTES as u64)
                    .and_then(|bytes| {
                        let text = std::str::from_utf8(&bytes).map_err(std::io::Error::other)?;
                        dialogic::parse_timeline(text, &characters)
                    });
            match timeline {
                Ok(timeline) => {
                    for diagnostic in &timeline.diagnostics {
                        warn(
                            &mut warnings,
                            &entry.path,
                            format!("line {}: {}", diagnostic.line + 1, diagnostic.reason),
                        );
                    }
                    if !duplicate {
                        for span in &timeline.spans {
                            entries.push(Locator::row(
                                &archive,
                                &entry.path,
                                &entry.stored_md5,
                                span,
                            )?);
                        }
                    }
                }
                Err(e) => warn(&mut warnings, &entry.path, e),
            }
        }
        Ok(Extraction {
            complete: warnings.is_empty(),
            entries,
            warnings,
        })
    }

    fn inject(&self, path: &Path, entries: &[StringEntry]) -> Result<InjectionReport> {
        // Structural refusal and a true no-translation no-op need no lock/write.
        let selected = target(path, entries)?;
        if !entries.iter().any(|e| e.translation.is_some()) {
            return Ok(empty_report());
        }
        let selected = selected.expect("translated entries imply target");
        let lock = GameLock::acquire(selected.path.parent().unwrap())?;
        self.inject_under_lock(path, entries, &lock)
    }

    fn revision_original_path(&self, file_path: &Path) -> PathBuf {
        file_path
            .ancestors()
            .find(|p| {
                p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pck"))
                    && (p.is_file() || (!p.is_absolute() && p.components().count() == 1))
            })
            .unwrap_or(file_path)
            .to_path_buf()
    }

    fn inject_revision(
        &self,
        path: &Path,
        entries: &mut [StringEntry],
        originals: &HashMap<PathBuf, RevisionOriginal>,
    ) -> Result<InjectionReport> {
        let Some(target) = target(path, entries)? else {
            return Ok(empty_report());
        };
        let canonical = target.path.canonicalize()?;
        let original = originals
            .get(&target.path)
            .or_else(|| originals.get(&canonical))
            .ok_or_else(|| {
                refused("revision requires a verified original for this exact physical PCK")
            })?;
        let lock = GameLock::acquire(target.path.parent().unwrap())?;
        let (report, replacements) = prepare_injection_from(&target, Some(original))?;
        install(&lock, target.path, report, replacements)
    }

    fn inject_under_lock(
        &self,
        path: &Path,
        entries: &[StringEntry],
        lock: &GameLock,
    ) -> Result<InjectionReport> {
        lock.validate_selection(path)?;
        let report = empty_report();
        let Some(target) = target(path, entries)? else {
            return Ok(report);
        };
        lock.validate_selection(&target.path)?;
        if !entries.iter().any(|e| e.translation.is_some()) {
            return Ok(report);
        }
        let (report, replacements) = prepare_injection(&target)?;
        install(lock, target.path, report, replacements)
    }
}

fn install(
    lock: &GameLock,
    path: PathBuf,
    mut report: InjectionReport,
    replacements: Replacements,
) -> Result<InjectionReport> {
    if !replacements.is_empty() {
        let refs = replacements
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect::<Vec<_>>();
        godot_pck::replace_archive(lock, &path, &refs, &[])?;
        report.files_modified = 1;
        report.files_written.push(path);
    }
    Ok(report)
}

// All member provenance and codec checks run before transactional staging.
// Kept separate so malformed-input refusal is testable without an OS lock.
type Replacements = Vec<(String, Vec<u8>)>;
fn prepare_injection(target: &Target<'_>) -> Result<(InjectionReport, Replacements)> {
    prepare_injection_from(target, None)
}

fn prepare_injection_from(
    target: &Target<'_>,
    revision: Option<&RevisionOriginal>,
) -> Result<(InjectionReport, Replacements)> {
    let mut report = empty_report();
    let (mut file, index) = source_index(&target.path)?;
    // Verification scans the physical original with bounded memory; only the
    // index and requested timeline payloads are subsequently retained/read.
    let mut original_file = revision.map(RevisionOriginal::open_verified).transpose()?;
    let original_index = original_file
        .as_mut()
        .map(godot_pck::read_index)
        .transpose()?;
    if let Some(original) = &original_index {
        if index.format_version != original.format_version
            || index.engine_version != original.engine_version
            || index.flags != original.flags
            || index.reserved != original.reserved
            || index.entries.len() != original.entries.len()
            || index.entries.iter().zip(&original.entries).any(|(a, b)| {
                a.path != b.path
                    || a.padded_path != b.padded_path
                    || a.flags != b.flags
                    || (a.path.ends_with(".dch")
                        && (a.stored_md5 != b.stored_md5 || a.size != b.size))
            })
        {
            return Err(refused("revision archive structure/order or character authority differs from verified original"));
        }
    }
    let (characters, duplicate) = characters(&mut file, &index, &mut report.warnings);
    if duplicate || !report.warnings.is_empty() {
        return Err(refused("character authority cannot be authenticated; re-extract after resolving character diagnostics"));
    }
    let mut groups: BTreeMap<&str, Vec<(&StringEntry, &Locator)>> = BTreeMap::new();
    for (row, locator) in &target.rows {
        groups
            .entry(&locator.member)
            .or_default()
            .push((row, locator));
    }
    let mut replacements = Vec::new();
    for (member, rows) in groups {
        let count = rows.iter().filter(|(r, _)| r.translation.is_some()).count();
        if count == 0 {
            continue;
        }
        let base_index = original_index.as_ref().unwrap_or(&index);
        let entry = base_index
            .entries
            .iter()
            .find(|e| e.path == member)
            .ok_or_else(|| refused(format!("unknown exact member: {member}")))?;
        if rows
            .iter()
            .any(|(_, l)| l.original_md5 != md5_hex(&entry.stored_md5))
        {
            if revision.is_some() {
                return Err(refused(format!(
                    "original member MD5 differs from row provenance: {member}"
                )));
            }
            report.skip("stale-member", count);
            warn(&mut report.warnings, member, "original member MD5 differs; stale/wrong archive or revision requires a core-verified original");
            continue;
        }
        let original = match godot_pck::read_member(
            original_file.as_mut().unwrap_or(&mut file),
            entry,
            dialogic::MAX_INPUT_BYTES as u64,
        )
        .and_then(|bytes| String::from_utf8(bytes).map_err(std::io::Error::other))
        {
            Ok(text) => text,
            Err(e) => {
                if revision.is_some() {
                    return Err(refused(format!(
                        "verified original member unreadable: {member}: {e}"
                    )));
                }
                report.skip("unreadable-member", count);
                warn(&mut report.warnings, member, e);
                continue;
            }
        };
        let timeline = dialogic::parse_timeline(&original, &characters)?;
        let mut translations = HashMap::new();
        let current = if revision.is_some() {
            let current_entry = index.entries.iter().find(|e| e.path == member).unwrap();
            let bytes =
                godot_pck::read_member(&mut file, current_entry, dialogic::MAX_INPUT_BYTES as u64)?;
            String::from_utf8(bytes)
                .map_err(|e| refused(format!("current member is not UTF-8: {e}")))?
        } else {
            original.clone()
        };
        let current_timeline = dialogic::parse_timeline(&current, &characters)?;
        if revision.is_some() {
            if timeline.diagnostics != current_timeline.diagnostics
                || timeline.spans.len() != current_timeline.spans.len()
            {
                return Err(refused(format!(
                    "revision codec structure differs: {member}"
                )));
            }
            for (a, b) in timeline.spans.iter().zip(&current_timeline.spans) {
                if a.id != b.id || a.speaker != b.speaker {
                    return Err(refused(format!(
                        "revision codec identity differs: {member}"
                    )));
                }
                translations.insert(
                    a.id.clone(),
                    a.encode_literal_translation(&b.literal_text())?,
                );
            }
            // Stronger than matching visible text/IDs/counts: every byte outside
            // translated pieces, segment edges, and all controls must survive.
            if dialogic::rebuild(&original, &timeline, &translations)? != current {
                return Err(refused(format!(
                    "revision codec structure differs outside translated spans: {member}"
                )));
            }
        }
        let mut changed = 0;
        for (row, locator) in rows {
            let span = timeline
                .spans
                .iter()
                .find(|s| locator.span_id().is_ok_and(|id| id == s.id))
                .ok_or_else(|| refused(format!("unknown positional span in {member}")))?;
            if span.speaker != locator.speaker || span.literal_text() != row.source {
                return Err(refused(format!(
                    "source/speaker disagrees with span in {member}"
                )));
            }
            let Some(translation) = &row.translation else {
                continue;
            };
            let current_span = current_timeline
                .spans
                .iter()
                .find(|s| s.id == span.id)
                .unwrap();
            if translation == &current_span.literal_text() {
                continue;
            }
            // Validate each row independently so one bad row cannot discard
            // valid neighbours; final rebuild validates their combination.
            match span
                .encode_literal_translation(translation)
                .and_then(|encoded| {
                    dialogic::rebuild(
                        &original,
                        &timeline,
                        &HashMap::from([(span.id.clone(), encoded.clone())]),
                    )?;
                    Ok(encoded)
                }) {
                Ok(encoded) => {
                    translations.insert(span.id.clone(), encoded);
                    changed += 1;
                }
                Err(e) => {
                    report.skip("invalid-translation", 1);
                    warn(
                        &mut report.warnings,
                        member,
                        format!("line {}: {e}", span.id.line + 1),
                    );
                }
            }
        }
        if translations.is_empty() {
            continue;
        }
        let rebuilt = dialogic::rebuild(&original, &timeline, &translations)?;
        if rebuilt != current {
            report.strings_written += changed;
            replacements.push((member.to_owned(), rebuilt.into_bytes()));
        }
    }
    drop(file);
    Ok((report, replacements))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fixture(path: &Path) -> Vec<StringEntry> {
        let mut empty = vec![0; 100];
        empty[..4].copy_from_slice(b"GDPC");
        empty[4..8].copy_from_slice(&2u32.to_le_bytes());
        empty[8..12].copy_from_slice(&4u32.to_le_bytes());
        let mut source = Cursor::new(empty);
        let index = godot_pck::read_index(&mut source).unwrap();
        let mut bytes = Vec::new();
        godot_pck::rebuild(
            &mut source,
            &index,
            &[],
            &[
                ("res://story.dtl", b"hero: Hello [b]{player}[/b]!\n- Yes\n"),
                ("res://hero.dch", b"{}"),
                ("res://raw.bin", b"\x00\xffunchanged"),
                ("res://other.dtl", b"Other words\n"),
            ],
            &mut bytes,
        )
        .unwrap();
        std::fs::write(path, bytes).unwrap();
        GodotDialogicPlugin.extract(path).unwrap()
    }

    fn plan(path: &Path, rows: &[StringEntry]) -> Result<(InjectionReport, Replacements)> {
        prepare_injection(&target(path, rows)?.unwrap())
    }

    fn rewrite(path: &Path, replacements: &Replacements, additions: &[(&str, &[u8])]) {
        let mut source = File::open(path).unwrap();
        let index = godot_pck::read_index(&mut source).unwrap();
        let refs = replacements
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect::<Vec<_>>();
        let mut output = Vec::new();
        godot_pck::rebuild(&mut source, &index, &refs, additions, &mut output).unwrap();
        drop(source);
        std::fs::write(path, output).unwrap();
    }

    #[test]
    fn revision_plan_abc_partial_noop_and_original_restoration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("game.pck");
        let mut rows = fixture(&path);
        assert_eq!(
            GodotDialogicPlugin.revision_original_path(&rows[0].file_path),
            Path::new("game.pck")
        );
        assert_eq!(
            GodotDialogicPlugin.revision_original_path(&path.join("folder.pck/story.dtl")),
            path
        );
        let pristine = temp.path().join("original");
        std::fs::copy(&path, &pristine).unwrap();
        let original = RevisionOriginal::capture(&pristine).unwrap();
        rows[0].translation = Some("Primera [b]{player}[/b]!".into());
        rows[1].translation = Some("Sí".into());
        rows[2].translation = Some("Otro texto traducido".into());
        rewrite(&path, &plan(&path, &rows).unwrap().1, &[]);
        for value in ["Segunda muy larga [b]{player}[/b]!", "C [b]{player}[/b]!"] {
            rows[0].translation = Some(value.into());
            let selected = target(&path, &rows[..1]).unwrap().unwrap();
            let (report, replacements) =
                prepare_injection_from(&selected, Some(&original)).unwrap();
            assert_eq!(report.strings_written, 1);
            assert_eq!(replacements.len(), 1);
            let text = String::from_utf8_lossy(&replacements[0].1);
            assert!(text.contains(value));
            assert!(text.ends_with("\n- Sí\n"), "prior span lost: {text}");
            rewrite(&path, &replacements, &[]);
        }
        let selected = target(&path, &rows[..1]).unwrap().unwrap();
        let (report, replacements) = prepare_injection_from(&selected, Some(&original)).unwrap();
        assert_eq!(report.strings_written, 0);
        assert!(replacements.is_empty());
        // An explicit translation back to source must undo the prior span.
        rows[0].translation = Some(rows[0].source.clone());
        let selected = target(&path, &rows[..1]).unwrap().unwrap();
        let (report, replacements) = prepare_injection_from(&selected, Some(&original)).unwrap();
        assert_eq!(report.strings_written, 1);
        assert!(String::from_utf8_lossy(&replacements[0].1).contains("Hello [b]{player}[/b]!"));
        let mut current = File::open(&path).unwrap();
        let index = godot_pck::read_index(&mut current).unwrap();
        let other = index
            .entries
            .iter()
            .find(|e| e.path == "res://other.dtl")
            .unwrap();
        assert_eq!(
            godot_pck::read_member(&mut current, other, 4096).unwrap(),
            b"Otro texto traducido\n"
        );
    }

    #[test]
    fn revision_plan_refuses_original_md5_and_codec_or_archive_structure_without_writes() {
        for variant in [
            "md5",
            "comment",
            "control",
            "event",
            "authority",
            "member-set",
            "order",
            "header",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("game.pck");
            let mut rows = fixture(&path);
            let pristine = temp.path().join("original");
            std::fs::copy(&path, &pristine).unwrap();
            let original = RevisionOriginal::capture(&pristine).unwrap();
            rows[0].translation = Some("Nuevo [b]{player}[/b]!".into());
            match variant {
                "md5" => {
                    rows[0].metadata.get_mut(LOCATOR_KEY).unwrap()["original_md5"] =
                        serde_json::json!("00000000000000000000000000000000")
                }
                "comment" => rewrite(
                    &path,
                    &vec![(
                        "res://story.dtl".into(),
                        b"hero: Hello [b]{player}[/b]!\n- Yes\n# added comment\n".to_vec(),
                    )],
                    &[],
                ),
                "control" => rewrite(
                    &path,
                    &vec![(
                        "res://story.dtl".into(),
                        b"hero: Hello [i]{player}[/i]!\n- Yes\n".to_vec(),
                    )],
                    &[],
                ),
                "event" => rewrite(
                    &path,
                    &vec![(
                        "res://story.dtl".into(),
                        b"hero: Hello [b]{player}[/b]!\n- Yes\nNew event\n".to_vec(),
                    )],
                    &[],
                ),
                "authority" => rewrite(
                    &path,
                    &vec![(
                        "res://hero.dch".into(),
                        br#"{"display_name":"Changed"}"#.to_vec(),
                    )],
                    &[],
                ),
                "member-set" => rewrite(&path, &vec![], &[("res://added.bin", b"new")]),
                "order" => {
                    let mut file = File::open(&path).unwrap();
                    let mut index = godot_pck::read_index(&mut file).unwrap();
                    index.entries.swap(0, 1);
                    let mut output = Vec::new();
                    // A genuine reordered archive, not a forged index argument.
                    let additions = index
                        .entries
                        .iter()
                        .map(|e| {
                            (
                                e.path.clone(),
                                godot_pck::read_member(&mut file, e, 4096).unwrap(),
                            )
                        })
                        .collect::<Vec<_>>();
                    drop(file);
                    let mut empty = vec![0; 100];
                    empty[..4].copy_from_slice(b"GDPC");
                    empty[4..8].copy_from_slice(&2u32.to_le_bytes());
                    empty[8..12].copy_from_slice(&4u32.to_le_bytes());
                    let mut source = Cursor::new(empty);
                    let index = godot_pck::read_index(&mut source).unwrap();
                    let refs = additions
                        .iter()
                        .map(|(k, v)| (k.as_str(), v.as_slice()))
                        .collect::<Vec<_>>();
                    godot_pck::rebuild(&mut source, &index, &[], &refs, &mut output).unwrap();
                    std::fs::write(&path, output).unwrap();
                }
                "header" => {
                    let mut bytes = std::fs::read(&path).unwrap();
                    bytes[32] = 1;
                    std::fs::write(&path, bytes).unwrap();
                }
                _ => unreachable!(),
            }
            let before = std::fs::read(&path).unwrap();
            let selected = target(&path, &rows[..1]).unwrap().unwrap();
            assert!(
                prepare_injection_from(&selected, Some(&original)).is_err(),
                "{variant}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn revision_api_refuses_missing_original_before_lock_or_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("game.pck");
        let mut rows = fixture(&path);
        rows[0].translation = Some("Nuevo [b]{player}[/b]!".into());
        let before = std::fs::read(&path).unwrap();
        assert!(GodotDialogicPlugin
            .inject_revision(&path, &mut rows, &HashMap::new())
            .unwrap_err()
            .to_string()
            .contains("verified original"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn adapter_plan_variable_length_preserves_every_untouched_member_and_second_pass() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("game.pck");
        let mut rows = fixture(&path);
        rows[0].translation = Some("Un saludo bastante más largo [b]{player}[/b]!".into());
        let (report, replacements) = plan(&path, &rows).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (1, 0));
        assert_eq!(replacements.len(), 1);
        let mut source = File::open(&path).unwrap();
        let index = godot_pck::read_index(&mut source).unwrap();
        let refs = replacements
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect::<Vec<_>>();
        let mut output = Cursor::new(Vec::new());
        let rebuilt = godot_pck::rebuild(&mut source, &index, &refs, &[], &mut output).unwrap();
        godot_pck::verify_rebuild(&mut output, &rebuilt).unwrap();
        let next_index = godot_pck::read_index(&mut output).unwrap();
        for entry in &index.entries {
            let next = next_index
                .entries
                .iter()
                .find(|e| e.path == entry.path)
                .unwrap();
            let before = godot_pck::read_member(&mut source, entry, 4096).unwrap();
            let after = godot_pck::read_member(&mut output, next, 4096).unwrap();
            if entry.path == "res://story.dtl" {
                assert!(after.starts_with("hero: Un saludo".as_bytes()));
                assert!(after.ends_with(b"\n- Yes\n"));
                assert_ne!(before.len(), after.len());
            } else {
                assert_eq!(after, before, "{}", entry.path);
            }
        }
        drop(source);
        std::fs::write(&path, output.into_inner()).unwrap();
        let (report, replacements) = plan(&path, &rows).unwrap();
        assert_eq!((report.strings_written, report.strings_skipped), (0, 1));
        assert!(replacements.is_empty());
        assert!(report.warnings[0].contains("MD5"));
    }

    #[test]
    fn adapter_plan_skips_bad_rows_keeps_good_neighbours_and_counts_noops() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("game.pck");
        let baseline = fixture(&path);
        for bad in [
            "Hola [i]{player}[/b]!",
            "Hola [b]{other}[/b]!",
            "Hola {player}[b][/b]!",
            "Hola [b]{player}[/b]! [b]",
        ] {
            let mut rows = baseline.clone();
            rows[0].translation = Some(bad.into());
            rows[1].translation = Some("Sí, adelante".into());
            let (report, replacements) = plan(&path, &rows).unwrap();
            assert_eq!(
                (report.strings_written, report.strings_skipped),
                (1, 1),
                "{bad}"
            );
            assert_eq!(report.skip_reasons["invalid-translation"], 1);
            assert!(report.warnings[0].contains("protected"));
            let text = String::from_utf8_lossy(&replacements[0].1);
            assert!(text.contains("Hello [b]{player}[/b]!"));
            assert!(text.contains("Sí, adelante"));
        }
        let mut rows = baseline;
        rows[0].translation = Some(rows[0].source.clone());
        let (report, replacements) = plan(&path, &rows).unwrap();
        assert_eq!(report.strings_written, 0);
        assert!(replacements.is_empty());
    }

    #[test]
    fn adapter_plan_md5_payload_source_speaker_and_unknown_member_refusals() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("game.pck");
        let mut rows = fixture(&path);
        rows[0].translation = Some("Hola [b]{player}[/b]!".into());
        let mut row = rows[0].clone();
        row.source = "forged source".into();
        assert!(plan(&path, &[row])
            .unwrap_err()
            .to_string()
            .contains("source/speaker"));
        let mut row = rows[0].clone();
        row.metadata.get_mut(LOCATOR_KEY).unwrap()["speaker"] = serde_json::json!("impostor");
        assert!(plan(&path, &[row])
            .unwrap_err()
            .to_string()
            .contains("source/speaker"));
        let mut row = rows[0].clone();
        let mut locator: Locator =
            serde_json::from_value(row.metadata[LOCATOR_KEY].clone()).unwrap();
        locator.member = "res://missing.dtl".into();
        row.id = locator.id();
        row.file_path = "game.pck/missing.dtl".into();
        row.metadata
            .insert(LOCATOR_KEY.into(), serde_json::to_value(locator).unwrap());
        assert!(plan(&path, &[row])
            .unwrap_err()
            .to_string()
            .contains("unknown exact member"));
        let mut row = rows[0].clone();
        row.metadata.get_mut(LOCATOR_KEY).unwrap()["original_md5"] =
            serde_json::json!("00000000000000000000000000000000");
        let (report, replacements) = plan(&path, &[row]).unwrap();
        assert_eq!(report.strings_skipped, 1);
        assert!(report.warnings[0].contains("MD5"));
        assert!(replacements.is_empty());
        let mut bytes = std::fs::read(&path).unwrap();
        let index = godot_pck::read_index(&mut Cursor::new(&bytes)).unwrap();
        bytes[index.entries[0].offset as usize] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        let (report, replacements) = plan(&path, &rows[..1]).unwrap();
        assert_eq!(report.strings_skipped, 1);
        assert!(report.warnings[0].contains("MD5"));
        assert!(replacements.is_empty());
    }

    #[test]
    fn adapter_plan_accepts_only_exact_core_remapped_absolute_paths() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("game.pck");
        let mut rows = fixture(&path);
        rows[0].translation = Some("Hola [b]{player}[/b]!".into());
        for row in &mut rows {
            row.file_path = temp.path().canonicalize().unwrap().join(&row.file_path);
        }
        assert_eq!(plan(&path, &rows).unwrap().0.strings_written, 1);
        assert_eq!(plan(temp.path(), &rows).unwrap().0.strings_written, 1);
        let foreign = tempfile::tempdir().unwrap();
        rows[0].file_path = foreign.path().join("game.pck/story.dtl");
        assert!(plan(&path, &rows).is_err());
    }
}
