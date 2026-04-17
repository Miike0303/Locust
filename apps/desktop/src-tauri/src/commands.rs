use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use locust_core::config::AppConfig;
use locust_core::database::{EntryFilter, GlossaryEntry, ProjectStats};
use locust_core::extraction::PluginInfo;
use locust_core::models::{OutputMode, StringEntry, StringStatus};
use locust_core::translation::TranslationOptions;
use locust_core::validation::Validator;
use locust_server::{AppState, ProjectInfo};

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

#[derive(Serialize)]
pub struct ProjectOpenResponse {
    pub format_id: String,
    pub format_name: String,
    pub total_strings: usize,
    pub project_path: String,
    pub project_name: String,
    pub supported_modes: Vec<OutputMode>,
}

#[tauri::command]
pub async fn open_project(
    path: String,
    format_id: Option<String>,
    state: State<'_, AppStateWrapper>,
) -> Result<ProjectOpenResponse, String> {
    let s = &state.0;
    let raw_path = PathBuf::from(&path);
    if !raw_path.exists() {
        return Err("Path not found".into());
    }

    // Resolve executable/file path to game root
    let path = locust_core::extraction::resolve_game_root(&raw_path, &s.format_registry);

    let plugin = if let Some(ref fid) = format_id {
        s.format_registry
            .get(fid)
            .ok_or_else(|| format!("Unknown format: {}", fid))?
    } else {
        s.format_registry
            .detect(&path)
            .ok_or_else(|| "Could not detect game format".to_string())?
    };

    let format_id = plugin.id().to_string();
    let format_name = plugin.name().to_string();
    let supported_modes = plugin.supported_modes();

    let entries = plugin.extract(&path).map_err(|e| e.to_string())?;

    s.db.clear_entries().map_err(|e| e.to_string())?;
    let total_strings = s.db.save_entries(&entries).map_err(|e| e.to_string())?;

    let project_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    {
        let mut proj = s.current_project.write().await;
        *proj = Some(ProjectInfo {
            path: path.clone(),
            format_id: format_id.clone(),
            name: project_name.clone(),
        });
    }

    let project_path = path.to_string_lossy().to_string();

    {
        let mut config = s.config.write().await;
        config.add_recent_project(path, project_name.clone(), format_id.clone());
        let _ = config.save(&AppConfig::default_path());
    }

    Ok(ProjectOpenResponse {
        format_id,
        format_name,
        total_strings,
        project_path,
        project_name,
        supported_modes,
    })
}

// ─── Format & Provider commands ─────────────────────────────────────────────

#[tauri::command]
pub fn get_formats(state: State<AppStateWrapper>) -> Vec<PluginInfo> {
    state.0.format_registry.list()
}

#[tauri::command]
pub async fn get_providers(state: State<'_, AppStateWrapper>) -> Result<Vec<serde_json::Value>, String> {
    let reg = state.0.provider_registry.read().await;
    Ok(serde_json::to_value(reg.list())
        .unwrap_or_default()
        .as_array()
        .cloned()
        .unwrap_or_default())
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
pub fn get_stats(state: State<AppStateWrapper>) -> Result<ProjectStats, String> {
    state.0.db.get_stats().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_strings(filter: StringsFilter, state: State<AppStateWrapper>) -> Result<StringsResponse, String> {
    let status = filter.status.and_then(|s| s.parse::<StringStatus>().ok());
    let limit = filter.limit.unwrap_or(100);
    let offset = filter.offset.unwrap_or(0);

    let count_filter = EntryFilter {
        status: status.clone(),
        file_path: filter.file_path.clone(),
        tag: filter.tag.clone(),
        search: filter.search.clone(),
        limit: None,
        offset: None,
    };
    let total = state.0.db.count_entries(&count_filter).map_err(|e| e.to_string())?;

    let entry_filter = EntryFilter {
        status,
        file_path: filter.file_path,
        tag: filter.tag,
        search: filter.search,
        limit: Some(limit),
        offset: Some(offset),
    };
    let entries = state.0.db.get_entries(&entry_filter).map_err(|e| e.to_string())?;

    Ok(StringsResponse { entries, total, offset, limit })
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

// ─── Translation commands ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct TranslateParams {
    pub provider_id: String,
    pub options: TranslationOptions,
}

#[tauri::command]
pub async fn start_translation(
    params: TranslateParams,
    state: State<'_, AppStateWrapper>,
) -> Result<String, String> {
    let s = &state.0;
    let reg = s.provider_registry.read().await;
    let provider = reg
        .get(&params.provider_id)
        .ok_or_else(|| "Provider not found".to_string())?;

    let job_id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1000);
    let (broadcast_tx, _) = tokio::sync::broadcast::channel(1000);
    let broadcast_tx_clone = broadcast_tx.clone();

    let entries = s.db.get_entries(&EntryFilter::default()).map_err(|e| e.to_string())?;
    let manager = locust_core::translation::TranslationManager::new(
        provider,
        s.db.clone(),
        s.glossary.clone(),
    );
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    let job_id_clone = job_id.clone();

    let jobs = s.active_jobs.clone();
    let cleanup_job_id = job_id.clone();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let is_terminal = matches!(
                event,
                locust_core::models::ProgressEvent::Completed { .. }
                    | locust_core::models::ProgressEvent::Failed { .. }
            );
            let _ = broadcast_tx_clone.send(event);
            if is_terminal {
                break;
            }
        }
        jobs.remove(&cleanup_job_id);
    });

    let handle = tokio::spawn(async move {
        let _ = manager
            .translate_entries(entries, params.options, tx, job_id_clone, cancel_clone)
            .await;
    });

    s.active_jobs.insert(
        job_id.clone(),
        locust_server::JobState {
            abort_handle: handle.abort_handle(),
            progress_tx: broadcast_tx,
        },
    );

    Ok(job_id)
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
pub async fn run_validation(state: State<'_, AppStateWrapper>) -> Result<serde_json::Value, String> {
    let s = &state.0;
    let entries = s.db.get_entries(&EntryFilter::default()).map_err(|e| e.to_string())?;
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

#[derive(Deserialize)]
pub struct InjectParams {
    pub project_path: String,
    pub format_id: String,
    pub mode: OutputMode,
    pub languages: Vec<String>,
    pub output_dir: Option<String>,
}

#[tauri::command]
pub async fn run_inject(
    params: InjectParams,
    state: State<'_, AppStateWrapper>,
) -> Result<serde_json::Value, String> {
    let s = &state.0;
    let injector = locust_core::extraction::MultiLangInjector::new(
        s.format_registry.clone(),
        s.db.clone(),
        s.backup_manager.clone(),
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let report = injector
        .inject(
            &PathBuf::from(&params.project_path),
            &params.format_id,
            params.mode,
            params.languages,
            params.output_dir.map(PathBuf::from),
            tx,
        )
        .await
        .map_err(|e| e.to_string())?;

    serde_json::to_value(report).map_err(|e| e.to_string())
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
                if obj.get("api_key").and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty()) {
                    obj.insert("api_key".to_string(), serde_json::Value::String("***".to_string()));
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
pub fn get_backups(state: State<AppStateWrapper>) -> Result<Vec<locust_core::backup::BackupEntry>, String> {
    state.0.backup_manager.list_backups().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_glossary(
    lang_pair: String,
    state: State<AppStateWrapper>,
) -> Result<Vec<GlossaryEntry>, String> {
    state.0.glossary.get_all(&lang_pair).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_glossary_entry(
    entry: GlossaryEntry,
    state: State<AppStateWrapper>,
) -> Result<(), String> {
    state
        .0
        .glossary
        .add(&entry.term, &entry.translation, &entry.lang_pair, entry.context.as_deref())
        .map_err(|e| e.to_string())
}
