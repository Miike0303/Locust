import { invoke } from "@tauri-apps/api/core";
import { addLog } from "../stores/logStore";

// ─── Runtime detection ────────────────────────────────────────────────────
const IS_TAURI = "__TAURI_INTERNALS__" in window;

// ─── HTTP fallback helpers ────────────────────────────────────────────────
let _serverPort = 7842;

async function getBaseUrl(): Promise<string> {
  if (IS_TAURI) {
    try {
      _serverPort = await invoke<number>("get_server_port");
    } catch { /* use default */ }
    return `http://localhost:${_serverPort}/api`;
  }
  return import.meta.env.DEV ? "/api" : `http://localhost:7842/api`;
}

let _basePromise: Promise<string> | null = null;
function baseUrl(): Promise<string> {
  if (!_basePromise) _basePromise = getBaseUrl();
  return _basePromise;
}

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const base = await baseUrl();
  const res = await fetch(`${base}${path}`, {
    headers: { "Content-Type": "application/json", ...options?.headers },
    ...options,
  });
  if (!res.ok) {
    const text = await res.text();
    addLog("error", `API ${res.status}: ${path}`, text, "api");
    throw new Error(`${res.status}: ${text}`);
  }
  return res.json();
}

async function requestText(path: string): Promise<string> {
  const base = await baseUrl();
  const res = await fetch(`${base}${path}`);
  if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
  return res.text();
}

// ─── Types ─────────────────────────────────────────────────────────────────

export type StringStatus = "pending" | "translated" | "reviewed" | "approved" | "error";
export type OutputMode = "replace" | "add";

export interface PluginInfo {
  id: string; name: string; description: string;
  extensions: string[]; supported_modes: OutputMode[];
}

export interface ProviderInfo {
  id: string; name: string; is_free: boolean; requires_api_key: boolean;
}

export interface ProjectInfo {
  path: string; format_id: string; name: string;
}

export interface ProjectOpenResponse {
  format_id: string; format_name: string; total_strings: number;
  project_path: string; project_name: string; supported_modes: OutputMode[];
}

export interface StringEntry {
  id: string; source: string; translation: string | null;
  file_path: string; context: string | null; tags: string[];
  status: StringStatus; provider_used: string | null;
  char_limit: number | null; created_at: string;
  translated_at: string | null; reviewed_at: string | null;
}

export interface StringFilter {
  status?: string; file_path?: string; tag?: string;
  search?: string; limit?: number; offset?: number;
}

export interface StringsResponse {
  entries: StringEntry[]; total: number; offset: number; limit: number;
}

export interface ProjectStats {
  total: number; pending: number; translated: number;
  reviewed: number; approved: number; error: number;
  total_cost_usd: number;
}

export interface GlossaryEntry {
  term: string; translation: string; lang_pair: string;
  context: string | null; case_sensitive: boolean;
}

export interface BackupEntry {
  id: string; path: string; created_at: string;
  source_path: string; file_count: number; size_bytes: number;
}

export interface AppConfig {
  providers: Record<string, any>;
  default_provider: string | null;
  default_source_lang: string;
  default_target_lang: string;
  default_batch_size: number;
  default_cost_limit: number | null;
  ui: { theme: string; font_size: number; show_source_column: boolean; table_row_height: number };
  recent_projects: { path: string; name: string; format_id: string; last_opened: string }[];
}

export interface TranslationStartParams {
  provider_id: string;
  options: {
    source_lang: string; target_lang: string; batch_size: number;
    max_concurrent: number; cost_limit_usd: number | null;
    game_context: string | null; use_glossary: boolean;
    use_memory: boolean; skip_approved: boolean;
  };
}

export interface InjectParams {
  project_path: string; format_id: string; mode: OutputMode;
  languages: string[]; output_dir?: string;
}

export interface MultiLangReport {
  mode: OutputMode; languages_processed: string[];
  languages_failed: [string, string][]; backup_id: string;
  reports: Record<string, any>;
}

export interface ValidationResponse {
  validation: any; fonts: any[];
}

export interface ProgressEventStarted { type: "started"; total: number; job_id: string }
export interface ProgressEventBatchCompleted { type: "batch_completed"; completed: number; total: number; cost_so_far: number; language: string | null }
export interface ProgressEventStringTranslated { type: "string_translated"; entry_id: string; translation: string }
export interface ProgressEventCompleted { type: "completed"; total_translated: number; total_cost: number; duration_secs: number }
export interface ProgressEventFailed { type: "failed"; entry_id: string | null; error: string }

// ─── API functions (Tauri IPC with HTTP fallback) ─────────────────────────

export const getFormats = (): Promise<PluginInfo[]> =>
  IS_TAURI ? invoke("get_formats") : request("/formats");

export const getProviders = (): Promise<ProviderInfo[]> =>
  IS_TAURI ? invoke("get_providers") : request("/providers");

export const checkProviderHealth = (id: string) =>
  request<{ ok: boolean; message: string }>(`/providers/${id}/health`, { method: "POST" });

export const openProject = (path: string, formatId?: string): Promise<ProjectOpenResponse> =>
  IS_TAURI
    ? invoke("open_project", { path, formatId })
    : request("/project/open", { method: "POST", body: JSON.stringify({ path, format_id: formatId }) });

export const getCurrentProject = () =>
  request<ProjectInfo | null>("/project/current");

export const getStrings = (filter: StringFilter): Promise<StringsResponse> =>
  IS_TAURI
    ? invoke("get_strings", { filter })
    : (() => {
        const params = new URLSearchParams();
        if (filter.status) params.set("status", filter.status);
        if (filter.file_path) params.set("file_path", filter.file_path);
        if (filter.tag) params.set("tag", filter.tag);
        if (filter.search) params.set("search", filter.search);
        if (filter.limit) params.set("limit", String(filter.limit));
        if (filter.offset) params.set("offset", String(filter.offset));
        return request<StringsResponse>(`/strings?${params}`);
      })();

export const getString = (id: string) =>
  request<StringEntry>(`/strings/${encodeURIComponent(id)}`);

export const patchString = (id: string, data: Partial<Pick<StringEntry, "translation" | "status">>): Promise<StringEntry> =>
  IS_TAURI
    ? invoke("patch_string", { id, data })
    : request(`/strings/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify(data) });

export const getStats = (): Promise<ProjectStats> =>
  IS_TAURI ? invoke("get_stats") : request("/stats");

export const startTranslation = (params: TranslationStartParams): Promise<{ job_id: string }> =>
  IS_TAURI
    ? invoke<string>("start_translation", { params }).then(job_id => ({ job_id }))
    : request("/translate/start", { method: "POST", body: JSON.stringify(params) });

export const cancelTranslation = (jobId: string): Promise<void> =>
  IS_TAURI
    ? invoke("cancel_translation", { jobId })
    : request(`/translate/cancel/${jobId}`, { method: "POST" });

export const inject = (params: InjectParams): Promise<MultiLangReport> =>
  IS_TAURI
    ? invoke("run_inject", { params })
    : request("/inject", { method: "POST", body: JSON.stringify(params) });

export const validate = (): Promise<ValidationResponse> =>
  IS_TAURI
    ? invoke("run_validation")
    : request("/validate", { method: "POST" });

export const getGlossary = (langPair: string): Promise<GlossaryEntry[]> =>
  IS_TAURI
    ? invoke("get_glossary", { langPair })
    : request(`/glossary?lang_pair=${encodeURIComponent(langPair)}`);

export const addGlossaryEntry = (entry: GlossaryEntry): Promise<void> =>
  IS_TAURI
    ? invoke("add_glossary_entry", { entry })
    : request("/glossary", { method: "POST", body: JSON.stringify(entry) });

export const deleteGlossaryEntry = (term: string, langPair: string) =>
  request<void>(`/glossary/${encodeURIComponent(term)}?lang_pair=${encodeURIComponent(langPair)}`, { method: "DELETE" });

export const exportPo = (lang: string) => requestText(`/export/po?lang=${encodeURIComponent(lang)}`);
export const exportXliff = (lang: string) => requestText(`/export/xliff?lang=${encodeURIComponent(lang)}`);
export const importPo = (lang: string, content: string) =>
  request<{ imported: number }>(`/import/po?lang=${encodeURIComponent(lang)}`, { method: "POST", body: content, headers: { "Content-Type": "text/plain" } });

export const getConfig = (): Promise<AppConfig> =>
  IS_TAURI ? invoke("get_config") : request("/config");

export const updateConfig = (partial: Partial<AppConfig>): Promise<AppConfig> =>
  IS_TAURI
    ? invoke("save_config", { partial })
    : request("/config", { method: "PATCH", body: JSON.stringify(partial) });

export const getBackups = (): Promise<BackupEntry[]> =>
  IS_TAURI ? invoke("get_backups") : request("/backups");

export const restoreBackup = (id: string) =>
  request<void>(`/backups/${id}/restore`, { method: "POST" });

// ─── Translation Memory ──────────────────────────────────────────────────

export interface MemoryEntry {
  source_hash: string;
  lang_pair: string;
  source: string;
  translation: string;
  uses: number;
  last_used: string;
}

export interface MemoryListResponse {
  entries: MemoryEntry[];
  total: number;
  limit: number;
  offset: number;
}

export interface MemoryFilter {
  search?: string;
  lang_pair?: string;
  limit?: number;
  offset?: number;
}

export const getTranslationMemoryStats = (): Promise<{ project_entries: number; global_entries: number }> =>
  request("/memory/stats");

export const getTranslationMemoryLangPairs = (): Promise<string[]> =>
  request("/memory/lang-pairs");

export const getTranslationMemory = (filter: MemoryFilter): Promise<MemoryListResponse> => {
  const params = new URLSearchParams();
  if (filter.search) params.set("search", filter.search);
  if (filter.lang_pair) params.set("lang_pair", filter.lang_pair);
  if (filter.limit) params.set("limit", String(filter.limit));
  if (filter.offset) params.set("offset", String(filter.offset));
  return request(`/memory?${params}`);
};

export const deleteTranslationMemoryEntry = (hash: string, langPair: string): Promise<void> =>
  request(`/memory/${encodeURIComponent(hash)}/${encodeURIComponent(langPair)}`, { method: "DELETE" });

export const clearTranslationMemory = (): Promise<void> =>
  request("/memory", { method: "DELETE" });

/** Get the WebSocket URL for a translation job */
export async function getWsUrl(jobId: string): Promise<string> {
  await baseUrl(); // ensure _serverPort is resolved
  const port = IS_TAURI ? _serverPort : 7842;
  return `ws://localhost:${port}/api/translate/ws/${jobId}`;
}
