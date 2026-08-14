use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use locust_core::config::AppConfig;
use locust_core::database::{EntryFilter, GlossaryEntry, PivotResult, ProjectStats, StringFacets};
use locust_core::extraction::PluginInfo;
use locust_core::models::{OutputMode, StringEntry, StringStatus};
use locust_core::project;
use locust_core::translation::TranslationOptions;
use locust_core::validation::Validator;
use locust_server::{
    poll_xai_device_login, spawn_translation_job, start_xai_device_login, AppState, ProjectInfo,
    XaiAuthPollResponse, XaiAuthStartResponse,
};

/// Wrapper so we can use Arc<AppState> as Tauri managed state
pub struct AppStateWrapper(pub Arc<AppState>);
pub struct ServerPort(pub u16);

// ─── Utility commands ───────────────────────────────────────────────────────

#[tauri::command]
pub fn get_server_port(port: State<ServerPort>) -> u16 {
    port.0
}

#[tauri::command]
pub async fn pick_game_folder() -> Result<Option<String>, String> {
    // The frontend uses @tauri-apps/plugin-dialog directly for the native dialog.
    // This command exists as a fallback / placeholder.
    Ok(None)
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
}

#[tauri::command]
pub async fn open_project(
    path: String,
    format_id: Option<String>,
    state: State<'_, AppStateWrapper>,
) -> Result<ProjectOpenResponse, String> {
    let s = &state.0;
    let raw_path = PathBuf::from(&path);
    let outcome = project::open_project(&s.db, &s.format_registry, &raw_path, format_id.as_deref())
        .map_err(|e| e.to_string())?;

    {
        let mut proj = s.current_project.write().await;
        *proj = Some(ProjectInfo {
            path: outcome.project_path.clone(),
            format_id: outcome.format_id.clone(),
            name: outcome.project_name.clone(),
        });
    }

    {
        let mut config = s.config.write().await;
        config.add_recent_project(
            outcome.project_path.clone(),
            outcome.project_name.clone(),
            outcome.format_id.clone(),
        );
        let _ = config.save(&AppConfig::default_path());
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
    })
}

async fn apply_open_project_db(
    s: &AppState,
    database_path: String,
    game_path: String,
    format_id: String,
) -> Result<ProjectOpenResponse, String> {
    let outcome = project::open_project_db(
        &s.db,
        &s.format_registry,
        Path::new(&database_path),
        Path::new(&game_path),
        &format_id,
    )
    .map_err(|e| e.to_string())?;

    {
        let mut proj = s.current_project.write().await;
        *proj = Some(ProjectInfo {
            path: outcome.project_path.clone(),
            format_id: outcome.format_id.clone(),
            name: outcome.project_name.clone(),
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
    })
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
    state.0.format_registry.list()
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

    let count_filter = EntryFilter {
        status: status.clone(),
        file_path: filter.file_path.clone(),
        tag: filter.tag.clone(),
        search: filter.search.clone(),
        limit: None,
        offset: None,
    };
    let total = state
        .0
        .db
        .count_entries(&count_filter)
        .map_err(|e| e.to_string())?;

    let entry_filter = EntryFilter {
        status,
        file_path: filter.file_path,
        tag: filter.tag,
        search: filter.search,
        limit: Some(limit),
        offset: Some(offset),
    };
    let entries = state
        .0
        .db
        .get_entries(&entry_filter)
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
pub async fn run_pivot(
    output_path: String,
    state: State<'_, AppStateWrapper>,
) -> Result<PivotResult, String> {
    if state.0.current_project.read().await.is_none() {
        return Err("no project open".into());
    }
    state
        .0
        .db
        .pivot_to(&PathBuf::from(output_path))
        .map_err(|e| e.to_string())
}

#[derive(Deserialize)]
pub struct PatchStringReq {
    pub translation: Option<String>,
    pub status: Option<StringStatus>,
}

#[tauri::command]
pub async fn patch_string(
    id: String,
    data: PatchStringReq,
    state: State<'_, AppStateWrapper>,
) -> Result<StringEntry, String> {
    let s = &state.0;
    if let Some(ref translation) = data.translation {
        s.db.save_translation(&id, translation, "manual")
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref status) = data.status {
        s.db.update_entry_status(&id, status.clone())
            .await
            .map_err(|e| e.to_string())?;
    }
    s.db.get_entry(&id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Entry not found".to_string())
}

#[derive(Deserialize)]
pub struct BatchPatchItem {
    pub id: String,
    pub translation: String,
}

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
    let pairs: Vec<(String, String)> = data
        .updates
        .into_iter()
        .map(|u| (u.id, u.translation))
        .collect();
    let requested = pairs.len();
    let applied = state
        .0
        .db
        .save_translations_batch(pairs, &data.provider)
        .await
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "requested": requested,
        "applied": applied,
        "skipped": requested.saturating_sub(applied),
    }))
}

// ─── Translation commands ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TranslateParams {
    pub provider_id: String,
    #[serde(default)]
    pub fallback_provider_ids: Option<Vec<String>>,
    pub options: TranslationOptions,
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
    if let Some((_, job)) = state.0.active_jobs.remove(&job_id) {
        job.abort_handle.abort();
        Ok(())
    } else {
        Err("Job not found".to_string())
    }
}

// ─── Validation & Injection ─────────────────────────────────────────────────

#[tauri::command]
pub async fn run_validation(
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    let s = &state.0;
    let entries =
        s.db.get_entries(&EntryFilter::default())
            .map_err(|e| e.to_string())?;
    let validation = Validator::validate_and_save(&entries, &s.db)
        .await
        .map_err(|e| e.to_string())?;

    let proj = s.current_project.read().await;
    let fonts: Vec<locust_core::font_validation::FontCoverageReport> = if let Some(ref p) = *proj {
        let translations: Vec<&str> = entries
            .iter()
            .filter_map(|e| e.translation.as_deref())
            .collect();
        locust_core::font_validation::FontValidator::check_game_fonts(&p.path, &translations)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(serde_json::json!({
        "validation": validation,
        "fonts": fonts,
    }))
}

/// Export current project translations to a PO or XLIFF file at `path`.
/// Frontend picks the path via the native save dialog.
#[tauri::command]
pub async fn export_translations(
    format: String,
    lang: String,
    path: String,
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    let s = &state.0;
    let entries =
        s.db.get_entries(&EntryFilter::default())
            .map_err(|e| e.to_string())?;
    if entries.is_empty() {
        return Err("no strings in project — open a game and extract first".into());
    }
    let config = s.config.read().await;
    let source =
        s.db.resolve_export_source_lang(&lang, &config.default_source_lang)
            .map_err(|e| e.to_string())?;
    let body = match format.as_str() {
        "po" => locust_core::export::export_po(&entries, &source, &lang),
        "xliff" => locust_core::export::export_xliff(&entries, &source, &lang),
        other => {
            return Err(format!(
                "unknown export format \"{other}\" — use \"po\" or \"xliff\""
            ))
        }
    };
    let out = PathBuf::from(&path);
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
    let s = &state.0;
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if content.trim().is_empty() {
        return Err("import file is empty".into());
    }
    let mut imported = 0usize;
    let mut skipped = 0usize;
    match format.as_str() {
        "po" => {
            let entries = locust_core::export::import_po(&content).map_err(|e| e.to_string())?;
            for pe in &entries {
                if pe.translation.is_empty() {
                    skipped += 1;
                    continue;
                }
                let Some(ref id) = pe.id else {
                    skipped += 1;
                    continue;
                };
                if s.db
                    .save_translation(id, &pe.translation, "import")
                    .await
                    .map_err(|e| e.to_string())?
                {
                    imported += 1;
                } else {
                    skipped += 1;
                }
            }
        }
        "xliff" => {
            let units = locust_core::export::import_xliff(&content).map_err(|e| e.to_string())?;
            for unit in &units {
                if unit.target.is_empty() {
                    skipped += 1;
                    continue;
                }
                if s.db
                    .save_translation(&unit.id, &unit.target, "import")
                    .await
                    .map_err(|e| e.to_string())?
                {
                    imported += 1;
                } else {
                    skipped += 1;
                }
            }
        }
        other => {
            return Err(format!(
                "unknown import format \"{other}\" — use \"po\" or \"xliff\""
            ))
        }
    }
    Ok(serde_json::json!({
        "path": path,
        "format": format,
        "imported": imported,
        "skipped": skipped,
    }))
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
    // Same guard as CLI/server: empty languages used to return success with
    // zero work and zero recording — a silent no-op that breaks `locust patch`.
    if params.languages.is_empty() {
        return Err("inject requires at least one language (e.g. [\"es\"])".into());
    }
    let s = &state.0;

    if params.direct {
        let game_path = PathBuf::from(&params.project_path);
        let format_id = params.format_id.clone();
        let languages = params.languages.clone();
        let registry = s.format_registry.clone();
        let db = s.db.clone();
        let backup = s.backup_manager.clone();
        let report = tokio::task::spawn_blocking(move || {
            locust_core::extraction::inject_direct(
                &registry, &db, &backup, &game_path, &format_id, &languages,
            )
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        return serde_json::to_value(report).map_err(|e| e.to_string());
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
        .inject(
            &PathBuf::from(&params.project_path),
            &params.format_id,
            mode,
            params.languages,
            params.output_dir.map(PathBuf::from),
            tx,
        )
        .await
        .map_err(|e| e.to_string())?;

    // Persist what each language's injection wrote — `locust patch` packs
    // exclusively from this recording, so an inject seam that skips it
    // produces projects that can never be packed.
    locust_core::extraction::record_multilang_injection(&s.db, &report, &languages, &|_lang| {
        INJECT_RECORD_REMEDY.to_string()
    })
    .map_err(|e| e.to_string())?;

    serde_json::to_value(report).map_err(|e| e.to_string())
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
pub async fn register_lang(params: RegisterLangParams) -> Result<serde_json::Value, String> {
    let game_path = PathBuf::from(params.game_path.trim());
    if params.game_path.trim().is_empty() || !game_path.is_dir() {
        return Err(format!(
            "game_path must be an existing directory (got {:?})",
            params.game_path
        ));
    }
    let lang = params.lang;
    let label = params.label;
    let report = tokio::task::spawn_blocking(move || {
        locust_formats::rpgmaker_lang::register_language(&game_path, &lang, &label)
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
    let config = state.0.config.read().await;
    let mut val = serde_json::to_value(&*config).unwrap_or_default();
    // Redact API keys
    if let Some(providers) = val.get_mut("providers").and_then(|v| v.as_object_mut()) {
        for (_id, pc) in providers.iter_mut() {
            if let Some(obj) = pc.as_object_mut() {
                if obj
                    .get("api_key")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty())
                {
                    obj.insert(
                        "api_key".to_string(),
                        serde_json::Value::String("***".to_string()),
                    );
                }
            }
        }
    }
    Ok(val)
}

#[tauri::command]
pub async fn save_config(
    partial: serde_json::Value,
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    let mut config = state.0.config.write().await;
    let mut current = serde_json::to_value(&*config).unwrap_or_default();
    if let (Some(cur_obj), Some(patch_obj)) = (current.as_object_mut(), partial.as_object()) {
        for (k, v) in patch_obj {
            cur_obj.insert(k.clone(), v.clone());
        }
    }
    *config = serde_json::from_value(current.clone()).map_err(|e| e.to_string())?;
    let _ = config.save(&AppConfig::default_path());
    Ok(current)
}

// ─── Backups & Glossary ─────────────────────────────────────────────────────

#[tauri::command]
pub fn get_backups(
    state: State<AppStateWrapper>,
) -> Result<Vec<locust_core::backup::BackupEntry>, String> {
    state
        .0
        .backup_manager
        .list_backups()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_glossary(
    lang_pair: String,
    state: State<AppStateWrapper>,
) -> Result<Vec<GlossaryEntry>, String> {
    state
        .0
        .glossary
        .get_all(&lang_pair)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_glossary_entry(
    entry: GlossaryEntry,
    state: State<AppStateWrapper>,
) -> Result<(), String> {
    state
        .0
        .glossary
        .add(
            &entry.term,
            &entry.translation,
            &entry.lang_pair,
            entry.context.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn write_tauri_locust_db(rows: &[StringEntry]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "locust_opendb_tauri_{}.locust.db",
            uuid::Uuid::new_v4()
        ));
        let db = locust_core::database::Database::open(&path).unwrap();
        db.save_entries(rows).unwrap();
        drop(db);
        path
    }

    fn translated_entry(id: &str, source: &str, translation: &str) -> StringEntry {
        let mut e = StringEntry::new(id, source, PathBuf::from("data/a.json"));
        e.translation = Some(translation.into());
        e.status = StringStatus::Translated;
        e
    }

    #[tokio::test]
    async fn open_project_db_switches_without_extract_or_touching_source() {
        let dir = std::env::temp_dir().join(format!("locust_opendb_t_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
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
        });
        state
            .db
            .save_entries(&[translated_entry("keep", "Hello", "Hola")])
            .unwrap();

        let other = std::env::temp_dir().join(format!(
            "locust_opendb_tauri_bad_{}.db",
            uuid::Uuid::new_v4()
        ));
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
    async fn open_project_db_rejects_unknown_format() {
        let db_path = write_tauri_locust_db(&[translated_entry("a", "Hello", "Hola")]);
        let state = locust_server::create_test_state();
        *state.current_project.write().await = Some(ProjectInfo {
            path: PathBuf::from("/tmp/original-game"),
            format_id: "rpgmaker-mv".into(),
            name: "original-game".into(),
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
