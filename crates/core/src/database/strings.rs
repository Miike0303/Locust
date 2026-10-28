use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::{params, types::Value, Connection};

use crate::error::{LocustError, Result};
use crate::models::{
    StringEntry, StringStatus, TranslationResult, INJECTION_CAPACITY_METADATA_KEY,
    INJECTION_SOURCE_METADATA_KEY, STALE_TRANSLATION_METADATA_KEY,
};

use super::{
    lock_connection, sha256_hex, Database, EntryFilter, FileStats, ImportApplyReport, ImportChange,
    ImportPreview, MergeStats, PivotResult, ProjectStats, StringFacets, TranslationBatchItem,
    TranslationBatchReport, TranslationSaveGuard, TranslationSaveOutcome,
};
#[cfg(test)]
use super::{
    ENTRY_ROWS_MATERIALIZED, GUARDED_TRANSLATION_TRANSACTIONS, IMPORT_READ_QUERIES, STRINGS_QUERIES,
};

impl Database {
    pub fn save_entries(&self, entries: &[StringEntry]) -> Result<usize> {
        let conn = lock_connection(&self.conn);
        // Single transaction: per-row implicit transactions fsync each insert,
        // which takes minutes for a full game extraction.
        // prepare_cached: parse/plan the INSERT once per connection, not per row
        // (33k Ochiru / 22k Injuu extracts).
        let tx = conn.unchecked_transaction()?;
        let mut insert = tx.prepare_cached(
            "INSERT OR REPLACE INTO strings
                 (id, source, translation, status, file_path, context, tags, metadata,
                  char_limit, provider_used, created_at, translated_at, reviewed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?;
        let mut count = 0usize;
        let mut originals = OriginalCache::new();
        for entry in entries {
            let tags_json = serde_json::to_string(&entry.tags)?;
            let metadata_json =
                serde_json::to_string(&persist_original_metadata(&tx, entry, &mut originals)?)?;
            let status_str = entry.status.to_string();
            let file_path_str = entry.file_path.to_string_lossy().to_string();
            let created_at_str = entry.created_at.to_rfc3339();
            let translated_at_str = entry.translated_at.map(|d| d.to_rfc3339());
            let reviewed_at_str = entry.reviewed_at.map(|d| d.to_rfc3339());
            let char_limit = entry.char_limit.map(|l| l as i64);

            insert.execute(params![
                entry.id,
                entry.source,
                entry.translation,
                status_str,
                file_path_str,
                entry.context,
                tags_json,
                metadata_json,
                char_limit,
                entry.provider_used,
                created_at_str,
                translated_at_str,
                reviewed_at_str,
            ])?;
            count += 1;
        }
        drop(insert);
        tx.commit()?;
        Ok(count)
    }

    pub fn get_entries(&self, filter: &EntryFilter) -> Result<Vec<StringEntry>> {
        let conn = lock_connection(&self.conn);
        let (where_clause, mut param_values) = entry_where_clause(filter);
        let mut sql = format!("SELECT {ENTRY_COLUMNS} FROM strings {where_clause}");
        append_entry_pagination(&mut sql, &mut param_values, filter);
        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p as _).collect();
        query_entries(&conn, &sql, &params_refs, &mut OriginalCache::new())
    }

    /// Return a page and its unpaginated total from one strings query. An empty
    /// page has no window total, so count it under the same connection lock.
    pub fn get_entries_page(&self, filter: &EntryFilter) -> Result<(Vec<StringEntry>, usize)> {
        let conn = lock_connection(&self.conn);
        let (where_clause, param_values) = entry_where_clause(filter);
        let mut sql = format!(
            "SELECT {ENTRY_COLUMNS}, COUNT(*) OVER () AS total FROM strings {where_clause}"
        );
        let mut page_params = param_values.clone();
        append_entry_pagination(&mut sql, &mut page_params, filter);
        let mut entries = Vec::new();
        let mut total = None;
        let mut originals = OriginalCache::new();
        {
            let mut stmt = conn.prepare(&sql)?;
            #[cfg(test)]
            record_strings_query();
            let mut rows = stmt.query(rusqlite::params_from_iter(&page_params))?;
            while let Some(row) = rows.next()? {
                total = Some(row.get::<_, usize>("total")?);
                entries.push(entry_from_row(row, &conn, &mut originals)?);
            }
        }
        let total = match total {
            Some(total) => total,
            None => count_filtered_entries(&conn, &where_clause, &param_values)?,
        };
        Ok((entries, total))
    }

    /// All siblings of the selected TextAsset groups, in the same id order as
    /// get_entries, without materializing unrelated strings.
    pub fn get_entries_for_textasset_groups(
        &self,
        group_ids: &HashSet<String>,
    ) -> Result<Vec<StringEntry>> {
        if group_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = lock_connection(&self.conn);
        let mut ids = Vec::new();
        // Filtering with json_extract alone would hide malformed metadata on
        // non-members (and differs from serde for duplicate JSON keys). Scan
        // only id/metadata, retaining neither unrelated rows nor their blobs.
        let mut stmt = conn.prepare("SELECT id, metadata FROM strings ORDER BY id")?;
        let mut rows = stmt.query([])?;
        let mut validated_originals = HashMap::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let metadata = parse_entry_metadata(&id, &row.get::<_, String>(1)?)?;
            if metadata
                .get(crate::textasset_group::GROUP_ID_KEY)
                .and_then(|value| value.as_str())
                .is_some_and(|group_id| group_ids.contains(group_id))
            {
                ids.push(id);
            } else {
                validate_unselected_original(&conn, &metadata, &mut validated_originals)?;
            }
        }
        let mut entries = Vec::new();
        let mut originals = OriginalCache::new();
        for chunk in ids.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!("SELECT id, source, translation, status, file_path, context, tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at FROM strings WHERE id IN ({placeholders}) ORDER BY id");
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|id| id as &dyn rusqlite::types::ToSql)
                .collect();
            entries.extend(query_entries(&conn, &sql, &params_refs, &mut originals)?);
        }
        Ok(entries)
    }

    pub fn get_entry(&self, id: &str) -> Result<Option<StringEntry>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT id, source, translation, status, file_path, context, tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at FROM strings WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(RawEntry {
                id: row.get(0)?,
                source: row.get(1)?,
                translation: row.get(2)?,
                status: row.get(3)?,
                file_path: row.get(4)?,
                context: row.get(5)?,
                tags: row.get(6)?,
                metadata: row.get(7)?,
                char_limit: row.get(8)?,
                provider_used: row.get(9)?,
                created_at: row.get(10)?,
                translated_at: row.get(11)?,
                reviewed_at: row.get(12)?,
            })
        })?;

        match rows.next() {
            Some(row) => Ok(Some(raw_to_entry(row?, &conn, &mut OriginalCache::new())?)),
            None => Ok(None),
        }
    }

    /// Distinct `file_path` and tag values for the whole project.
    /// Tags are a JSON array column; flattened via `json_each`, not by
    /// loading every `StringEntry`.
    pub fn get_string_facets(&self) -> Result<StringFacets> {
        let conn = lock_connection(&self.conn);

        let mut file_stmt =
            conn.prepare("SELECT DISTINCT file_path FROM strings ORDER BY file_path")?;
        let file_paths = file_stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        let mut tag_stmt = conn.prepare(
            "SELECT DISTINCT je.value
             FROM strings
             JOIN json_each(strings.tags) AS je
             WHERE json_valid(strings.tags)
               AND json_type(strings.tags) = 'array'
               AND typeof(je.value) = 'text'
               AND length(je.value) > 0
             ORDER BY je.value",
        )?;
        let tags = tag_stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        Ok(StringFacets { file_paths, tags })
    }

    /// Rows with a non-empty trimmed `translation` — the only inputs pivot needs.
    /// Avoids loading every pending row on large projects (tens of thousands).
    fn entries_with_nonempty_translation(&self) -> Result<Vec<StringEntry>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT id, source, translation, status, file_path, context, tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at
             FROM strings
             WHERE translation IS NOT NULL AND length(trim(translation)) > 0
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RawEntry {
                id: row.get(0)?,
                source: row.get(1)?,
                translation: row.get(2)?,
                status: row.get(3)?,
                file_path: row.get(4)?,
                context: row.get(5)?,
                tags: row.get(6)?,
                metadata: row.get(7)?,
                char_limit: row.get(8)?,
                provider_used: row.get(9)?,
                created_at: row.get(10)?,
                translated_at: row.get(11)?,
                reviewed_at: row.get(12)?,
            })
        })?;
        let mut entries = Vec::new();
        let mut originals = OriginalCache::new();
        for row in rows {
            entries.push(raw_to_entry(row?, &conn, &mut originals)?);
        }
        Ok(entries)
    }

    /// Write a new project DB at `output` whose SOURCE text is this project's
    /// non-empty translations. Does not modify `self`. Refuses to overwrite.
    /// This legacy partial workflow skips missing/blank translations and does
    /// not require a ready status. Use [`Self::pivot_to_complete`] for a full gate.
    pub fn pivot_to(&self, output: &Path) -> Result<PivotResult> {
        self.pivot_to_impl(output, false)
    }

    /// Pivot every row from one consistent snapshot, or refuse before creating
    /// the output DB. Each translation must be nonblank (Unicode whitespace),
    /// Translated/Reviewed/Approved, current, and preserve protected controls.
    /// Empty projects and existing outputs are refused; source rows are unchanged.
    pub fn pivot_to_complete(&self, output: &Path) -> Result<PivotResult> {
        self.pivot_to_impl(output, true)
    }

    fn pivot_to_impl(&self, output: &Path, require_complete: bool) -> Result<PivotResult> {
        if output.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("refusing to overwrite existing file: {}", output.display()),
            )
            .into());
        }

        let entries = if require_complete {
            // One unfiltered SELECT materializes the entire row snapshot. Check
            // and pivot this same inventory, never count then reload/filter it.
            self.get_entries(&EntryFilter::default())?
        } else {
            self.entries_with_nonempty_translation()?
        };
        let mut pivoted: Vec<StringEntry> = Vec::with_capacity(entries.len());
        for e in entries {
            if require_complete {
                if e.translation.as_deref().is_none_or(|t| t.trim().is_empty()) {
                    return Err(LocustError::Other(anyhow::anyhow!(
                        "incomplete pivot: entry '{}' has a missing or blank translation",
                        e.id
                    )));
                }
                if !matches!(
                    e.status,
                    StringStatus::Translated | StringStatus::Reviewed | StringStatus::Approved
                ) {
                    return Err(LocustError::Other(anyhow::anyhow!(
                        "incomplete pivot: entry '{}' is not ready (status: {}); expected translated, reviewed, or approved",
                        e.id, e.status
                    )));
                }
            }
            e.require_current_translation()
                .map_err(|message| LocustError::Other(anyhow::anyhow!(message)))?;
            e.require_preserved_translation_controls()
                .map_err(|message| LocustError::Other(anyhow::anyhow!(message)))?;
            let injection_source = e
                .injection_source()
                .map_err(|message| LocustError::Other(anyhow::anyhow!(message)))?
                .to_string();
            let grouped = crate::textasset_group::is_grouped_entry(&e);
            let capacity = crate::validation::binary_slot_budget(&e)?;
            let Some(translation) = e.translation.filter(|t| !t.trim().is_empty()) else {
                continue;
            };
            let mut ne = StringEntry::new(e.id, translation, e.file_path);
            ne.context = e.context;
            ne.tags = e.tags;
            ne.char_limit = e.char_limit;
            ne.metadata = e.metadata;
            ne.textasset_original = e.textasset_original;
            ne.metadata.insert(
                INJECTION_SOURCE_METADATA_KEY.to_string(),
                serde_json::Value::String(injection_source),
            );
            if !grouped && crate::textasset_group::structural_textasset_capacity(&ne).is_none() {
                if let Some((encoding, bytes)) = capacity {
                    ne.metadata.insert(
                        INJECTION_CAPACITY_METADATA_KEY.to_string(),
                        serde_json::json!({"encoding": encoding, "bytes": bytes}),
                    );
                }
            }
            pivoted.push(ne);
        }

        if pivoted.is_empty() {
            return Err(LocustError::Other(anyhow::anyhow!(
                "no translated entries in {} to pivot from",
                self.path().display()
            )));
        }

        let out_db = Database::open(output)?;
        let count = out_db.save_entries(&pivoted)?;
        {
            let conn = lock_connection(&self.conn);
            let mut stmt = conn.prepare("SELECT key, value FROM project_metadata")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (key, value) = row?;
                out_db.set_project_metadata(&key, &serde_json::from_str(&value)?)?;
            }
        }
        Ok(PivotResult {
            database_path: output.to_string_lossy().into_owned(),
            entries: count,
        })
    }

    /// Count packable translations without materializing entries or originals.
    pub fn count_translated_entries(&self) -> Result<usize> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare(
            "SELECT translation FROM strings WHERE status IN ('translated', 'reviewed', 'approved')",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0))?;
        let mut count = 0;
        for row in rows {
            // SQLite trim() only strips spaces; retain Rust's Unicode rule.
            if row?.is_some_and(|translation| !translation.trim().is_empty()) {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn count_entries(&self, filter: &EntryFilter) -> Result<usize> {
        let conn = lock_connection(&self.conn);
        let (where_clause, param_values) = entry_where_clause(filter);
        count_filtered_entries(&conn, &where_clause, &param_values)
    }

    /// Update an existing string's translation. Returns `true` if a row was
    /// updated, `false` if `entry_id` is unknown (import must not count misses
    /// as successes).
    pub async fn save_translation(
        &self,
        entry_id: &str,
        translation: &str,
        provider: &str,
    ) -> Result<bool> {
        Ok(self
            .save_translation_if_unchanged(entry_id, translation, provider, None)
            .await?
            == TranslationSaveOutcome::Updated)
    }

    /// Atomically compare the original translation and write. None preserves
    /// legacy unconditional saves; Some(None) requires SQL NULL (not "").
    pub async fn save_translation_if_unchanged(
        &self,
        entry_id: &str,
        translation: &str,
        provider: &str,
        expected_translation: Option<Option<String>>,
    ) -> Result<TranslationSaveOutcome> {
        self.save_translation_and_status_if_unchanged(
            entry_id,
            translation,
            provider,
            None,
            expected_translation,
        )
        .await
    }

    /// Save an editor translation and optional final status in one transaction.
    /// The comparison guard, timestamps, provider, and history trigger share the
    /// same write. With no explicit status, retain translation-only semantics.
    pub async fn save_translation_and_status_if_unchanged(
        &self,
        entry_id: &str,
        translation: &str,
        provider: &str,
        status: Option<StringStatus>,
        expected_translation: Option<Option<String>>,
    ) -> Result<TranslationSaveOutcome> {
        let conn = self.conn.clone();
        let entry_id = entry_id.to_string();
        let translation = translation.to_string();
        let provider = provider.to_string();
        let status = status.map(|status| status.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let now = Utc::now().to_rfc3339();
            let tx = conn.unchecked_transaction()?;
            let n = tx.execute(
                "UPDATE strings SET translation = ?1, status = COALESCE(?8, 'translated'),
                 provider_used = ?2, translated_at = ?3,
                 metadata = CASE WHEN ?5 OR ?8 IN ('reviewed', 'approved')
                     THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END,
                 reviewed_at = CASE WHEN ?8 IN ('reviewed', 'approved')
                     THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE reviewed_at END
                 WHERE id = ?4 AND (?6 = 0 OR translation IS ?7)",
                params![
                    translation,
                    provider,
                    now,
                    entry_id,
                    !translation.trim().is_empty(),
                    expected_translation.is_some(),
                    expected_translation.flatten(),
                    status
                ],
            )?;
            let outcome = if n > 0 {
                TranslationSaveOutcome::Updated
            } else if tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM strings WHERE id = ?1)",
                [&entry_id],
                |row| row.get::<_, bool>(0),
            )? {
                TranslationSaveOutcome::Conflict
            } else {
                TranslationSaveOutcome::Missing
            };
            tx.commit()?;
            Ok(outcome)
        })
        .await
        .unwrap()
    }

    /// Apply many translation updates in one transaction (search-replace, bulk
    /// import). Each item is `(entry_id, translation)`. Returns how many rows
    /// were actually updated (unknown ids are skipped, not errors).
    pub async fn save_translations_batch(
        &self,
        updates: Vec<(String, String)>,
        provider: &str,
    ) -> Result<usize> {
        let updates = updates
            .into_iter()
            .map(|(id, translation)| TranslationBatchItem {
                id,
                translation,
                expected_translation: None,
            })
            .collect();
        Ok(self
            .save_translations_batch_if_unchanged(updates, provider)
            .await?
            .applied)
    }

    /// Compare and write each item in one transaction. Conflicts are left
    /// untouched and included in `skipped`; unknown ids are only skipped.
    /// Absent expectations retain unconditional legacy behavior.
    pub async fn save_translations_batch_if_unchanged(
        &self,
        updates: Vec<TranslationBatchItem>,
        provider: &str,
    ) -> Result<TranslationBatchReport> {
        if updates.is_empty() {
            return Ok(TranslationBatchReport::default());
        }
        let conn = self.conn.clone();
        let provider = provider.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let now = Utc::now().to_rfc3339();
            let tx = conn.unchecked_transaction()?;
            let mut report = TranslationBatchReport {
                requested: updates.len(),
                ..Default::default()
            };
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?4 AND (?6 = 0 OR translation IS ?7)",
                )?;
                let mut exists = tx.prepare_cached(
                    "SELECT EXISTS(SELECT 1 FROM strings WHERE id = ?1)",
                )?;
                for update in &updates {
                    let guarded = update.expected_translation.is_some();
                    let expected = update.expected_translation.as_ref().and_then(|v| v.as_deref());
                    let n = stmt.execute(params![update.translation, provider, now, update.id, !update.translation.trim().is_empty(), guarded, expected])?;
                    if n > 0 {
                        report.applied += 1;
                    } else if guarded && exists.query_row([&update.id], |row| row.get::<_, bool>(0))? {
                        report.conflicts.push(update.id.clone());
                    }
                }
            }
            tx.commit()?;
            report.skipped = report.requested - report.applied;
            Ok(report)
        })
        .await
        .unwrap()
    }

    /// Apply imports only to the exact semantic source they were translated
    /// from. Check and write inside one SQLite transaction, including pivots.
    /// Stale/unknown rows remain untouched, including their review metadata.
    pub async fn save_imported_translations_batch(
        &self,
        updates: Vec<crate::export::ImportedTranslation>,
    ) -> Result<ImportApplyReport> {
        self.save_imported_translations_batch_with(updates, false)
            .await
    }

    /// Identical translations on non-pending rows count as imported without changes.
    /// Pending rows are written to confirm their translation, even if identical.
    /// With `keep_existing`, differing nonblank translations are also preserved.
    pub async fn save_imported_translations_batch_with(
        &self,
        updates: Vec<crate::export::ImportedTranslation>,
        keep_existing: bool,
    ) -> Result<ImportApplyReport> {
        validate_import_ids(&updates)?;
        if updates.is_empty() {
            return Ok(ImportApplyReport::default());
        }
        let conn = self.conn.clone();
        let report = tokio::task::spawn_blocking(move || -> Result<_> {
            #[cfg(test)]
            IMPORT_READ_QUERIES.with(|count| count.set(0));
            let conn = lock_connection(&conn);
            let tx = conn.unchecked_transaction()?;
            let now = Utc::now().to_rfc3339();
            let mut report = ImportApplyReport::default();
            {
                let mut write = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = 'import', translated_at = ?2, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?3 AND source = ?4",
                )?;
                let current = read_import_rows(&tx, &updates)?;
                for entry in updates {
                    let decision = classify_import(current.get(&entry.id), &entry, keep_existing);
                    if matches!(decision, ImportDecision::Fill | ImportDecision::Replace { .. } | ImportDecision::Confirm { .. })
                        && write.execute(params![entry.translation, now, entry.id, entry.source, !entry.translation.trim().is_empty()])? == 0 {
                        continue;
                    }
                    decision.record(&mut report);
                }
            }
            tx.commit()?;
            #[cfg(test)]
            let report = (report, IMPORT_READ_QUERIES.with(|count| count.get()));
            Ok(report)
        }).await.map_err(|e| LocustError::Other(anyhow::anyhow!("import save task failed: {e}")))??;
        // Return the blocking worker's measured reads to the calling test thread.
        #[cfg(test)]
        let report = {
            IMPORT_READ_QUERIES.with(|count| count.set(count.get() + report.1));
            report.0
        };
        Ok(report)
    }

    /// Preview an import using only reads. A later save rechecks the current rows.
    pub fn preview_imported_translations(
        &self,
        updates: &[crate::export::ImportedTranslation],
        keep_existing: bool,
    ) -> Result<ImportPreview> {
        validate_import_ids(updates)?;
        let mut preview = ImportPreview::default();
        if updates.is_empty() {
            return Ok(preview);
        }
        let conn = lock_connection(&self.conn);
        let current = read_import_rows(&conn, updates)?;
        for entry in updates {
            let decision = classify_import(current.get(&entry.id), entry, keep_existing);
            decision.record(&mut preview.report);
            let changes = match decision {
                ImportDecision::Replace {
                    previous,
                    previous_status,
                } => Some((&mut preview.replacements, previous, previous_status)),
                ImportDecision::Confirm {
                    previous,
                    previous_status,
                } => Some((&mut preview.confirmations, previous, previous_status)),
                ImportDecision::Fill => {
                    preview.fill_ids.push(entry.id.clone());
                    None
                }
                ImportDecision::Kept => {
                    preview.kept_ids.push(entry.id.clone());
                    None
                }
                ImportDecision::Unchanged => {
                    preview.unchanged_ids.push(entry.id.clone());
                    None
                }
                ImportDecision::Unknown | ImportDecision::StaleSource => None,
            };
            if let Some((changes, previous, previous_status)) = changes {
                changes.push(ImportChange {
                    id: entry.id.clone(),
                    previous,
                    previous_status,
                    new_text: entry.translation.clone(),
                });
            }
        }
        Ok(preview)
    }

    /// Fingerprint only the catalog's affected rows, including absent IDs. Raw
    /// stored values include review/source metadata and timestamps, avoiding
    /// HashMap serialization order and preserving NULL versus empty text.
    /// The transport holds project admission across preview/fingerprint and
    /// across fingerprint verification/save, so edits cannot interleave them.
    pub fn import_rows_fingerprint(
        &self,
        updates: &[crate::export::ImportedTranslation],
    ) -> Result<String> {
        validate_import_ids(updates)?;
        let conn = lock_connection(&self.conn);
        let mut current = BTreeMap::new();
        for entry in updates {
            current.insert(entry.id.clone(), None);
        }
        for chunk in updates.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let mut statement = conn.prepare_cached(&format!(
                "SELECT id, source, translation, status, file_path, context, tags, metadata, CAST(char_limit AS TEXT), provider_used, created_at, translated_at, reviewed_at FROM strings WHERE id IN ({placeholders})"
            ))?;
            let columns = statement.column_count();
            let rows = statement.query_map(
                rusqlite::params_from_iter(chunk.iter().map(|entry| &entry.id)),
                |row| {
                    let fields = (1..columns)
                        .map(|column| row.get::<_, Option<String>>(column))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    Ok((row.get::<_, String>(0)?, fields))
                },
            )?;
            for row in rows {
                let (id, fields) = row?;
                current.insert(id, Some(fields));
            }
        }
        Ok(sha256_hex(&serde_json::to_vec(&current)?))
    }

    /// Commit a provider batch atomically. Unlike an import, a missing entry is
    /// an error: the caller must never announce translations that were not saved.
    pub async fn save_translation_results(&self, results: &[TranslationResult]) -> Result<()> {
        if results.is_empty() {
            return Ok(());
        }
        let conn = self.conn.clone();
        let results = results.to_vec();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let tx = conn.unchecked_transaction()?;
            let now = Utc::now().to_rfc3339();
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?4",
                )?;
                for result in results {
                    if stmt.execute(params![result.translation, result.provider, now, result.entry_id, !result.translation.trim().is_empty()])? != 1 {
                        return Err(LocustError::DatabaseError(rusqlite::Error::QueryReturnedNoRows));
                    }
                }
            }
            tx.commit()?;
            Ok(())
        }).await.map_err(|e| LocustError::ProviderError(format!("translation save task failed: {e}")))?
    }

    /// Automatic provider/cache saves compare their request snapshot in the
    /// same write transaction. One missing or changed row rolls back the entire
    /// batch, including stale-marker removal. Explicit editing APIs stay unconditional.
    pub(crate) async fn save_translation_results_guarded(
        &self,
        results: &[TranslationResult],
        expected: &HashMap<String, TranslationSaveGuard>,
    ) -> Result<()> {
        let mut updates = Vec::with_capacity(results.len());
        let mut ids = std::collections::HashSet::new();
        for result in results {
            if !ids.insert(&result.entry_id) {
                return Err(translation_save_conflict(&result.entry_id));
            }
            let guard = expected
                .get(&result.entry_id)
                .ok_or_else(|| translation_save_conflict(&result.entry_id))?;
            updates.push((
                result.entry_id.clone(),
                result.translation.clone(),
                result.provider.clone(),
                guard.clone(),
            ));
        }
        self.save_guarded_updates(updates).await
    }

    pub(crate) async fn save_translation_guarded(
        &self,
        entry: &StringEntry,
        translation: &str,
        provider: &str,
    ) -> Result<()> {
        self.save_guarded_updates(vec![(
            entry.id.clone(),
            translation.into(),
            provider.into(),
            TranslationSaveGuard::from(entry),
        )])
        .await
    }

    pub(crate) async fn save_guarded_updates(
        &self,
        updates: Vec<(String, String, String, TranslationSaveGuard)>,
    ) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let conn = self.conn.clone();
        let result = tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            let tx = conn.unchecked_transaction()?;
            let now = Utc::now().to_rfc3339();
            {
                let mut statement = tx.prepare_cached(
                    "UPDATE strings SET translation = ?1, status = 'translated', provider_used = ?2, translated_at = ?3, metadata = CASE WHEN ?5 THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END WHERE id = ?4 AND source = ?6 AND translation IS ?7 AND status = ?8 AND provider_used IS ?9"
                )?;
                for (id, translation, provider, guard) in updates {
                    let changed = statement.execute(params![translation, provider, now, id, !translation.trim().is_empty(), guard.source, guard.translation, guard.status, guard.provider])?;
                    if changed != 1 { return Err(translation_save_conflict(&id)); }
                }
            }
            tx.commit()?;
            Ok(())
        }).await.map_err(|e| LocustError::ProviderError(format!("translation save task failed: {e}")))?;
        #[cfg(test)]
        if result.is_ok() {
            GUARDED_TRANSLATION_TRANSACTIONS.with(|count| {
                if let Some(n) = count.get() {
                    count.set(Some(n + 1));
                }
            });
        }
        result
    }

    pub async fn update_entry_status(&self, entry_id: &str, status: StringStatus) -> Result<()> {
        let conn = self.conn.clone();
        let entry_id = entry_id.to_string();
        let status_str = status.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = lock_connection(&conn);
            conn.execute(
                "UPDATE strings SET status = ?1, metadata = CASE WHEN ?1 IN ('reviewed', 'approved') THEN json_remove(metadata, '$.locust_stale_translation') ELSE metadata END, reviewed_at = CASE WHEN ?1 IN ('reviewed', 'approved') THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') ELSE reviewed_at END WHERE id = ?2",
                params![status_str, entry_id],
            )?;
            Ok(())
        })
        .await
        .unwrap()
    }

    pub fn get_stats(&self) -> Result<ProjectStats> {
        let conn = lock_connection(&self.conn);
        // One table scan: the editor polls this after every page of work.
        let mut stmt =
            conn.prepare_cached("SELECT status, COUNT(*) FROM strings GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?;
        let mut stats = ProjectStats::default();
        for row in rows {
            let (status, n) = row?;
            stats.total += n;
            match status.as_str() {
                "pending" => stats.pending = n,
                "translated" => stats.translated = n,
                "reviewed" => stats.reviewed = n,
                "approved" => stats.approved = n,
                "error" => stats.error = n,
                _ => {}
            }
        }
        Ok(stats)
    }

    /// Progress by status, ordered by file path. Pending rows may retain stale text.
    pub fn get_file_stats(&self) -> Result<Vec<FileStats>> {
        let conn = lock_connection(&self.conn);
        let mut stmt = conn.prepare_cached(
            "SELECT file_path, status, COUNT(*) FROM strings GROUP BY file_path, status",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, usize>(2)?,
            ))
        })?;
        let mut files = BTreeMap::new();
        for row in rows {
            let (file_path, status, n) = row?;
            let stats = files.entry(file_path.clone()).or_insert_with(|| FileStats {
                file_path,
                ..FileStats::default()
            });
            stats.total += n;
            match status.as_str() {
                "pending" => stats.pending = n,
                "translated" => stats.translated = n,
                "reviewed" => stats.reviewed = n,
                "approved" => stats.approved = n,
                "error" => stats.error = n,
                _ => {}
            }
        }
        Ok(files.into_values().collect())
    }

    pub fn clear_entries(&self) -> Result<()> {
        let conn = lock_connection(&self.conn);
        conn.execute("DELETE FROM strings", [])?;
        Ok(())
    }

    /// Merge a fresh extract into the live `strings` table without wiping
    /// translations. Existing ids keep translation / status / timestamps /
    /// provider; a changed `source` keeps the translation but forces `pending`.
    /// Pivot DBs instead validate incoming physical sources atomically, retain
    /// semantic sources/provenance, and only refresh valid locators.
    /// Ids missing from `entries` are deleted. One transaction.
    pub fn merge_entries(&self, entries: &[StringEntry]) -> Result<MergeStats> {
        self.merge_entries_impl(entries, false)
    }

    /// Merge an explicitly partial extraction without deleting unread resources.
    /// Matching pivot rows must still prove their physical source is unchanged.
    pub fn merge_entries_preserving_missing(&self, entries: &[StringEntry]) -> Result<MergeStats> {
        self.merge_entries_impl(entries, true)
    }

    fn merge_entries_impl(
        &self,
        entries: &[StringEntry],
        preserve_missing: bool,
    ) -> Result<MergeStats> {
        let conn = lock_connection(&self.conn);
        let tx = conn.unchecked_transaction()?;

        struct Stored {
            source: String,
            file_path: String,
            context: Option<String>,
            tags: String,
            char_limit: Option<i64>,
            metadata: HashMap<String, serde_json::Value>,
            translation: Option<String>,
            status: String,
            provider_used: Option<String>,
            created_at: String,
            translated_at: Option<String>,
            reviewed_at: Option<String>,
        }

        let mut existing: HashMap<String, Stored> = HashMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT id, source, file_path, context, tags, char_limit, metadata,
                        translation, status, provider_used, created_at, translated_at, reviewed_at
                 FROM strings",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                ))
            })?;
            for row in rows {
                let (
                    id,
                    source,
                    file_path,
                    context,
                    tags,
                    char_limit,
                    metadata_json,
                    translation,
                    status,
                    provider_used,
                    created_at,
                    translated_at,
                    reviewed_at,
                ) = row?;
                let metadata = serde_json::from_str(&metadata_json).map_err(|error| {
                    LocustError::Other(anyhow::anyhow!(
                        "entry '{id}' has malformed metadata: {error}"
                    ))
                })?;
                existing.insert(
                    id,
                    Stored {
                        source,
                        file_path,
                        context,
                        tags,
                        char_limit,
                        metadata,
                        translation,
                        status,
                        provider_used,
                        created_at,
                        translated_at,
                        reviewed_at,
                    },
                );
            }
        }

        let mut incoming: HashMap<&str, &StringEntry> = HashMap::new();
        let mut originals = OriginalCache::new();
        for entry in entries {
            incoming.insert(entry.id.as_str(), entry);
        }

        // A pivot DB's source is semantic text, so a fresh extraction may only
        // refresh locators after every existing row proves the same untouched
        // physical baseline. Validate the complete overlap before DELETE/UPDATE.
        let database_is_pivoted = existing
            .values()
            .any(|stored| stored.metadata.contains_key(INJECTION_SOURCE_METADATA_KEY));
        if database_is_pivoted {
            let mut validation_originals = OriginalCache::new();
            let mut matched = 0usize;
            for (id, entry) in &incoming {
                let Some(old) = existing.get(*id) else {
                    continue; // no semantic pivot source exists for a newly extracted row
                };
                let expected = match old.metadata.get(INJECTION_SOURCE_METADATA_KEY) {
                    Some(serde_json::Value::String(value)) if !value.is_empty() => value,
                    _ => {
                        return Err(LocustError::Other(anyhow::anyhow!(
                            "pivot entry '{id}' has malformed {INJECTION_SOURCE_METADATA_KEY} metadata"
                        )));
                    }
                };
                if entry.source != expected.as_str() {
                    return Err(LocustError::Other(anyhow::anyhow!(
                        "source_changed for pivot entry '{id}': selected game does not match the original extraction baseline; extract a fresh project for this game copy"
                    )));
                }
                if let Some(original_hash) = old.metadata.get("locres_source_hash") {
                    if entry.metadata.get("locres_source_hash") != Some(original_hash) {
                        return Err(LocustError::Other(anyhow::anyhow!(
                            "source_changed for pivot locres entry '{id}': Unreal source hash no longer matches the original baseline"
                        )));
                    }
                }
                if old
                    .metadata
                    .contains_key(crate::textasset_group::GROUP_ORIGINAL_REF_KEY)
                    || old
                        .metadata
                        .contains_key(crate::textasset_group::GROUP_ORIGINAL_KEY)
                {
                    let mut old_entry = StringEntry::new(*id, &old.source, PathBuf::new());
                    old_entry.metadata = old.metadata.clone();
                    hydrate_original(&tx, &mut old_entry, &mut validation_originals)?;
                    let expected = crate::textasset_group::shared_original(&old_entry)
                        .map_err(original_error)?;
                    let actual =
                        crate::textasset_group::shared_original(entry).map_err(original_error)?;
                    if expected.as_ref().map(|o| o.sha256()) != actual.as_ref().map(|o| o.sha256())
                    {
                        return Err(original_error(format!("source_changed for pivot TextAsset '{id}': original blob no longer matches the physical baseline")));
                    }
                }
                matched += 1;
            }
            if !preserve_missing && matched != existing.len() {
                return Err(LocustError::Other(anyhow::anyhow!(
                    "source_changed: extraction is missing entries from this pivot's original baseline; extract a fresh project for this game copy"
                )));
            }
            // Do not mix newly extracted physical-language rows into a DB whose
            // source language is the prior pivot translation.
            incoming.retain(|id, _| existing.contains_key(*id));
        }

        let mut stats = MergeStats::default();

        {
            let mut delete = tx.prepare_cached("DELETE FROM strings WHERE id = ?1")?;
            for (id, stored) in &existing {
                if !preserve_missing && !incoming.contains_key(id.as_str()) {
                    delete.execute(params![id])?;
                    stats.removed += 1;
                    if has_saved_translation(&stored.translation) {
                        stats.lost_translations += 1;
                    }
                }
            }
        }

        {
            let mut update = tx.prepare_cached(
                "UPDATE strings SET
                        source = ?1,
                        file_path = ?2,
                        context = ?3,
                        tags = ?4,
                        metadata = ?5,
                        char_limit = ?6,
                        translation = ?7,
                        status = ?8,
                        provider_used = ?9,
                        created_at = ?10,
                        translated_at = ?11,
                        reviewed_at = ?12
                     WHERE id = ?13",
            )?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO strings
                     (id, source, translation, status, file_path, context, tags, metadata,
                      char_limit, provider_used, created_at, translated_at, reviewed_at)
                     VALUES (?1, ?2, NULL, 'pending', ?3, ?4, ?5, ?6, ?7, NULL, ?8, NULL, NULL)",
            )?;

            for (id, entry) in incoming {
                let tags_json = serde_json::to_string(&entry.tags)?;
                let file_path_str = entry.file_path.to_string_lossy().to_string();
                let char_limit = entry.char_limit.map(|l| l as i64);

                if let Some(old) = existing.get(id) {
                    let pivoted = old.metadata.contains_key(INJECTION_SOURCE_METADATA_KEY);
                    let source_changed = !pivoted && old.source != entry.source;
                    let status = if source_changed {
                        stats.stale_source_reset += 1;
                        if has_saved_translation(&old.translation) {
                            stats.lost_translations += 1;
                        }
                        StringStatus::Pending.to_string()
                    } else {
                        old.status.clone()
                    };
                    let source = if pivoted {
                        old.source.as_str()
                    } else {
                        entry.source.as_str()
                    };
                    let mut metadata = persist_original_metadata(&tx, entry, &mut originals)?;
                    if let Some(marker) = old.metadata.get(STALE_TRANSLATION_METADATA_KEY) {
                        // Refreshing locators must never silently accept an old
                        // translation. Preserve its first source hash over any
                        // number of edits; corrupt/unknown markers remain errors.
                        let mut marker = marker.clone();
                        if source_changed
                            && marker.get("version").and_then(serde_json::Value::as_u64) == Some(1)
                        {
                            if let Some(object) = marker.as_object_mut() {
                                object.insert(
                                    "current_source_sha256".into(),
                                    serde_json::json!(sha256_hex(source.as_bytes())),
                                );
                            }
                        }
                        metadata.insert(STALE_TRANSLATION_METADATA_KEY.into(), marker);
                    } else if source_changed
                        && old
                            .translation
                            .as_deref()
                            .is_some_and(|text| !text.trim().is_empty())
                    {
                        metadata.insert(
                            STALE_TRANSLATION_METADATA_KEY.into(),
                            serde_json::json!({
                                "version": 1,
                                "translated_source_sha256": sha256_hex(old.source.as_bytes()),
                                "current_source_sha256": sha256_hex(source.as_bytes()),
                            }),
                        );
                    }
                    if pivoted {
                        for key in [
                            INJECTION_SOURCE_METADATA_KEY,
                            INJECTION_CAPACITY_METADATA_KEY,
                            "locres_source_hash",
                        ] {
                            if let Some(value) = old.metadata.get(key) {
                                metadata.insert(key.to_string(), value.clone());
                            }
                        }
                    }
                    if has_saved_translation(&old.translation) {
                        stats.preserved_translations += 1;
                    }
                    if old.source == source
                        && old.file_path == file_path_str
                        && old.context == entry.context
                        && old.tags == tags_json
                        && old.char_limit == char_limit
                        && old.metadata == metadata
                        && old.status == status
                    {
                        continue;
                    }
                    stats.updated += 1;
                    let metadata_json = serde_json::to_string(&metadata)?;
                    update.execute(params![
                        source,
                        file_path_str,
                        entry.context,
                        tags_json,
                        metadata_json,
                        char_limit,
                        old.translation,
                        status,
                        old.provider_used,
                        old.created_at,
                        old.translated_at,
                        old.reviewed_at,
                        entry.id,
                    ])?;
                } else {
                    let metadata_json = serde_json::to_string(&persist_original_metadata(
                        &tx,
                        entry,
                        &mut originals,
                    )?)?;
                    stats.added += 1;
                    let created_at = entry.created_at.to_rfc3339();
                    insert.execute(params![
                        entry.id,
                        entry.source,
                        file_path_str,
                        entry.context,
                        tags_json,
                        metadata_json,
                        char_limit,
                        created_at,
                    ])?;
                }
            }
        }

        tx.commit()?;
        Ok(stats)
    }
}

enum ImportDecision {
    Unknown,
    StaleSource,
    Unchanged,
    Kept,
    Fill,
    Replace {
        previous: String,
        previous_status: String,
    },
    /// An identical translation on a pending row still needs its status confirmed.
    Confirm {
        previous: String,
        previous_status: String,
    },
}

impl ImportDecision {
    fn record(&self, report: &mut ImportApplyReport) {
        match self {
            Self::Unknown => report.unknown_ids += 1,
            Self::StaleSource => report.stale_sources += 1,
            Self::Unchanged => {
                report.imported += 1;
                report.unchanged += 1;
            }
            Self::Kept => report.kept_existing += 1,
            Self::Fill | Self::Replace { .. } | Self::Confirm { .. } => report.imported += 1,
        }
    }
}

fn validate_import_ids(updates: &[crate::export::ImportedTranslation]) -> Result<()> {
    let mut seen = HashSet::new();
    for entry in updates {
        if !seen.insert(&entry.id) {
            return Err(LocustError::ValidationError {
                entry_id: entry.id.clone(),
                message: "duplicate import id; no translations were saved".into(),
            });
        }
    }
    Ok(())
}

type ImportRow = (String, Option<String>, String);

/// Use the caller's connection so a save reads and writes in the same transaction.
fn read_import_rows(
    conn: &Connection,
    updates: &[crate::export::ImportedTranslation],
) -> Result<HashMap<String, ImportRow>> {
    let mut current = HashMap::with_capacity(updates.len());
    for chunk in updates.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let mut statement = conn.prepare_cached(&format!(
            "SELECT id, source, translation, status FROM strings WHERE id IN ({placeholders})"
        ))?;
        #[cfg(test)]
        IMPORT_READ_QUERIES.with(|count| count.set(count.get() + 1));
        let rows = statement.query_map(
            rusqlite::params_from_iter(chunk.iter().map(|entry| &entry.id)),
            |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?))),
        )?;
        for row in rows {
            let (id, state) = row?;
            current.insert(id, state);
        }
    }
    Ok(current)
}

fn classify_import(
    current: Option<&ImportRow>,
    entry: &crate::export::ImportedTranslation,
    keep_existing: bool,
) -> ImportDecision {
    let Some((source, translation, status)) = current else {
        return ImportDecision::Unknown;
    };
    if source != &entry.source {
        return ImportDecision::StaleSource;
    }
    if translation.as_deref() == Some(entry.translation.as_str()) {
        return if status != "pending" {
            ImportDecision::Unchanged
        } else {
            ImportDecision::Confirm {
                previous: entry.translation.clone(),
                previous_status: status.clone(),
            }
        };
    }
    match translation.as_ref().filter(|text| !text.trim().is_empty()) {
        Some(_) if keep_existing => ImportDecision::Kept,
        Some(previous) => ImportDecision::Replace {
            previous: previous.clone(),
            previous_status: status.clone(),
        },
        None => ImportDecision::Fill,
    }
}

fn translation_save_conflict(id: &str) -> LocustError {
    LocustError::ValidationError {
        entry_id: id.to_owned(),
        message: "translation_conflict: source or translation changed while this result was being computed (or the entry was removed); no results from this batch were saved; refresh and retry".into(),
    }
}

fn has_saved_translation(translation: &Option<String>) -> bool {
    translation.as_ref().is_some_and(|text| !text.is_empty())
}

struct RawEntry {
    id: String,
    source: String,
    translation: Option<String>,
    status: String,
    file_path: String,
    context: Option<String>,
    tags: String,
    metadata: String,
    char_limit: Option<i64>,
    provider_used: Option<String>,
    created_at: String,
    translated_at: Option<String>,
    reviewed_at: Option<String>,
}

type OriginalCache = HashMap<String, Arc<crate::models::TextAssetOriginal>>;

const ENTRY_COLUMNS: &str = "id, source, translation, status, file_path, context, tags, metadata, char_limit, provider_used, created_at, translated_at, reviewed_at";

fn entry_where_clause(filter: &EntryFilter) -> (String, Vec<Value>) {
    let mut sql = String::from("WHERE 1=1");
    let mut params = Vec::new();
    if let Some(ref status) = filter.status {
        sql.push_str(" AND status = ?");
        params.push(Value::Text(status.to_string()));
    }
    if let Some(ref file_path) = filter.file_path {
        sql.push_str(" AND file_path = ?");
        params.push(Value::Text(file_path.clone()));
    }
    if let Some(ref tag) = filter.tag {
        sql.push_str(" AND tags LIKE ?");
        params.push(Value::Text(format!("%\"{}\"%", tag)));
    }
    if let Some(ref search) = filter.search {
        sql.push_str(" AND (source LIKE ? OR translation LIKE ?)");
        let pattern = format!("%{}%", search);
        params.push(Value::Text(pattern.clone()));
        params.push(Value::Text(pattern));
    }
    (sql, params)
}

fn append_entry_pagination(sql: &mut String, params: &mut Vec<Value>, filter: &EntryFilter) {
    sql.push_str(" ORDER BY id");
    if let Some(limit) = filter.limit {
        sql.push_str(" LIMIT ?");
        params.push(Value::Integer(limit as i64));
    }
    if let Some(offset) = filter.offset {
        if filter.limit.is_none() {
            sql.push_str(" LIMIT -1");
        }
        sql.push_str(" OFFSET ?");
        params.push(Value::Integer(offset as i64));
    }
}

fn count_filtered_entries(
    conn: &Connection,
    where_clause: &str,
    params: &[Value],
) -> Result<usize> {
    let sql = format!("SELECT COUNT(*) FROM strings {where_clause}");
    #[cfg(test)]
    record_strings_query();
    Ok(conn.query_row(&sql, rusqlite::params_from_iter(params), |row| row.get(0))?)
}

#[cfg(test)]
fn record_strings_query() {
    STRINGS_QUERIES.with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
}

fn query_entries(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::types::ToSql],
    originals: &mut OriginalCache,
) -> Result<Vec<StringEntry>> {
    let mut stmt = conn.prepare(sql)?;
    #[cfg(test)]
    record_strings_query();
    let mut rows = stmt.query(params)?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next()? {
        entries.push(entry_from_row(row, conn, originals)?);
    }
    Ok(entries)
}

fn entry_from_row(
    row: &rusqlite::Row<'_>,
    conn: &Connection,
    originals: &mut OriginalCache,
) -> Result<StringEntry> {
    let raw = RawEntry {
        id: row.get(0)?,
        source: row.get(1)?,
        translation: row.get(2)?,
        status: row.get(3)?,
        file_path: row.get(4)?,
        context: row.get(5)?,
        tags: row.get(6)?,
        metadata: row.get(7)?,
        char_limit: row.get(8)?,
        provider_used: row.get(9)?,
        created_at: row.get(10)?,
        translated_at: row.get(11)?,
        reviewed_at: row.get(12)?,
    };
    #[cfg(test)]
    ENTRY_ROWS_MATERIALIZED.with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
    raw_to_entry(raw, conn, originals)
}

fn parse_entry_metadata(id: &str, json: &str) -> Result<HashMap<String, serde_json::Value>> {
    serde_json::from_str(json).map_err(|error| {
        LocustError::Other(anyhow::anyhow!(
            "entry '{id}' has malformed metadata: {error}"
        ))
    })
}

// Preserve get_entries' original-metadata errors even for unselected groups.
// Check shared payloads once per digest, retaining only their lengths; unrelated
// StringEntries are never hydrated; payloads are discarded after validation.
fn validate_unselected_original(
    conn: &Connection,
    metadata: &HashMap<String, serde_json::Value>,
    validated: &mut HashMap<String, usize>,
) -> Result<()> {
    use crate::textasset_group as group;
    let inline = metadata
        .get(group::GROUP_ORIGINAL_KEY)
        .map(|value| {
            let text = value
                .as_str()
                .ok_or_else(|| original_error("malformed TextAsset original".into()))?;
            crate::models::TextAssetOriginal::new(text).map_err(original_error)
        })
        .transpose()?;
    if let Some(value) = metadata.get(group::GROUP_ORIGINAL_REF_KEY) {
        let reference = value
            .as_str()
            .ok_or_else(|| original_error("malformed TextAsset reference".into()))?;
        let byte_len = if let Some(original) = inline {
            if reference != original.sha256() {
                return Err(original_error(
                    "shared TextAsset original digest or length does not match metadata".into(),
                ));
            }
            original.text().len()
        } else if let Some(byte_len) = validated.get(reference) {
            *byte_len
        } else {
            let original =
                load_original(conn, reference, &mut OriginalCache::new())?.ok_or_else(|| {
                    original_error(format!("missing shared TextAsset original {reference}"))
                })?;
            let byte_len = original.text().len();
            validated.insert(reference.to_owned(), byte_len);
            byte_len
        };
        if metadata
            .get(group::GROUP_ORIGINAL_BYTES_KEY)
            .and_then(|value| value.as_u64())
            != Some(byte_len as u64)
        {
            return Err(original_error(
                "shared TextAsset original digest or length does not match metadata".into(),
            ));
        }
    } else if metadata.contains_key(group::GROUP_ORIGINAL_BYTES_KEY) {
        return Err(original_error(
            "TextAsset original length has no digest reference".into(),
        ));
    }
    Ok(())
}

fn original_error(message: String) -> LocustError {
    LocustError::Other(anyhow::anyhow!(message))
}

fn load_original(
    conn: &Connection,
    reference: &str,
    cache: &mut OriginalCache,
) -> Result<Option<Arc<crate::models::TextAssetOriginal>>> {
    if let Some(original) = cache.get(reference) {
        return Ok(Some(Arc::clone(original)));
    }
    if reference.len() != 64
        || !reference
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(original_error(
            "malformed TextAsset SHA256 reference".into(),
        ));
    }
    let mut stmt = conn.prepare("SELECT byte_len, CASE WHEN length(CAST(payload AS BLOB)) <= 1048576 THEN payload ELSE NULL END FROM textasset_originals WHERE sha256 = ?1")?;
    let mut rows = stmt.query(params![reference])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let byte_len: i64 = row.get(0)?;
    let payload: Option<String> = row.get(1)?;
    let payload =
        payload.ok_or_else(|| original_error("oversized TextAsset original in database".into()))?;
    let original =
        Arc::new(crate::models::TextAssetOriginal::new(&payload).map_err(original_error)?);
    if byte_len != payload.len() as i64 || original.sha256() != reference {
        return Err(original_error(
            "corrupt shared TextAsset original: digest or byte length mismatch".into(),
        ));
    }
    cache.insert(reference.to_owned(), Arc::clone(&original));
    Ok(Some(original))
}

fn hydrate_original(
    conn: &Connection,
    entry: &mut StringEntry,
    cache: &mut OriginalCache,
) -> Result<()> {
    use crate::textasset_group as group;
    if entry.textasset_original.is_none() && !entry.metadata.contains_key(group::GROUP_ORIGINAL_KEY)
    {
        if let Some(value) = entry.metadata.get(group::GROUP_ORIGINAL_REF_KEY) {
            let reference = value
                .as_str()
                .ok_or_else(|| original_error("malformed TextAsset reference".into()))?;
            entry.textasset_original =
                Some(load_original(conn, reference, cache)?.ok_or_else(|| {
                    original_error(format!("missing shared TextAsset original {reference}"))
                })?);
        }
    }
    if let Some(original) = group::shared_original(entry).map_err(original_error)? {
        let shared = cache
            .entry(original.sha256().to_owned())
            .or_insert(original);
        group::attach_shared_original(entry, Arc::clone(shared));
    }
    Ok(())
}

fn persist_original_metadata(
    conn: &Connection,
    entry: &StringEntry,
    cache: &mut OriginalCache,
) -> Result<HashMap<String, serde_json::Value>> {
    use crate::textasset_group as group;
    // Skip legacy payload before cloning metadata: normalization must not make
    // another full original copy merely to remove it again.
    let mut normalized = StringEntry::new(&entry.id, "", PathBuf::new());
    normalized.metadata = entry
        .metadata
        .iter()
        .filter(|(key, _)| key.as_str() != group::GROUP_ORIGINAL_KEY)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if entry.textasset_original.is_none() && !entry.metadata.contains_key(group::GROUP_ORIGINAL_KEY)
    {
        hydrate_original(conn, &mut normalized, cache)?;
    } else {
        normalized.textasset_original = group::shared_original(entry).map_err(original_error)?;
    }
    if let Some(original) = &normalized.textasset_original {
        if !cache.contains_key(original.sha256())
            && load_original(conn, original.sha256(), cache)?.is_none()
        {
            conn.execute(
                "INSERT INTO textasset_originals(sha256, byte_len, payload) VALUES (?1, ?2, ?3)",
                params![
                    original.sha256(),
                    original.text().len() as i64,
                    original.text()
                ],
            )?;
            cache.insert(original.sha256().to_owned(), Arc::clone(original));
        }
        let original = Arc::clone(original);
        group::attach_shared_original(&mut normalized, original);
    } else {
        hydrate_original(conn, &mut normalized, cache)?;
    }
    Ok(normalized.metadata)
}

fn raw_to_entry(
    raw: RawEntry,
    conn: &Connection,
    originals: &mut OriginalCache,
) -> Result<StringEntry> {
    let status: StringStatus = raw.status.parse().unwrap_or(StringStatus::Pending);
    let tags: Vec<String> = serde_json::from_str(&raw.tags).unwrap_or_default();
    let metadata = parse_entry_metadata(&raw.id, &raw.metadata)?;
    let created_at: DateTime<Utc> = DateTime::parse_from_rfc3339(&raw.created_at)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let translated_at = raw
        .translated_at
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));
    let reviewed_at = raw
        .reviewed_at
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));

    let mut entry = StringEntry {
        id: raw.id,
        source: raw.source,
        translation: raw.translation,
        file_path: PathBuf::from(raw.file_path),
        context: raw.context,
        tags,
        metadata,
        textasset_original: None,
        status,
        provider_used: raw.provider_used,
        char_limit: raw.char_limit.map(|l| l as usize),
        created_at,
        translated_at,
        reviewed_at,
    };
    hydrate_original(conn, &mut entry, originals)?;
    Ok(entry)
}

#[cfg(test)]
mod tests;
