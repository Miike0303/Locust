use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use locust_core::database::{EntryFilter, GlossaryEntry, PivotResult, ProjectStats, StringFacets};
use locust_core::extraction::PluginInfo;
use locust_core::models::{OutputMode, StringEntry, StringStatus};
use locust_core::project;
use locust_core::translation::TranslationOptionsRequest;
use locust_core::validation::Validator;
use locust_server::{
    cancel_translation_job, poll_xai_device_login, remember_open_project,
    run_owned_project_operation, spawn_translation_job, start_xai_device_login,
    try_project_operation, update_app_config, AppState, ProjectInfo, XaiAuthPollResponse,
    XaiAuthStartResponse, INJECT_EMPTY_LANGUAGES_MESSAGE,
};

/// Wrapper so we can use Arc<AppState> as Tauri managed state
pub struct AppStateWrapper(pub Arc<AppState>);
pub struct ServerPort(pub u16);

// ─── Utility commands ───────────────────────────────────────────────────────

#[tauri::command]
pub fn get_server_port(port: State<ServerPort>) -> u16 {
    port.0
}

// ─── Project commands ───────────────────────────────────────────────────────

// Plain derive is fine here — unlike `TokenStore`, every field is already
// serialized to the frontend, so `{:?}` exposes nothing new.
#[derive(Serialize, Debug)]
pub struct ProjectOpenResponse {
    pub format_id: String,
    pub format_name: String,
    pub total_strings: usize,
    pub project_path: String,
    pub project_name: String,
    pub supported_modes: Vec<OutputMode>,
    pub database_path: String,
    pub added: usize,
    pub updated: usize,
    pub stale_source_reset: usize,
    pub removed: usize,
    pub preserved_translations: usize,
    pub extraction_warnings: Vec<String>,
    pub persistence_warning: Option<String>,
}

/// Advisory verification only; confirmation revalidates under the source lock.
#[tauri::command]
pub fn preflight_project_open(
    game_path: String,
    format: Option<String>,
    state: State<'_, AppStateWrapper>,
) -> Result<project::ProjectOpenPreflight, String> {
    project::preflight_project_open(
        &state.0.format_registry,
        Path::new(&game_path),
        format.as_deref(),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn resume_project(
    database_path: String,
    game_path: String,
    format_id: String,
    state: State<'_, AppStateWrapper>,
) -> Result<ProjectOpenResponse, String> {
    apply_saved_project(&state.0, database_path, game_path, format_id, true).await
}

#[tauri::command]
pub async fn open_project(
    path: String,
    format_id: Option<String>,
    prefer_saved: Option<bool>,
    state: State<'_, AppStateWrapper>,
) -> Result<ProjectOpenResponse, String> {
    apply_open_project(&state.0, path, format_id, prefer_saved.unwrap_or(false)).await
}

async fn apply_open_project(
    s: &Arc<AppState>,
    path: String,
    format_id: Option<String>,
    prefer_saved: bool,
) -> Result<ProjectOpenResponse, String> {
    let s = s.clone();
    let exclusive = try_project_operation(&s)?;
    run_owned_project_operation(exclusive, async move {
        let raw_path = PathBuf::from(&path);
        let outcome = if prefer_saved {
            project::open_recent_project(&s.db, &s.format_registry, &raw_path, format_id.as_deref())
        } else {
            project::open_project(&s.db, &s.format_registry, &raw_path, format_id.as_deref())
        }
        .map_err(|e| e.to_string())?;

        let persistence_warning = remember_open_project(&s, &outcome, true).await;
        {
            let mut proj = s.current_project.write().await;
            *proj = Some(ProjectInfo {
                path: outcome.project_path.clone(),
                format_id: outcome.format_id.clone(),
                name: outcome.project_name.clone(),
                extraction_warnings: outcome.extraction_warnings.clone(),
                database_path: Some(outcome.database_path.clone()),
                supported_modes: outcome.supported_modes.clone(),
                persistence_warning: persistence_warning.clone(),
            });
        }

        Ok(ProjectOpenResponse {
            format_id: outcome.format_id,
            format_name: outcome.format_name,
            total_strings: outcome.total_strings,
            project_path: outcome.project_path.to_string_lossy().into_owned(),
            project_name: outcome.project_name,
            supported_modes: outcome.supported_modes,
            database_path: outcome.database_path.to_string_lossy().into_owned(),
            added: outcome.added,
            updated: outcome.updated,
            stale_source_reset: outcome.stale_source_reset,
            removed: outcome.removed,
            preserved_translations: outcome.preserved_translations,
            extraction_warnings: outcome.extraction_warnings,
            persistence_warning,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn apply_open_project_db(
    s: &Arc<AppState>,
    database_path: String,
    game_path: String,
    format_id: String,
) -> Result<ProjectOpenResponse, String> {
    apply_saved_project(s, database_path, game_path, format_id, false).await
}

async fn apply_saved_project(
    s: &Arc<AppState>,
    database_path: String,
    game_path: String,
    format_id: String,
    verified_resume: bool,
) -> Result<ProjectOpenResponse, String> {
    let s = s.clone();
    let exclusive = try_project_operation(&s)?;
    run_owned_project_operation(exclusive, async move {
        let open = if verified_resume {
            project::open_verified_saved_project
        } else {
            project::open_project_db
        };
        let outcome = open(
            &s.db,
            &s.format_registry,
            Path::new(&database_path),
            Path::new(&game_path),
            &format_id,
        )
        .map_err(|e| e.to_string())?;

        let persistence_warning = remember_open_project(&s, &outcome, true).await;
        {
            let mut proj = s.current_project.write().await;
            *proj = Some(ProjectInfo {
                path: outcome.project_path.clone(),
                format_id: outcome.format_id.clone(),
                name: outcome.project_name.clone(),
                extraction_warnings: outcome.extraction_warnings.clone(),
                database_path: Some(outcome.database_path.clone()),
                supported_modes: outcome.supported_modes.clone(),
                persistence_warning: persistence_warning.clone(),
            });
        }

        Ok(ProjectOpenResponse {
            format_id: outcome.format_id,
            format_name: outcome.format_name,
            total_strings: outcome.total_strings,
            project_path: outcome.project_path.to_string_lossy().into_owned(),
            project_name: outcome.project_name,
            supported_modes: outcome.supported_modes,
            database_path: outcome.database_path.to_string_lossy().into_owned(),
            added: outcome.added,
            updated: outcome.updated,
            stale_source_reset: outcome.stale_source_reset,
            removed: outcome.removed,
            preserved_translations: outcome.preserved_translations,
            extraction_warnings: outcome.extraction_warnings,
            persistence_warning,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open an existing Locust project database without extracting or merging.
#[tauri::command]
pub async fn open_project_db(
    database_path: String,
    game_path: String,
    format_id: String,
    state: State<'_, AppStateWrapper>,
) -> Result<ProjectOpenResponse, String> {
    apply_open_project_db(&state.0, database_path, game_path, format_id).await
}

// ─── Format & Provider commands ─────────────────────────────────────────────

#[tauri::command]
pub fn get_formats(state: State<AppStateWrapper>) -> Vec<PluginInfo> {
    format_list(&state.0)
}

fn format_list(state: &Arc<AppState>) -> Vec<PluginInfo> {
    state.format_registry.list()
}

#[tauri::command]
pub async fn get_providers(
    state: State<'_, AppStateWrapper>,
) -> Result<Vec<serde_json::Value>, String> {
    let reg = state.0.provider_registry.read().await;
    Ok(
        serde_json::to_value(locust_providers::list_providers_for_api(&reg))
            .unwrap_or_default()
            .as_array()
            .cloned()
            .unwrap_or_default(),
    )
}

// ─── String commands ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct StringsFilter {
    pub status: Option<String>,
    pub file_path: Option<String>,
    pub tag: Option<String>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Serialize)]
pub struct StringsResponse {
    pub entries: Vec<StringEntry>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
}

#[tauri::command]
pub async fn get_stats(state: State<'_, AppStateWrapper>) -> Result<ProjectStats, String> {
    if state.0.current_project.read().await.is_none() {
        return Ok(ProjectStats::default());
    }
    state.0.db.get_stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_strings(
    filter: StringsFilter,
    state: State<'_, AppStateWrapper>,
) -> Result<StringsResponse, String> {
    let limit = filter.limit.unwrap_or(100);
    let offset = filter.offset.unwrap_or(0);
    if state.0.current_project.read().await.is_none() {
        return Ok(StringsResponse {
            entries: vec![],
            total: 0,
            offset,
            limit,
        });
    }

    let status = filter.status.and_then(|s| s.parse::<StringStatus>().ok());

    let entry_filter = EntryFilter {
        status,
        file_path: filter.file_path,
        tag: filter.tag,
        search: filter.search,
        limit: Some(limit),
        offset: Some(offset),
    };
    let (entries, total) = state
        .0
        .db
        .get_entries_page(&entry_filter)
        .map_err(|e| e.to_string())?;

    Ok(StringsResponse {
        entries,
        total,
        offset,
        limit,
    })
}

#[tauri::command]
pub async fn get_string_facets(state: State<'_, AppStateWrapper>) -> Result<StringFacets, String> {
    if state.0.current_project.read().await.is_none() {
        return Ok(StringFacets::default());
    }
    state.0.db.get_string_facets().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_injection_recordings(
    state: State<'_, AppStateWrapper>,
) -> Result<Vec<Option<String>>, String> {
    if state.0.current_project.read().await.is_none() {
        return Ok(vec![]);
    }
    state.0.db.list_recorded_langs().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn run_pivot(
    output_path: String,
    require_complete: Option<bool>,
    state: State<'_, AppStateWrapper>,
) -> Result<PivotResult, String> {
    apply_run_pivot(&state.0, output_path, require_complete).await
}

async fn apply_run_pivot(
    s: &Arc<AppState>,
    output_path: String,
    require_complete: Option<bool>,
) -> Result<PivotResult, String> {
    let _exclusive = try_project_operation(s)?;
    if s.current_project.read().await.is_none() {
        return Err("no project open".into());
    }
    let output = PathBuf::from(output_path);
    let result = if require_complete.unwrap_or(false) {
        s.db.pivot_to_complete(&output)
    } else {
        s.db.pivot_to(&output)
    };
    result.map_err(|e| e.to_string())
}

#[derive(Deserialize)]
pub struct PatchStringReq {
    pub translation: Option<String>,
    pub status: Option<StringStatus>,
    #[serde(
        default,
        deserialize_with = "locust_core::database::deserialize_expected_translation"
    )]
    pub expected_translation: Option<Option<String>>,
}

#[tauri::command]
pub async fn patch_string(
    id: String,
    data: PatchStringReq,
    state: State<'_, AppStateWrapper>,
) -> Result<StringEntry, String> {
    let s = state.0.clone();
    let exclusive = try_project_operation(&s)?;
    run_owned_project_operation(exclusive, async move {
        if s.current_project.read().await.is_none() {
            return Err("no project open".into());
        }
        apply_string_patch(&s.db, &id, data).await
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn apply_string_patch(
    db: &locust_core::database::Database,
    id: &str,
    data: PatchStringReq,
) -> Result<StringEntry, String> {
    if let Some(ref translation) = data.translation {
        let outcome = db
            .save_translation_and_status_if_unchanged(
                id,
                translation,
                "manual",
                data.status,
                data.expected_translation,
            )
            .await
            .map_err(|e| e.to_string())?;
        if outcome == locust_core::database::TranslationSaveOutcome::Conflict {
            return Err(locust_core::database::TRANSLATION_CONFLICT_MESSAGE.into());
        }
    } else if let Some(ref status) = data.status {
        db.update_entry_status(id, status.clone())
            .await
            .map_err(|e| e.to_string())?;
    }
    db.get_entry(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Entry not found".to_string())
}

pub type BatchPatchItem = locust_core::database::TranslationBatchItem;

#[derive(Deserialize)]
pub struct BatchPatchReq {
    pub updates: Vec<BatchPatchItem>,
    #[serde(default = "default_batch_provider")]
    pub provider: String,
}

fn default_batch_provider() -> String {
    "manual".into()
}

/// Bulk translation updates in one SQLite transaction (search-replace).
#[tauri::command]
pub async fn batch_patch_strings(
    data: BatchPatchReq,
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    if data.updates.len() > 50_000 {
        return Err("batch too large (max 50000 updates)".into());
    }
    let s = state.0.clone();
    let exclusive = try_project_operation(&s)?;
    run_owned_project_operation(exclusive, async move {
        if s.current_project.read().await.is_none() {
            return Err("no project open".into());
        }
        let report =
            s.db.save_translations_batch_if_unchanged(data.updates, &data.provider)
                .await
                .map_err(|e| e.to_string())?;
        Ok(serde_json::json!(report))
    })
    .await
    .map_err(|e| e.to_string())?
}

// ─── Translation commands ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TranslateParams {
    pub provider_id: String,
    #[serde(default)]
    pub fallback_provider_ids: Option<Vec<String>>,
    pub options: TranslationOptionsRequest,
}

#[tauri::command]
pub async fn start_translation(
    params: TranslateParams,
    state: State<'_, AppStateWrapper>,
) -> Result<String, String> {
    spawn_translation_job(
        &state.0,
        params.provider_id,
        params.fallback_provider_ids,
        params.options,
    )
    .await
}

#[tauri::command]
pub async fn cancel_translation(
    job_id: String,
    state: State<'_, AppStateWrapper>,
) -> Result<(), String> {
    cancel_translation_job(&state.0, &job_id)
}

// ─── Validation & Injection ─────────────────────────────────────────────────

#[tauri::command]
pub async fn run_validation(
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    apply_run_validation(&state.0).await
}

async fn apply_run_validation(s: &Arc<AppState>) -> Result<serde_json::Value, String> {
    let s = s.clone();
    // Same exclusive as HTTP validate: issue writes must not race Database::reopen.
    let exclusive = try_project_operation(&s)?;
    run_owned_project_operation(exclusive, async move {
        if s.current_project.read().await.is_none() {
            return Err("no project open".into());
        }
        let entries =
            s.db.get_entries(&EntryFilter::default())
                .map_err(|e| e.to_string())?;
        let validation = Validator::validate_and_save(&entries, &s.db)
            .await
            .map_err(|e| e.to_string())?;

        let game_path = s
            .current_project
            .read()
            .await
            .as_ref()
            .map(|p| p.path.clone());
        let font_audit = locust_server::audit_project_fonts(game_path, entries).await;
        let font_suggestions =
            locust_core::font_validation::suggestions_for_font_reports(&font_audit.fonts);

        Ok(serde_json::json!({
            "validation": validation,
            "fonts": font_audit.fonts,
            "font_issues": font_audit.issues,
            "font_limitations": locust_core::font_validation::FONT_COVERAGE_LIMITATION,
            "font_suggestions": font_suggestions,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Export current project translations to a PO or XLIFF file at `path`.
/// Frontend picks the path via the native save dialog.
#[tauri::command]
pub async fn export_translations(
    format: String,
    lang: String,
    path: String,
    file: Option<String>,
    status: Option<Vec<String>>,
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    apply_export_translations(&state.0, format, lang, path, file, status).await
}

async fn apply_export_translations(
    s: &Arc<AppState>,
    format: String,
    lang: String,
    path: String,
    file: Option<String>,
    status: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    let _exclusive = try_project_operation(s)?;
    if s.current_project.read().await.is_none() {
        return Err("no project open".into());
    }
    let scope = locust_core::export::ExportScope::parse(file, status.as_deref())?;
    let entries = scope.entries(&s.db).map_err(|e| e.to_string())?;
    if entries.is_empty() {
        return Err("no entries to export".into());
    }
    let default_source = s.config.read().await.default_source_lang.clone();
    let source =
        s.db.resolve_export_source_lang(&lang, &default_source)
            .map_err(|e| e.to_string())?;
    let body = match format.as_str() {
        "po" => locust_core::export::export_po(&entries, &source, &lang),
        "xliff" => locust_core::export::export_xliff(&entries, &source, &lang),
        other => return Err(format!("unknown export format: {other}")),
    };
    let out = PathBuf::from(&path);
    locust_core::export::check_export_destination(&out, &s.db.path()).map_err(|e| e.to_string())?;
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    std::fs::write(&out, body.as_bytes()).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "path": path,
        "format": format,
        "lang": lang,
        "entries": entries.len(),
        "bytes": body.len(),
    }))
}

/// Import translations from a PO or XLIFF file into the open project DB.
#[tauri::command]
pub async fn import_translations(
    format: String,
    path: String,
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    apply_import_translations(&state.0, format, path).await
}

async fn apply_import_translations(
    s: &Arc<AppState>,
    format: String,
    path: String,
) -> Result<serde_json::Value, String> {
    let s = s.clone();
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if content.trim().is_empty() {
        return Err("import file is empty".into());
    }
    let (updates, pre_skipped) = match format.as_str() {
        "po" => {
            let entries = locust_core::export::import_po(&content).map_err(|e| e.to_string())?;
            locust_core::export::po_entries_for_batch(&entries)
        }
        "xliff" => {
            let units = locust_core::export::import_xliff(&content).map_err(|e| e.to_string())?;
            locust_core::export::xliff_units_for_batch(&units)
        }
        other => return Err(format!("unknown import format: {other}")),
    };
    let attempted = updates.len();
    let exclusive = try_project_operation(&s)?;
    run_owned_project_operation(exclusive, async move {
        if s.current_project.read().await.is_none() {
            return Err("no project open".into());
        }
        let report =
            s.db.save_imported_translations_batch(updates)
                .await
                .map_err(|e| e.to_string())?;
        let (imported, skipped) =
            locust_core::export::import_counts_after_batch(pre_skipped, attempted, report.imported);
        Ok(serde_json::json!({
            "path": path,
            "format": format,
            "imported": imported,
            "skipped": skipped,
            "stale_sources": report.stale_sources,
            "unknown_ids": report.unknown_ids,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Deserialize)]
pub struct InjectParams {
    pub project_path: String,
    pub format_id: String,
    /// Ignored when `direct` is true.
    #[serde(default)]
    pub mode: Option<OutputMode>,
    pub languages: Vec<String>,
    pub output_dir: Option<String>,
    /// CLI `--direct`: in-place inject + recording for patch pack.
    #[serde(default)]
    pub direct: bool,
    /// Opt-in known-capacity preflight, supported only for Replace copies.
    #[serde(default)]
    pub require_binary_slot_fit: bool,
}

/// Unblocking advice attached to a containment failure when recording an
/// injection made through the desktop app. The app cannot know the exact
/// paths the user's shell will use, so it names the shape of the working
/// command rather than a copy-pasteable line.
const INJECT_RECORD_REMEDY: &str =
    "Restore the original game files from the backup listed above (or from a \
     clean copy) first — this engine writes translations into the ORIGINAL \
     tree, and a re-run against the mutated tree writes and records nothing. \
     Then record the injection with the CLI's direct mode: locust inject \
     <game folder> -P <project db> --direct -l <lang> — `locust patch` packs \
     from that recording.";

#[tauri::command]
pub async fn run_inject(
    params: InjectParams,
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    apply_run_inject(params, &state.0).await
}

async fn apply_run_inject(
    params: InjectParams,
    s: &Arc<AppState>,
) -> Result<serde_json::Value, String> {
    // Same guard as CLI/server: empty languages used to return success with
    // zero work and zero recording — a silent no-op that breaks `locust patch`.
    if params.languages.is_empty() {
        return Err(INJECT_EMPTY_LANGUAGES_MESSAGE.to_string());
    }
    if params.require_binary_slot_fit
        && (params.direct
            || params
                .mode
                .as_ref()
                .is_some_and(|mode| *mode != OutputMode::Replace))
    {
        return Err("require_binary_slot_fit is supported only for Replace mode".into());
    }
    let s = s.clone();
    let exclusive = try_project_operation(&s)?;

    if params.direct {
        if s.current_project.read().await.is_none() {
            return Err("no project open".into());
        }
        let game_path = PathBuf::from(&params.project_path);
        let format_id = params.format_id.clone();
        let languages = params.languages.clone();
        let registry = s.format_registry.clone();
        let db = s.db.clone();
        let backup = s.backup_manager.clone();
        let report = tokio::task::spawn_blocking(move || {
            let _exclusive = exclusive;
            locust_core::extraction::inject_direct(
                &registry, &db, &backup, &game_path, &format_id, &languages,
            )
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        return serde_json::to_value(report).map_err(|e| e.to_string());
    }

    run_owned_project_operation(exclusive, async move {
        if s.current_project.read().await.is_none() {
            return Err("no project open".into());
        }
        let mode = params.mode.unwrap_or(OutputMode::Replace);
        let injector = locust_core::extraction::MultiLangInjector::new(
            s.format_registry.clone(),
            s.db.clone(),
            s.backup_manager.clone(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        let languages = params.languages.clone();
        let report = injector
            .inject_with_options(
                &PathBuf::from(&params.project_path),
                &params.format_id,
                locust_core::extraction::MultiLangInjectionOptions {
                    mode,
                    require_binary_slot_fit: params.require_binary_slot_fit,
                },
                params.languages,
                params.output_dir.map(PathBuf::from),
                tx,
            )
            .await
            .map_err(|e| e.to_string())?;

        // Persist what each language's injection wrote — `locust patch` packs
        // exclusively from this recording, so an inject seam that skips it
        // produces projects that can never be packed. Attach outcomes so the UI
        // can refuse a success toast on NothingRecorded / KeptPrevious.
        let outcomes = locust_core::extraction::record_multilang_injection(
            &s.db,
            &report,
            &languages,
            &|_lang| INJECT_RECORD_REMEDY.to_string(),
        )
        .map_err(|e| e.to_string())?;

        let mut body = serde_json::to_value(report).map_err(|e| e.to_string())?;
        if let Some(obj) = body.as_object_mut() {
            obj.insert(
                "outcomes".into(),
                serde_json::to_value(outcomes).map_err(|e| e.to_string())?,
            );
        }
        Ok(body)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Deserialize)]
pub struct RegisterLangParams {
    pub game_path: String,
    pub lang: String,
    pub label: String,
}

/// Register a language in RM MZ multi-lang UI (Iavra / VisuMZ / Map choices).
/// Same as CLI `locust register-lang`. Mutates game files; writes `*.bak-locust`.
#[tauri::command]
pub async fn register_lang(
    params: RegisterLangParams,
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    let game_path = PathBuf::from(params.game_path.trim());
    if params.game_path.trim().is_empty() || !game_path.is_dir() {
        return Err(format!(
            "game_path must be an existing directory (got {:?})",
            params.game_path
        ));
    }
    let lang = params.lang;
    let label = params.label;
    let guard = try_project_operation(&state.0)?;
    let db = state
        .0
        .current_project
        .read()
        .await
        .as_ref()
        .map(|_| state.0.db.clone());
    let report = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        locust_formats::rpgmaker_lang::register_language_with_db(
            &game_path,
            &lang,
            &label,
            db.as_deref(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    serde_json::to_value(report).map_err(|e| e.to_string())
}

// ─── xAI device-code login ──────────────────────────────────────────────────

#[tauri::command]
pub async fn xai_auth_start(
    state: State<'_, AppStateWrapper>,
) -> Result<XaiAuthStartResponse, String> {
    start_xai_device_login(&state.0).await
}

#[tauri::command]
pub async fn xai_auth_poll(
    handle: String,
    state: State<'_, AppStateWrapper>,
) -> Result<XaiAuthPollResponse, String> {
    poll_xai_device_login(&state.0, &handle).await
}

// ─── Config ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_config(state: State<'_, AppStateWrapper>) -> Result<serde_json::Value, String> {
    Ok(locust_server::public_app_config(&state.0).await)
}

#[tauri::command]
pub async fn save_config(
    partial: serde_json::Value,
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    update_app_config(&state.0, partial)
        .await
        .map_err(|(_, message)| message)
}

// ─── Backups & Glossary ─────────────────────────────────────────────────────

#[tauri::command]
pub fn get_backups(
    state: State<AppStateWrapper>,
    report: Option<bool>,
) -> Result<serde_json::Value, String> {
    backup_response(&state.0.backup_manager, report.unwrap_or(false))
}

fn backup_response(
    manager: &locust_core::backup::BackupManager,
    report: bool,
) -> Result<serde_json::Value, String> {
    if report {
        let listing = manager.list_backups_report().map_err(|e| e.to_string())?;
        let unreadable: Vec<_> = listing
            .unreadable
            .into_iter()
            .map(|(id, error)| serde_json::json!({"id": id, "error": error}))
            .collect();
        Ok(serde_json::json!({"entries": listing.entries, "unreadable": unreadable}))
    } else {
        serde_json::to_value(manager.list_backups().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub async fn get_glossary(
    lang_pair: String,
    state: State<'_, AppStateWrapper>,
) -> Result<Vec<GlossaryEntry>, String> {
    apply_get_glossary(&state.0, &lang_pair).await
}

async fn apply_get_glossary(s: &AppState, lang_pair: &str) -> Result<Vec<GlossaryEntry>, String> {
    if s.current_project.read().await.is_none() {
        return Err("no project open".into());
    }
    s.glossary.get_all(lang_pair).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_glossary_entry(
    entry: GlossaryEntry,
    state: State<'_, AppStateWrapper>,
) -> Result<(), String> {
    apply_add_glossary_entry(&state.0, &entry).await
}

async fn apply_add_glossary_entry(s: &AppState, entry: &GlossaryEntry) -> Result<(), String> {
    if s.current_project.read().await.is_none() {
        return Err("no project open".into());
    }
    s.glossary.add_entry(entry).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn godot_get_formats_transport_parity() {
        let state = locust_server::create_test_state();
        let list = super::format_list(&state);
        let godot = list
            .iter()
            .find(|format| format.id == "godot-dialogic")
            .unwrap();
        assert_eq!(godot.extensions, ["pck"]);
        assert_eq!(
            godot.supported_modes,
            [locust_core::models::OutputMode::Replace]
        );
        assert_eq!(
            godot.stability,
            locust_core::extraction::FormatStability::Experimental
        );
    }

    #[tokio::test]
    async fn godot_native_open_saved_file_and_inject_transport_parity() {
        use std::io::Cursor;
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        let game = shared.join("game.pck");
        let mut header = vec![0; 100];
        header[..4].copy_from_slice(b"GDPC");
        header[4..8].copy_from_slice(&2u32.to_le_bytes());
        header[8..12].copy_from_slice(&4u32.to_le_bytes());
        let mut source = Cursor::new(header);
        let index = locust_formats::godot_pck::read_index(&mut source).unwrap();
        let mut original = Vec::new();
        locust_formats::godot_pck::rebuild(
            &mut source,
            &index,
            &[],
            &[("res://main.dtl", b"Hello\n")],
            &mut original,
        )
        .unwrap();
        std::fs::write(&game, &original).unwrap();
        std::fs::write(shared.join("index.html"), "<p>Foreign game</p>").unwrap();
        let state = locust_server::create_test_state();
        let selected = game.to_string_lossy().into_owned();
        let opened = super::apply_open_project(&state, selected.clone(), None, false)
            .await
            .unwrap();
        assert_eq!(opened.format_id, "godot-dialogic");
        assert_eq!(opened.supported_modes, [OutputMode::Replace]);
        assert!(locust_core::database::paths_identical(
            Path::new(&opened.project_path),
            &game
        ));
        let saved = super::apply_open_project_db(
            &state,
            opened.database_path,
            selected.clone(),
            "godot-dialogic".into(),
        )
        .await
        .unwrap();
        assert!(locust_core::database::paths_identical(
            Path::new(&saved.project_path),
            &game
        ));
        let mut rows = state.db.get_entries(&Default::default()).unwrap();
        rows[0].translation = Some("Hola, una traduccion mas larga".into());
        rows[0].status = StringStatus::Approved;
        state.db.save_entries(&rows).unwrap();
        for direct in [false, true] {
            let params = serde_json::from_value(serde_json::json!({
                "project_path": selected, "format_id": "godot-dialogic",
                "languages": ["es"], "direct": direct, "mode": "replace",
                "output_dir": temp.path().join("copies")
            }))
            .unwrap();
            let report = super::apply_run_inject(params, &state).await.unwrap();
            assert_eq!(report["languages_processed"], serde_json::json!(["es"]));
            let recorded = state.db.get_injection(Some("es")).unwrap().unwrap();
            assert_eq!(recorded.files.len(), 1);
            assert_eq!(recorded.files[0].rel, "game.pck");
            if direct {
                assert!(locust_core::database::paths_identical(
                    &recorded.root,
                    &shared
                ));
            } else {
                assert_eq!(std::fs::read(&game).unwrap(), original);
                assert!(!recorded.root.join("index.html").exists());
            }
        }
    }

    struct CapacitySentinel;

    impl locust_core::extraction::FormatPlugin for CapacitySentinel {
        fn id(&self) -> &str {
            "capacity-sentinel"
        }
        fn name(&self) -> &str {
            "Capacity sentinel"
        }
        fn supported_extensions(&self) -> &[&str] {
            &["bin"]
        }
        fn extract(&self, _: &Path) -> locust_core::error::Result<Vec<StringEntry>> {
            unreachable!()
        }
        fn inject(
            &self,
            path: &Path,
            _: &[StringEntry],
        ) -> locust_core::error::Result<locust_core::extraction::InjectionReport> {
            let file = path.join("story.bin");
            std::fs::write(&file, b"partial output")?;
            Ok(locust_core::extraction::InjectionReport {
                files_modified: 1,
                strings_written: 1,
                strings_skipped: 1,
                files_written: vec![file],
                skip_reasons: Default::default(),
                warnings: vec![],
            })
        }
    }

    #[tokio::test]
    async fn binary_slot_fit_run_inject_transport_contract() {
        for (flag, target, direct, mode, refused) in [
            (Some(true), "oversize", false, None, true),
            (Some(true), "fit", false, None, false),
            (Some(true), "fit", false, Some("replace"), false),
            (None, "oversize", false, None, false),
            (Some(false), "oversize", false, None, false),
            (Some(true), "fit", true, None, true),
            (Some(true), "fit", false, Some("add"), true),
            (Some(false), "oversize", true, Some("add"), false),
            (None, "oversize", true, Some("add"), false),
        ] {
            let mut state = locust_server::create_test_state();
            let root = state.config_path.parent().unwrap().to_path_buf();
            let game = root.join("game");
            let output = root.join("output");
            let backups = root.join("backups");
            std::fs::create_dir(&game).unwrap();
            std::fs::write(game.join("story.bin"), b"original").unwrap();
            let mut registry = locust_core::extraction::FormatRegistry::new();
            registry.register(Box::new(CapacitySentinel));
            let mutable = Arc::get_mut(&mut state).unwrap();
            mutable.format_registry = Arc::new(registry);
            mutable.backup_manager =
                Arc::new(locust_core::backup::BackupManager::new(backups.clone()));
            pivot_project(&state).await;
            let mut row = StringEntry::new("slot", "abc", "story.bin".into());
            row.translation = Some(target.into());
            row.status = StringStatus::Translated;
            row.metadata
                .insert("binary_slot".into(), serde_json::json!("utf8"));
            state.db.save_entries(&[row]).unwrap();
            let before = pivot_snapshot(&state);
            let mut request = serde_json::json!({
                "project_path": game, "format_id": "capacity-sentinel",
                "languages": ["es"], "output_dir": output, "direct": direct
            });
            if let Some(flag) = flag {
                request["require_binary_slot_fit"] = flag.into();
            }
            if let Some(mode) = mode {
                request["mode"] = mode.into();
            }
            // run_inject delegates directly here; constructing Tauri State needs
            // the unavailable test feature (StateManager constructors are private).
            let result =
                apply_run_inject(serde_json::from_value(request.clone()).unwrap(), &state).await;
            if refused {
                let error = result.expect_err(&request.to_string());
                assert_eq!(
                    error,
                    if direct || mode == Some("add") {
                        "require_binary_slot_fit is supported only for Replace mode"
                    } else {
                        "injection error: 1 translation(s) exceed known binary slot capacity; Replace is refused before backup or output; run validation and resolve the capacity issues"
                    }
                );
                assert!(!backups.exists());
                assert!(!output.exists());
                assert!(state.db.get_injection(Some("es")).unwrap().is_none());
                assert_eq!(std::fs::read_dir(&game).unwrap().count(), 1);
            } else {
                let body = result.unwrap();
                assert!(backups.exists());
                assert!(state.db.get_injection(Some("es")).unwrap().is_some());
                if !direct {
                    assert_eq!(body["reports"]["es"]["strings_skipped"], 1);
                    assert!(output.exists());
                }
            }
            assert_eq!(pivot_snapshot(&state), before);
            assert_eq!(
                std::fs::read(game.join("story.bin")).unwrap(),
                if direct && !refused {
                    b"partial output".as_slice()
                } else {
                    b"original".as_slice()
                }
            );
        }
    }

    fn pivot_ready(id: &str) -> StringEntry {
        let mut row = StringEntry::new(id, "日本語 [name]", "script.rpy".into());
        row.translation = Some("English [name]".into());
        row.status = StringStatus::Translated;
        row
    }

    async fn pivot_project(state: &AppState) {
        *state.current_project.write().await = Some(
            serde_json::from_value(serde_json::json!({
                "path":"synthetic-game", "format_id":"renpy", "name":"Fixture"
            }))
            .unwrap(),
        );
    }

    fn pivot_snapshot(state: &AppState) -> serde_json::Value {
        serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn pivot_dispatch_strict_refuses_incomplete_stale_and_invalid_controls() {
        for case in [
            "missing",
            "empty",
            "blank",
            "pending",
            "error",
            "stale",
            "control",
            "physical-control",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let state = locust_server::create_test_state();
            pivot_project(&state).await;
            let mut invalid = pivot_ready("z");
            let reason = match case {
                "missing" => {
                    invalid.translation = None;
                    "missing or blank translation"
                }
                "empty" => {
                    invalid.translation = Some(String::new());
                    "missing or blank translation"
                }
                "blank" => {
                    invalid.translation = Some("\t\u{a0}\u{2003}\u{3000}".into());
                    "missing or blank translation"
                }
                "pending" => {
                    invalid.status = StringStatus::Pending;
                    "not ready"
                }
                "error" => {
                    invalid.status = StringStatus::Error;
                    "not ready"
                }
                "stale" => {
                    invalid.metadata.insert(
                        locust_core::models::STALE_TRANSLATION_METADATA_KEY.into(),
                        serde_json::json!({
                            "version": 1,
                            "translated_source_sha256": "0".repeat(64),
                            "current_source_sha256": invalid.source_hash(),
                        }),
                    );
                    "stale translation"
                }
                "physical-control" => {
                    invalid.source = "English".into();
                    invalid.translation = Some("Spanish".into());
                    invalid.metadata.insert(
                        "locust_injection_source".into(),
                        serde_json::json!("日本語 [name]"),
                    );
                    "changed protected controls"
                }
                _ => {
                    invalid.translation = Some("English".into());
                    "changed protected controls"
                }
            };
            state.db.save_entries(&[pivot_ready("a"), invalid]).unwrap();
            let before = pivot_snapshot(&state);
            let error = apply_run_pivot(
                &state,
                temp.path().join("out.db").to_string_lossy().into_owned(),
                Some(true),
            )
            .await
            .expect_err(case);
            assert!(error.contains(reason), "{case}: {error}");
            assert_eq!(
                std::fs::read_dir(temp.path()).unwrap().count(),
                0,
                "no output or sidecars"
            );
            assert_eq!(pivot_snapshot(&state), before);
        }
    }

    #[tokio::test]
    async fn pivot_dispatch_complete_preserves_all_rows_and_physical_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let state = locust_server::create_test_state();
        pivot_project(&state).await;
        let mut slot = pivot_ready("a");
        slot.source = "English".into();
        slot.translation = Some("Spanish".into());
        slot.metadata
            .insert("binary_slot".into(), serde_json::json!("utf8"));
        slot.metadata.insert(
            "locust_injection_source".into(),
            serde_json::json!("日本語文"),
        );
        slot.metadata.insert(
            "locust_injection_capacity".into(),
            serde_json::json!({"encoding":"utf8", "bytes":12}),
        );
        slot.context = Some("speaker".into());
        slot.tags = vec!["dialogue".into()];
        let mut reviewed = pivot_ready("b");
        reviewed.status = StringStatus::Reviewed;
        let mut approved = pivot_ready("c");
        approved.status = StringStatus::Approved;
        state.db.save_entries(&[slot, reviewed, approved]).unwrap();
        let before = pivot_snapshot(&state);
        let output = temp.path().join("complete.db");
        let result = apply_run_pivot(&state, output.to_string_lossy().into_owned(), Some(true))
            .await
            .unwrap();
        assert_eq!(result.entries, 3);
        assert_eq!(result.database_path, output.to_string_lossy());
        let pivot = locust_core::database::Database::open(&output).unwrap();
        for row in pivot.get_entries(&EntryFilter::default()).unwrap() {
            let original = state.db.get_entry(&row.id).unwrap().unwrap();
            assert_eq!(Some(row.source.as_str()), original.translation.as_deref());
            assert_eq!(
                row.injection_source().unwrap(),
                original.injection_source().unwrap()
            );
            assert_eq!(
                row.injection_capacity().unwrap(),
                original.injection_capacity().unwrap()
            );
            assert_eq!(row.status, StringStatus::Pending);
            assert!(row.translation.is_none());
            assert_eq!(row.context, original.context);
            assert_eq!(row.tags, original.tags);
        }
        assert_eq!(pivot_snapshot(&state), before);
    }

    #[tokio::test]
    async fn pivot_dispatch_legacy_absent_and_false_keep_partial_behavior() {
        for strict in [None, Some(false)] {
            let temp = tempfile::tempdir().unwrap();
            let state = locust_server::create_test_state();
            pivot_project(&state).await;
            let mut retained = pivot_ready("retained");
            retained.status = StringStatus::Pending;
            let mut blank = pivot_ready("blank");
            blank.translation = Some("  ".into());
            state
                .db
                .save_entries(&[
                    pivot_ready("ready"),
                    retained,
                    blank,
                    StringEntry::new("missing", "Source", "script.rpy".into()),
                ])
                .unwrap();
            let before = pivot_snapshot(&state);
            let result = apply_run_pivot(
                &state,
                temp.path()
                    .join("partial.db")
                    .to_string_lossy()
                    .into_owned(),
                strict,
            )
            .await
            .unwrap();
            assert_eq!(result.entries, 2);
            assert_eq!(pivot_snapshot(&state), before);
        }
    }

    #[tokio::test]
    async fn pivot_dispatch_keeps_persisted_source_bytes_and_refuses_source_as_output() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.db");
        let output_dir = tempfile::tempdir().unwrap();
        // Close the seed writer so its WAL is checkpointed before measuring bytes.
        let seed = locust_core::database::Database::open(&source).unwrap();
        seed.save_entries(&[
            pivot_ready("a"),
            StringEntry::new("z", "Missing", "script.rpy".into()),
        ])
        .unwrap();
        drop(seed);
        let state = locust_server::create_test_state_with_db(&source);
        pivot_project(&state).await;
        let before = std::fs::read(&source).unwrap();
        let rows_before = pivot_snapshot(&state);
        for output in [output_dir.path().join("refused.db"), source.clone()] {
            let error = apply_run_pivot(&state, output.to_string_lossy().into_owned(), Some(true))
                .await
                .unwrap_err();
            assert!(
                error.contains(if output == source {
                    "refusing to overwrite"
                } else {
                    "missing or blank translation"
                }),
                "{error}"
            );
        }
        assert_eq!(std::fs::read_dir(output_dir.path()).unwrap().count(), 0);
        assert_eq!(pivot_snapshot(&state), rows_before);
        assert_eq!(std::fs::read(&source).unwrap(), before);
        drop(state);
        assert_eq!(std::fs::read(&source).unwrap(), before);
    }

    #[tokio::test]
    async fn pivot_dispatch_preserves_admission_and_overwrite_guards() {
        let temp = tempfile::tempdir().unwrap();
        let state = locust_server::create_test_state();
        state.db.save_entries(&[pivot_ready("a")]).unwrap();
        let output = temp.path().join("out.db").to_string_lossy().into_owned();
        assert_eq!(
            apply_run_pivot(&state, output.clone(), Some(true))
                .await
                .unwrap_err(),
            "no project open"
        );
        pivot_project(&state).await;
        let guard = try_project_operation(&state).unwrap();
        assert_eq!(
            apply_run_pivot(&state, output.clone(), Some(true))
                .await
                .unwrap_err(),
            locust_server::PROJECT_BUSY_MESSAGE
        );
        drop(guard);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
        std::fs::write(&output, b"keep output").unwrap();
        assert!(apply_run_pivot(&state, output.clone(), Some(true))
            .await
            .unwrap_err()
            .contains("refusing to overwrite"));
        assert_eq!(std::fs::read(&output).unwrap(), b"keep output");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    use super::*;

    #[tokio::test]
    async fn strings_manual_save_translation_and_status_records_one_final_revision() {
        let db = locust_core::database::Database::open_in_memory().unwrap();
        db.save_entries(&[StringEntry::new(
            "atomic",
            "Hello",
            PathBuf::from("story.html"),
        )])
        .unwrap();
        let saved = apply_string_patch(
            &db,
            "atomic",
            serde_json::from_value(serde_json::json!({
                "translation": "Hola", "status": "reviewed", "expected_translation": null
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(saved.translation.as_deref(), Some("Hola"));
        assert_eq!(saved.status, StringStatus::Reviewed);
        assert_eq!(saved.provider_used.as_deref(), Some("manual"));
        assert!(saved.translated_at.is_some());
        assert!(saved.reviewed_at.is_some());
        let history = db.get_string_history("atomic", 50).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].old_status.as_deref(), Some("pending"));
        assert_eq!(history[0].new_status.as_deref(), Some("reviewed"));
        assert_eq!(history[0].new_translation.as_deref(), Some("Hola"));
    }

    #[tokio::test]
    async fn manual_save_conflict_preserves_row_and_maps_the_stable_message() {
        let db = locust_core::database::Database::open_in_memory().unwrap();
        let mut row = StringEntry::new("row", "Source", PathBuf::from("story.html"));
        row.translation = Some("V1".into());
        row.status = StringStatus::Approved;
        db.save_entries(&[row]).unwrap();
        for expected in [serde_json::json!("V0"), serde_json::Value::Null] {
            let data = serde_json::from_value(serde_json::json!({
                "translation": "A draft", "expected_translation": expected, "status": "pending"
            }))
            .unwrap();
            assert_eq!(
                apply_string_patch(&db, "row", data).await.unwrap_err(),
                "translation changed since it was loaded"
            );
            let saved = db.get_entry("row").unwrap().unwrap();
            assert_eq!(saved.translation.as_deref(), Some("V1"));
            assert_eq!(saved.status, StringStatus::Approved);
        }
        for body in [
            serde_json::json!({"translation":"V2", "expected_translation":"V1"}),
            serde_json::json!({"translation":"legacy"}),
            serde_json::json!({"status":"reviewed"}),
        ] {
            apply_string_patch(&db, "row", serde_json::from_value(body).unwrap())
                .await
                .unwrap();
        }
        let saved = db.get_entry("row").unwrap().unwrap();
        assert_eq!(saved.translation.as_deref(), Some("legacy"));
        assert_eq!(saved.status, StringStatus::Reviewed);
    }

    #[test]
    fn backup_response_supports_report_and_legacy_modes() {
        let state = locust_server::create_test_state();
        let root = state.config_path.parent().unwrap();
        std::fs::create_dir_all(root).unwrap();
        let manager = locust_core::backup::BackupManager::new(root.join("backups"));
        assert_eq!(
            backup_response(&manager, false).unwrap(),
            serde_json::json!([])
        );
        assert_eq!(
            backup_response(&manager, true).unwrap(),
            serde_json::json!({"entries": [], "unreadable": []})
        );
        let game = root.join("game");
        std::fs::create_dir(&game).unwrap();
        std::fs::write(game.join("story.txt"), b"original").unwrap();
        let healthy = manager.create_backup(&game).unwrap();
        let damaged = manager.root().join("damaged");
        std::fs::create_dir(&damaged).unwrap();
        std::fs::write(damaged.join("manifest.json"), b"{").unwrap();
        let report = backup_response(&manager, true).unwrap();
        assert_eq!(report["entries"][0]["id"], healthy.id);
        assert_eq!(report["unreadable"][0]["id"], "damaged");
        assert!(report["unreadable"][0]["error"]
            .as_str()
            .is_some_and(|e| !e.is_empty()));
        assert_eq!(backup_response(&manager, false).unwrap(), report["entries"]);
        std::fs::write(healthy.path.join("manifest.json"), b"{").unwrap();
        let all_damaged = backup_response(&manager, true).unwrap();
        assert_eq!(all_damaged["entries"], serde_json::json!([]));
        assert_eq!(all_damaged["unreadable"].as_array().unwrap().len(), 2);
        assert!(
            backup_response(&manager, false).is_err(),
            "legacy all-damaged failure stays unchanged"
        );
        let fatal_root = root.join("not-a-directory");
        std::fs::write(&fatal_root, b"file").unwrap();
        let failed = locust_core::backup::BackupManager::new(fatal_root);
        assert!(
            backup_response(&failed, true).is_err(),
            "fatal errors must not become empty reports"
        );
    }

    async fn direct_resume_fixture() -> (Arc<AppState>, PathBuf, String, String) {
        let mut state = locust_server::create_test_state();
        let root = state.config_path.parent().unwrap();
        let game = root.join("game");
        let backups = root.join("backups");
        Arc::get_mut(&mut state).unwrap().backup_manager =
            Arc::new(locust_core::backup::BackupManager::new(backups));
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("story.html"), "<p>Original source</p>").unwrap();
        let opened = apply_open_project(
            &state,
            game.to_string_lossy().into_owned(),
            Some("html-game".into()),
            false,
        )
        .await
        .unwrap();
        let mut row = state
            .db
            .get_entries(&EntryFilter::default())
            .unwrap()
            .remove(0);
        let id = row.id.clone();
        row.translation = Some("Translated".into());
        row.status = StringStatus::Approved;
        row.provider_used = Some("fixture".into());
        state.db.save_entries(&[row]).unwrap();
        locust_core::extraction::inject_direct(
            &state.format_registry,
            &state.db,
            &state.backup_manager,
            &game,
            "html-game",
            &["en".into()],
        )
        .unwrap();
        (state, game, opened.database_path, id)
    }

    #[tokio::test]
    async fn verified_resume_adapter_keeps_saved_approvals_and_zero_merge_counters() {
        let (state, game, database, id) = direct_resume_fixture().await;
        let preflight =
            project::preflight_project_open(&state.format_registry, &game, None).unwrap();
        assert_eq!(
            serde_json::to_value(preflight).unwrap()["kind"],
            "resume_available"
        );
        let result = apply_saved_project(
            &state,
            database.clone(),
            game.to_string_lossy().into_owned(),
            "html-game".into(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            (
                result.added,
                result.updated,
                result.stale_source_reset,
                result.removed,
                result.preserved_translations
            ),
            (0, 0, 0, 0, 0)
        );
        let row = state.db.get_entry(&id).unwrap().unwrap();
        assert_eq!(row.source, "Original source");
        assert_eq!(row.translation.as_deref(), Some("Translated"));
        assert_eq!(row.status, StringStatus::Approved);
        assert_eq!(row.provider_used.as_deref(), Some("fixture"));
        assert_eq!(result.database_path, database);
        assert_eq!(
            state.current_project.read().await.as_ref().unwrap().path,
            game
        );
        assert!(state.config_path.is_file());
    }

    #[tokio::test]
    async fn verified_resume_adapter_rechecks_drift_and_admission_without_switching_project() {
        let (state, game, database, id) = direct_resume_fixture().await;
        let before_project =
            serde_json::to_value(state.current_project.read().await.clone()).unwrap();
        let before_config = serde_json::to_value(state.config.read().await.clone()).unwrap();
        let guard = try_project_operation(&state).unwrap();
        assert_eq!(
            apply_saved_project(
                &state,
                database.clone(),
                game.to_string_lossy().into_owned(),
                "html-game".into(),
                true,
            )
            .await
            .unwrap_err(),
            locust_server::PROJECT_BUSY_MESSAGE
        );
        drop(guard);
        std::fs::write(game.join("story.html"), "<p>Drift</p>").unwrap();
        assert!(apply_saved_project(
            &state,
            database.clone(),
            game.to_string_lossy().into_owned(),
            "html-game".into(),
            true,
        )
        .await
        .is_err());
        assert_eq!(state.db.path(), PathBuf::from(database));
        assert_eq!(
            state.db.get_entry(&id).unwrap().unwrap().source,
            "Original source"
        );
        assert_eq!(
            state.db.get_entry(&id).unwrap().unwrap().status,
            StringStatus::Approved
        );
        assert_eq!(
            serde_json::to_value(state.current_project.read().await.clone()).unwrap(),
            before_project
        );
        assert_eq!(
            serde_json::to_value(state.config.read().await.clone()).unwrap(),
            before_config
        );
    }

    #[test]
    fn string_facets_and_pivot_json_keep_contract_field_names() {
        let facets = serde_json::to_value(StringFacets {
            file_paths: vec!["data/Actors.json".into()],
            tags: vec!["dialogue".into()],
        })
        .unwrap();
        assert_eq!(
            facets.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["file_paths", "tags"]
        );

        let pivoted = serde_json::to_value(PivotResult {
            database_path: "/tmp/out.locust.db".into(),
            entries: 3,
        })
        .unwrap();
        assert_eq!(
            pivoted.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["database_path", "entries"]
        );
    }

    #[test]
    fn translate_params_deserializes_fallback_provider_ids() {
        let params: TranslateParams = serde_json::from_value(serde_json::json!({
            "provider_id": "always-error",
            "fallback_provider_ids": ["mock"],
            "options": {
                "source_lang": "en",
                "target_lang": "es",
                "batch_size": 10,
                "max_concurrent": 1,
                "cost_limit_usd": null,
                "game_context": null,
                "use_glossary": false,
                "use_memory": false,
                "skip_approved": true
            }
        }))
        .unwrap();
        assert_eq!(params.provider_id, "always-error");
        assert_eq!(params.fallback_provider_ids, Some(vec!["mock".to_string()]));
    }

    #[test]
    fn provider_translation_defaults_wire_options() {
        for (provider, expected) in [
            ("google", (200, 4, Some(40_000))),
            ("antigravity", (100, 4, Some(20_000))),
            ("grok", (10, 10, Some(6000))),
            ("grok-sub", (10, 10, Some(6000))),
            ("mock", (40, 1, Some(6000))),
        ] {
            for explicit in [false, true] {
                let mut options = serde_json::json!({
                    "source_lang": "ja", "target_lang": "en",
                    "use_glossary": false, "use_memory": false, "skip_approved": true
                });
                if explicit {
                    options["batch_size"] = 7.into();
                    options["max_concurrent"] = 2.into();
                    options["max_batch_tokens"] = 900.into();
                }
                let params: TranslateParams = serde_json::from_value(
                    serde_json::json!({"provider_id": provider, "options": options}),
                )
                .unwrap();
                let resolved = params.options.resolve(&params.provider_id, None);
                assert_eq!(
                    (
                        resolved.batch_size,
                        resolved.max_concurrent,
                        resolved.max_batch_tokens
                    ),
                    if explicit {
                        (7, 2, Some(900))
                    } else {
                        expected
                    },
                    "{provider}"
                );
            }
        }
    }
    #[test]
    fn provider_translation_defaults_accept_omitted_command_fields() {
        let params = serde_json::from_value::<TranslateParams>(serde_json::json!({
            "provider_id": "google",
            "options": {
                "source_lang": "ja", "target_lang": "en",
                "use_glossary": true, "use_memory": true, "skip_approved": true
            }
        }));
        assert!(params.is_ok(), "{}", params.err().unwrap());
    }

    #[tokio::test]
    async fn xai_auth_poll_unknown_handle_is_clean_error() {
        let state = locust_server::create_test_state();
        let err = poll_xai_device_login(&state, "missing-handle")
            .await
            .expect_err("unknown handle must error");
        assert!(
            err.to_lowercase().contains("not found"),
            "clean error, not a panic/500: {err}"
        );
    }

    #[test]
    fn xai_auth_shapes_match_http_contract() {
        let start = serde_json::to_value(XaiAuthStartResponse {
            handle: "h1".into(),
            user_code: "ABCD".into(),
            verification_uri: "https://auth.x.ai/device".into(),
            expires_in_secs: 900,
        })
        .unwrap();
        let obj = start.as_object().unwrap();
        assert!(obj.contains_key("handle"));
        assert!(obj.contains_key("user_code"));
        assert!(obj.contains_key("verification_uri"));
        assert!(obj.contains_key("expires_in_secs"));
        assert_eq!(obj.len(), 4);

        for status in [
            locust_server::XaiAuthStatus::Pending,
            locust_server::XaiAuthStatus::Complete,
            locust_server::XaiAuthStatus::Denied,
            locust_server::XaiAuthStatus::Expired,
        ] {
            let v = serde_json::to_value(XaiAuthPollResponse { status }).unwrap();
            let s = v["status"].as_str().unwrap();
            assert!(
                matches!(s, "pending" | "complete" | "denied" | "expired"),
                "{s}"
            );
            assert_eq!(s, s.to_lowercase());
        }
    }

    #[tokio::test]
    async fn xai_auth_poll_respects_interval_without_upstream() {
        use locust_providers::xai_oauth::DeviceCode;
        use locust_server::{XaiAuthStatus, XaiPendingAuth};
        use std::sync::Arc;
        use std::time::Instant;

        let state = locust_server::create_test_state();
        *state.xai_token_url.write().await = Some("http://127.0.0.1:1/oauth2/token".into());
        let handle = "interval-handle".to_string();
        state.xai_pending.insert(
            handle.clone(),
            XaiPendingAuth {
                device: Arc::new(DeviceCode::from_parts(
                    "raw-device-must-not-leak",
                    "CODE",
                    "https://auth.x.ai/device",
                    60,
                    900,
                    u64::MAX,
                )),
                last_poll: Some(Instant::now()),
            },
        );
        let resp = poll_xai_device_login(&state, &handle).await.unwrap();
        assert_eq!(resp.status, XaiAuthStatus::Pending);
    }

    fn write_tauri_locust_db(rows: &[StringEntry]) -> (tempfile::TempDir, PathBuf) {
        let path_guard = tempfile::Builder::new()
            .prefix("locust_opendb_tauri_")
            .tempdir()
            .unwrap();
        let path = path_guard.path().join("fixture.locust.db");
        let db = locust_core::database::Database::open(&path).unwrap();
        db.save_entries(rows).unwrap();
        drop(db);
        (path_guard, path)
    }

    fn translated_entry(id: &str, source: &str, translation: &str) -> StringEntry {
        let mut e = StringEntry::new(id, source, PathBuf::from("data/a.json"));
        e.translation = Some(translation.into());
        e.status = StringStatus::Translated;
        e
    }

    #[tokio::test]
    async fn validation_requires_an_open_project() {
        let state = locust_server::create_test_state();
        state
            .db
            .save_entries(&[translated_entry("leftover", "Hello", "Hola")])
            .unwrap();
        let before = serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap();
        let issues_before =
            serde_json::to_value(state.db.get_validation_issues(None).unwrap()).unwrap();

        assert_eq!(
            apply_run_validation(&state).await.unwrap_err(),
            "no project open"
        );
        assert_eq!(
            serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap(),
            before
        );
        assert_eq!(
            serde_json::to_value(state.db.get_validation_issues(None).unwrap()).unwrap(),
            issues_before
        );
    }

    #[tokio::test]
    async fn export_requires_an_open_project() {
        let state = locust_server::create_test_state();
        state
            .db
            .save_entries(&[translated_entry("leftover", "Hello", "Hola")])
            .unwrap();
        let before = serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap();
        let output = std::env::temp_dir().join(format!("locust_export_{}", uuid::Uuid::new_v4()));

        for format in ["po", "xliff"] {
            let file = output.with_extension(format);
            assert_eq!(
                apply_export_translations(
                    &state,
                    format.into(),
                    "es".into(),
                    file.to_string_lossy().into_owned(),
                    None,
                    None,
                )
                .await
                .unwrap_err(),
                "no project open"
            );
            assert!(!file.exists());
        }
        assert_eq!(
            serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn export_preserves_database_and_sidecars() {
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_export_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let project = dir.join("project.locust.db");
        let state = locust_server::create_test_state_with_db(&project);
        *state.current_project.write().await = Some(ProjectInfo::default());
        state
            .db
            .save_entries(&[translated_entry("hello", "Hello", "Hola")])
            .unwrap();
        let before_rows =
            serde_json::to_value(state.db.get_entries(&EntryFilter::default()).unwrap()).unwrap();
        let before_bytes = std::fs::read(&project).unwrap();

        for format in ["po", "xliff"] {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let output = dir.join(format!("project.locust.db{suffix}"));
                let before_output = output.exists().then(|| std::fs::read(&output).unwrap());
                let err = apply_export_translations(
                    &state,
                    format.into(),
                    "es".into(),
                    output.to_string_lossy().into_owned(),
                    None,
                    None,
                )
                .await
                .unwrap_err();
                assert!(err.contains("choose a different file"), "{err}");
                assert_eq!(std::fs::read(&project).unwrap(), before_bytes);
                assert_eq!(
                    output.exists().then(|| std::fs::read(&output).unwrap()),
                    before_output
                );
            }
        }
        drop(state);
        let reopened = locust_core::database::Database::open(&project).unwrap();
        assert_eq!(
            serde_json::to_value(reopened.get_entries(&EntryFilter::default()).unwrap()).unwrap(),
            before_rows
        );
        drop(reopened);
        std::fs::remove_file(&project).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[tokio::test]
    async fn export_allows_confirmed_catalog_replacement() {
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_export_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let project = dir.join("project.locust.db");
        let state = locust_server::create_test_state_with_db(&project);
        *state.current_project.write().await = Some(ProjectInfo::default());
        state
            .db
            .save_entries(&[translated_entry("hello", "Hello", "Hola")])
            .unwrap();
        let output = dir.join("catalog.po");
        std::fs::write(&output, b"previous catalog").unwrap();

        let result = apply_export_translations(
            &state,
            "po".into(),
            "es".into(),
            output.to_string_lossy().into_owned(),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result["entries"], 1);
        let content = std::fs::read_to_string(&output).unwrap();
        let entries = locust_core::export::import_po(&content).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].translation, "Hola");
        drop(state);
        std::fs::remove_file(&output).unwrap();
        std::fs::remove_file(&project).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    async fn export_scope_state() -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let state = locust_server::create_test_state_with_db(&dir.path().join("project.db"));
        *state.current_project.write().await = Some(ProjectInfo::default());
        state.config.write().await.default_source_lang = "ja".into();
        let rows = [
            ("z", "story/a.rpy", StringStatus::Translated),
            ("b", "story/a.rpy", StringStatus::Reviewed),
            ("a", "story/a.rpy", StringStatus::Pending),
            ("c", "story/a.rpy", StringStatus::Approved),
            ("d", "story/a.rpy", StringStatus::Error),
            ("e", "story/b.rpy", StringStatus::Translated),
        ]
        .into_iter()
        .map(|(id, file, status)| {
            let mut row = translated_entry(id, &format!("Source {id}"), &format!("Destino {id}"));
            row.file_path = file.into();
            row.status = status;
            row
        })
        .collect::<Vec<_>>();
        state.db.save_entries(&rows).unwrap();
        (dir, state)
    }

    // Exercise the real HTTP route alongside the native command without a
    // second HTTP client dependency. Test queries contain only ASCII URL bytes.
    async fn http_export(base: &str, format: &str, query: &str) -> (u16, Vec<u8>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let address = base.strip_prefix("http://").unwrap();
        let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
        connection.write_all(format!("GET /api/export/{format}?lang=es{query} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut response = Vec::new();
        connection.read_to_end(&mut response).await.unwrap();
        let split = response
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        let headers = std::str::from_utf8(&response[..split]).unwrap();
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, response[split + 4..].to_vec())
    }

    #[tokio::test]
    async fn export_scopes_match_http_bytes_order_and_legacy_unfiltered_output() {
        let (dir, state) = export_scope_state().await;
        let all = state.db.get_entries(&EntryFilter::default()).unwrap();
        let (base, server) = locust_server::start_test_server(state.clone()).await;
        for (file, statuses, query, ids) in [
            (None, None, "", vec!["a", "b", "c", "d", "e", "z"]),
            (
                Some("story/a.rpy"),
                None,
                "&file=story/a.rpy",
                vec!["a", "b", "c", "d", "z"],
            ),
            (
                None,
                Some(vec!["translated", "reviewed", "translated"]),
                "&status=translated&status=reviewed&status=translated",
                vec!["b", "e", "z"],
            ),
            (
                Some("story/a.rpy"),
                Some(vec!["translated", "reviewed"]),
                "&file=story/a.rpy&status=translated&status=reviewed",
                vec!["b", "z"],
            ),
            (
                None,
                Some(vec![
                    "pending",
                    "translated",
                    "reviewed",
                    "approved",
                    "error",
                ]),
                "&status=pending&status=translated&status=reviewed&status=approved&status=error",
                vec!["a", "b", "c", "d", "e", "z"],
            ),
        ] {
            let expected: Vec<_> = all
                .iter()
                .filter(|row| ids.contains(&row.id.as_str()))
                .cloned()
                .collect();
            for format in ["po", "xliff"] {
                let output = dir.path().join(format!("assignment.{format}"));
                let result = apply_export_translations(
                    &state,
                    format.into(),
                    "es".into(),
                    output.to_string_lossy().into_owned(),
                    file.map(str::to_owned),
                    statuses
                        .as_ref()
                        .map(|values| values.iter().map(|value| (*value).into()).collect()),
                )
                .await
                .unwrap();
                assert_eq!(result["entries"], expected.len());
                let bytes = std::fs::read(&output).unwrap();
                let legacy = if format == "po" {
                    locust_core::export::export_po(&expected, "ja", "es")
                } else {
                    locust_core::export::export_xliff(&expected, "ja", "es")
                };
                assert_eq!(bytes, legacy.as_bytes());
                assert_eq!(result["bytes"], bytes.len());
                let (status, http_bytes) = http_export(&base, format, query).await;
                assert_eq!(status, 200);
                assert_eq!(http_bytes, bytes, "{format}: {query}");
            }
        }
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn export_scope_errors_match_http_and_never_create_or_replace_a_file() {
        let (dir, state) = export_scope_state().await;
        let (base, server) = locust_server::start_test_server(state.clone()).await;
        for (file, statuses, query, error) in [
            (Some("missing.rpy"), None, "&file=missing.rpy", "no entries to export"),
            (Some("story/A.rpy"), None, "&file=story/A.rpy", "no entries to export"),
            (Some("story/b.rpy"), Some(vec!["reviewed"]), "&file=story/b.rpy&status=reviewed", "no entries to export"),
            (None, Some(vec![]), "&status=", "no entries to export"),
            (None, Some(vec!["bogus"]), "&status=bogus", "invalid export status 'bogus'; valid values: pending, translated, reviewed, approved, error"),
        ] {
            for format in ["po", "xliff"] {
                for existing in [false, true] {
                    let output = dir.path().join(if existing { "existing.po" } else { "absent/assignment.po" });
                    if existing { std::fs::write(&output, b"keep this catalog").unwrap(); }
                    let result = apply_export_translations(&state, format.into(), "es".into(), output.to_string_lossy().into_owned(), file.map(str::to_owned), statuses.as_ref().map(|values| values.iter().map(|value| (*value).into()).collect())).await;
                    assert_eq!(result.unwrap_err(), error);
                    if existing {
                        assert_eq!(std::fs::read(&output).unwrap(), b"keep this catalog");
                    } else {
                        assert!(!output.parent().unwrap().exists());
                    }
                }
                let (status, body) = http_export(&base, format, query).await;
                assert_eq!(status, 400);
                assert_eq!(body, error.as_bytes());
            }
        }
        let held = try_project_operation(&state).unwrap();
        let output = dir.path().join("blocked.po");
        for format in ["po", "xliff"] {
            assert_eq!(
                apply_export_translations(
                    &state,
                    format.into(),
                    "es".into(),
                    output.to_string_lossy().into_owned(),
                    Some("story/a.rpy".into()),
                    Some(vec!["translated".into()])
                )
                .await
                .unwrap_err(),
                locust_server::PROJECT_BUSY_MESSAGE
            );
            assert!(!output.exists());
        }
        drop(held);
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn import_requires_an_open_project() {
        let state = locust_server::create_test_state();
        state
            .db
            .save_entries(&[translated_entry("leftover", "Hello", "Hola")])
            .unwrap();
        let before = serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap();
        let input_guard = tempfile::Builder::new()
            .prefix("locust_import_")
            .tempdir()
            .unwrap();
        let input = input_guard.path().join("catalog");
        let po = "msgctxt \"leftover\"\nmsgid \"Hello\"\nmsgstr \"Wrong\"\n";
        let xliff = r#"<xliff version="1.2"><file><body><trans-unit id="leftover"><source>Hello</source><target>Wrong</target></trans-unit></body></file></xliff>"#;

        for (format, body) in [("po", po), ("xliff", xliff)] {
            std::fs::write(&input, body).unwrap();
            assert_eq!(
                apply_import_translations(
                    &state,
                    format.into(),
                    input.to_string_lossy().into_owned(),
                )
                .await
                .unwrap_err(),
                "no project open"
            );
            assert_eq!(
                serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap(),
                before
            );
        }
        std::fs::remove_file(&input).unwrap();
    }

    #[tokio::test]
    async fn injection_requires_an_open_project() {
        let state = locust_server::create_test_state();
        state
            .db
            .save_entries(&[translated_entry("leftover", "Hello", "Hola")])
            .unwrap();
        let before = serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap();
        let recordings_before = state.db.list_recorded_langs().unwrap();
        let game_guard = tempfile::Builder::new()
            .prefix("locust_inject_")
            .tempdir()
            .unwrap();
        let game = game_guard.path().to_path_buf();
        let file = game.join("story.html");
        let original = b"<p>Hello</p>";
        std::fs::write(&file, original).unwrap();

        for direct in [false, true] {
            let params = InjectParams {
                project_path: game.to_string_lossy().into_owned(),
                format_id: "html-game".into(),
                mode: None,
                languages: vec!["es".into()],
                output_dir: None,
                direct,
                require_binary_slot_fit: false,
            };
            assert_eq!(
                apply_run_inject(params, &state).await.unwrap_err(),
                "no project open"
            );
            assert_eq!(
                serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap(),
                before
            );
            assert_eq!(state.db.list_recorded_langs().unwrap(), recordings_before);
            assert_eq!(std::fs::read(&file).unwrap(), original);
        }
        std::fs::remove_dir_all(&game).unwrap();
    }

    #[tokio::test]
    async fn glossary_add_preserves_case_sensitive_and_context() {
        let state = locust_server::create_test_state();
        *state.current_project.write().await = Some(ProjectInfo::default());
        for context in [Some("  Organization title\nUSA  "), Some(""), None] {
            let entry = GlossaryEntry {
                term: "US".into(),
                translation: "EEUU".into(),
                lang_pair: "en-es".into(),
                context: context.map(str::to_owned),
                case_sensitive: true,
            };
            apply_add_glossary_entry(&state, &entry).await.unwrap();
            let entries = apply_get_glossary(&state, "en-es").await.unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                serde_json::to_value(&entries[0]).unwrap(),
                serde_json::to_value(&entry).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn glossary_requires_an_open_project() {
        let state = locust_server::create_test_state();
        state
            .db
            .save_entries(&[translated_entry("leftover", "Hello", "Hola")])
            .unwrap();
        state.glossary.add("HP", "PV", "en-es", None).unwrap();
        let before = serde_json::to_value(state.glossary.get_all("en-es").unwrap()).unwrap();
        let row_before = serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap();
        let new_entry = GlossaryEntry {
            term: "MP".into(),
            translation: "PM".into(),
            lang_pair: "en-es".into(),
            context: None,
            case_sensitive: false,
        };

        assert_eq!(
            apply_get_glossary(&state, "en-es").await.unwrap_err(),
            "no project open"
        );
        assert_eq!(
            apply_add_glossary_entry(&state, &new_entry)
                .await
                .unwrap_err(),
            "no project open"
        );
        assert_eq!(
            serde_json::to_value(state.glossary.get_all("en-es").unwrap()).unwrap(),
            before
        );
        assert_eq!(
            serde_json::to_value(state.db.get_entry("leftover").unwrap()).unwrap(),
            row_before
        );
    }

    #[tokio::test]
    async fn open_project_db_switches_without_extract_or_touching_source() {
        let dir_guard = tempfile::Builder::new()
            .prefix("locust_opendb_t_")
            .tempdir()
            .unwrap();
        let dir = dir_guard.path().to_path_buf();
        let source_path = dir.join("source.locust.db");
        {
            let src = locust_core::database::Database::open(&source_path).unwrap();
            src.save_entries(&[
                translated_entry("a", "Hello", "Hola"),
                StringEntry::new("b", "World", PathBuf::from("data/a.json")),
            ])
            .unwrap();
            drop(src);
        }

        let state = locust_server::create_test_state_with_db(&source_path);
        *state.current_project.write().await = Some(ProjectInfo {
            path: PathBuf::from("/tmp/original-game"),
            format_id: "rpgmaker-mv".into(),
            name: "original-game".into(),
            extraction_warnings: Vec::new(),
            ..Default::default()
        });

        let pivot = dir.join("pivoted.locust.db");
        assert_eq!(state.db.pivot_to(&pivot).unwrap().entries, 1);

        let game = dir.join("PivotedGame");
        std::fs::create_dir_all(&game).unwrap();

        let out = apply_open_project_db(
            &state,
            pivot.to_string_lossy().into_owned(),
            game.to_string_lossy().into_owned(),
            "rpgmaker-mv".into(),
        )
        .await
        .unwrap();

        assert_eq!(out.total_strings, 1);
        assert_eq!(out.added, 0);
        assert_eq!(out.updated, 0);
        assert_eq!(out.stale_source_reset, 0);
        assert_eq!(out.removed, 0);
        assert_eq!(out.preserved_translations, 0);
        assert_eq!(out.format_id, "rpgmaker-mv");
        assert_eq!(out.project_name, "PivotedGame");
        assert_eq!(out.database_path, pivot.to_string_lossy());

        let live = state.db.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].source, "Hola");

        let cur = state.current_project.read().await.clone().unwrap();
        assert_eq!(cur.name, "PivotedGame");
        assert_eq!(cur.format_id, "rpgmaker-mv");
        assert_eq!(cur.database_path.as_ref(), Some(&pivot));
        assert_eq!(cur.supported_modes, out.supported_modes);
        assert!(out.persistence_warning.is_none());
        let saved = locust_core::config::AppConfig::load(&state.config_path).unwrap();
        assert_eq!(
            saved.recent_projects[0].database_path.as_ref(),
            Some(&pivot)
        );

        let src = locust_core::database::Database::open(&source_path).unwrap();
        let src_rows = src.get_entries(&EntryFilter::default()).unwrap();
        assert_eq!(src_rows.len(), 2);
        assert_eq!(
            src_rows.iter().find(|e| e.id == "a").unwrap().source,
            "Hello"
        );
        drop(src);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn open_project_db_rejects_non_locust_and_keeps_current() {
        let state = locust_server::create_test_state();
        *state.current_project.write().await = Some(ProjectInfo {
            path: PathBuf::from("/tmp/original-game"),
            format_id: "rpgmaker-mv".into(),
            name: "original-game".into(),
            extraction_warnings: Vec::new(),
            ..Default::default()
        });
        state
            .db
            .save_entries(&[translated_entry("keep", "Hello", "Hola")])
            .unwrap();

        let other_guard = tempfile::Builder::new()
            .prefix("locust_opendb_tauri_bad_")
            .tempdir()
            .unwrap();
        let other = other_guard.path().join("fixture.db");
        std::fs::write(&other, b"not-a-sqlite-database").unwrap();

        let err = apply_open_project_db(
            &state,
            other.to_string_lossy().into_owned(),
            "/tmp/game".into(),
            "rpgmaker-mv".into(),
        )
        .await
        .expect_err("must reject");
        assert!(
            err.to_lowercase().contains("not a locust project database"),
            "{err}"
        );

        let cur = state.current_project.read().await.clone().unwrap();
        assert_eq!(cur.name, "original-game");
        assert_eq!(state.db.get_entry("keep").unwrap().unwrap().source, "Hello");
        assert_eq!(std::fs::read(&other).unwrap(), b"not-a-sqlite-database");
        let _ = std::fs::remove_file(&other);
    }

    #[tokio::test]
    async fn open_project_db_rejects_missing_file() {
        let state = locust_server::create_test_state();
        *state.current_project.write().await = Some(ProjectInfo {
            path: PathBuf::from("/tmp/original-game"),
            format_id: "rpgmaker-mv".into(),
            name: "original-game".into(),
            extraction_warnings: Vec::new(),
            ..Default::default()
        });
        let missing = std::env::temp_dir().join(format!(
            "locust_opendb_tauri_missing_{}.locust.db",
            uuid::Uuid::new_v4()
        ));
        let err = apply_open_project_db(
            &state,
            missing.to_string_lossy().into_owned(),
            "/tmp/game".into(),
            "rpgmaker-mv".into(),
        )
        .await
        .expect_err("must reject");
        assert!(
            err.to_lowercase().contains("project not found")
                || err.to_lowercase().contains("not found"),
            "{err}"
        );
        let cur = state.current_project.read().await.clone().unwrap();
        assert_eq!(cur.name, "original-game");
        assert_eq!(state.db.path(), PathBuf::from(":memory:"));
    }

    #[tokio::test]
    async fn open_project_db_cannot_switch_during_an_owned_operation() {
        let state = locust_server::create_test_state();
        state
            .db
            .save_entries(&[translated_entry("keep", "Japanese source", "English")])
            .unwrap();
        let operation = try_project_operation(&state).unwrap();
        let error = apply_open_project_db(
            &state,
            "not-read-while-busy.locust.db".into(),
            "not-read-while-busy".into(),
            "rpgmaker-mv".into(),
        )
        .await
        .expect_err("another operation owns this project");
        assert_eq!(error, locust_server::PROJECT_BUSY_MESSAGE);
        assert_eq!(
            state.db.get_entry("keep").unwrap().unwrap().source,
            "Japanese source"
        );
        assert_eq!(state.db.path(), PathBuf::from(":memory:"));
        drop(operation);
        let error = apply_open_project_db(
            &state,
            format!("missing-{}.locust.db", uuid::Uuid::new_v4()),
            "missing-game".into(),
            "rpgmaker-mv".into(),
        )
        .await
        .expect_err("missing database");
        assert_ne!(error, locust_server::PROJECT_BUSY_MESSAGE);
        assert!(
            try_project_operation(&state).is_ok(),
            "failed open must release its reservation"
        );
    }

    #[tokio::test]
    async fn open_project_db_rejects_unknown_format() {
        let (_db_path_guard, db_path) =
            write_tauri_locust_db(&[translated_entry("a", "Hello", "Hola")]);
        let state = locust_server::create_test_state();
        *state.current_project.write().await = Some(ProjectInfo {
            path: PathBuf::from("/tmp/original-game"),
            format_id: "rpgmaker-mv".into(),
            name: "original-game".into(),
            extraction_warnings: Vec::new(),
            ..Default::default()
        });
        let err = apply_open_project_db(
            &state,
            db_path.to_string_lossy().into_owned(),
            "/tmp/game".into(),
            "not-a-format".into(),
        )
        .await
        .expect_err("must reject");
        assert!(err.contains("not-a-format"), "{err}");
        let cur = state.current_project.read().await.clone().unwrap();
        assert_eq!(cur.name, "original-game");
        assert_eq!(state.db.path(), PathBuf::from(":memory:"));
        drop(state);
        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn prefer_saved_open_skips_extract_after_source_change() {
        let game_guard = tempfile::Builder::new()
            .prefix("locust_prefer_saved_")
            .tempdir()
            .unwrap();
        let game = game_guard.path().join("game");
        std::fs::create_dir(&game).unwrap();
        std::fs::write(game.join("story.html"), "<p>Hello</p>").unwrap();
        let state = locust_server::create_test_state();
        let first = apply_open_project(
            &state,
            game.to_string_lossy().into_owned(),
            Some("html-game".into()),
            false,
        )
        .await
        .unwrap();
        let hero = state
            .db
            .get_entries(&EntryFilter::default())
            .unwrap()
            .into_iter()
            .find(|e| e.source == "Hello")
            .unwrap();
        assert!(state
            .db
            .save_translation(&hero.id, "Hola", "mock")
            .await
            .unwrap());
        std::fs::write(game.join("story.html"), "<p>Hello, traveler</p>").unwrap();

        let saved = apply_open_project(
            &state,
            game.to_string_lossy().into_owned(),
            Some("html-game".into()),
            true,
        )
        .await
        .unwrap();
        assert_eq!(saved.stale_source_reset, 0);
        assert_eq!(saved.database_path, first.database_path);
        assert_eq!(
            state.db.get_entry(&hero.id).unwrap().unwrap().source,
            "Hello"
        );

        let explicit = apply_open_project(
            &state,
            game.to_string_lossy().into_owned(),
            Some("html-game".into()),
            false,
        )
        .await
        .unwrap();
        assert_eq!(explicit.stale_source_reset, 1);
        drop(state);
        let _ = std::fs::remove_file(&first.database_path);
        let _ = std::fs::remove_dir_all(&game);
    }

    #[tokio::test]
    async fn xai_auth_poll_expired_entry_is_removed() {
        use locust_providers::xai_oauth::DeviceCode;
        use locust_server::{XaiAuthStatus, XaiPendingAuth};
        use std::sync::Arc;

        let state = locust_server::create_test_state();
        let handle = "expired-handle".to_string();
        state.xai_pending.insert(
            handle.clone(),
            XaiPendingAuth {
                device: Arc::new(DeviceCode::from_parts(
                    "raw-device-must-not-leak",
                    "CODE",
                    "https://auth.x.ai/device",
                    5,
                    1,
                    1,
                )),
                last_poll: None,
            },
        );
        let resp = poll_xai_device_login(&state, &handle).await.unwrap();
        assert_eq!(resp.status, XaiAuthStatus::Expired);
        assert!(state.xai_pending.get(&handle).is_none());
        let err = poll_xai_device_login(&state, &handle)
            .await
            .expect_err("consumed handle");
        assert!(err.to_lowercase().contains("not found"), "{err}");
    }
}
