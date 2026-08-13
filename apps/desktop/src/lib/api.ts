import { invoke } from "@tauri-apps/api/core";
import { addLog } from "../stores/logStore";

// ─── Runtime detection ────────────────────────────────────────────────────
const IS_TAURI = "__TAURI_INTERNALS__" in window;

// ─── HTTP fallback helpers ────────────────────────────────────────────────
let _serverPort = 7842;
let _basePromise: Promise<string> | null = null;

async function getBaseUrl(): Promise<string> {
  if (IS_TAURI) {
    try {
      _serverPort = await invoke<number>("get_server_port");
    } catch {
      // Fallback may serve this call, but must not be memoized — sidecar may still be booting.
      _basePromise = null;
      return `http://localhost:${_serverPort}/api`;
    }
    return `http://localhost:${_serverPort}/api`;
  }
  return import.meta.env.DEV ? "/api" : `http://localhost:7842/api`;
}

function baseUrl(): Promise<string> {
  if (!_basePromise) _basePromise = getBaseUrl();
  return _basePromise;
}

function unreachableBackend(base: string, path: string): Error {
  const msg = `Cannot reach Locust backend at ${base} — is locust server running?`;
  addLog("error", `API unreachable: ${path}`, msg, "api");
  return new Error(msg);
}

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const base = await baseUrl();
  let res: Response;
  try {
    res = await fetch(`${base}${path}`, {
      headers: { "Content-Type": "application/json", ...options?.headers },
      ...options,
    });
  } catch {
    throw unreachableBackend(base, path);
  }
  if (!res.ok) {
    const text = await res.text();
    addLog("error", `API ${res.status}: ${path}`, text, "api");
    throw new Error(`${res.status}: ${text}`);
  }
  // 204 / empty body (DELETE, some POSTs) — do not call res.json()
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  if (!text) return undefined as T;
  return JSON.parse(text) as T;
}

async function requestText(path: string): Promise<string> {
  const base = await baseUrl();
  let res: Response;
  try {
    res = await fetch(`${base}${path}`);
  } catch {
    throw unreachableBackend(base, path);
  }
  if (!res.ok) throw new Error(`${res.status}: ${await res.text()}`);
  return res.text();
}

// ─── Types ─────────────────────────────────────────────────────────────────

export type StringStatus = "pending" | "translated" | "reviewed" | "approved" | "error";
export type OutputMode = "replace" | "add";

export type FormatStability = "stable" | "experimental" | "comingsoon";

export interface PluginInfo {
  id: string; name: string; description: string;
  extensions: string[]; supported_modes: OutputMode[];
  stability?: FormatStability;
}

export interface ProviderInfo {
  id: string; name: string; is_free: boolean; requires_api_key: boolean;
  /** False when the server knows the provider but it is not registered yet (no API key). */
  configured?: boolean;
}

export interface ProjectInfo {
  path: string; format_id: string; name: string;
  /** From project open; drives Inject modal mode list. */
  supported_modes?: OutputMode[];
}

export interface ProjectOpenResponse {
  format_id: string; format_name: string; total_strings: number;
  project_path: string; project_name: string; supported_modes: OutputMode[];
}

export interface StringEntry {
  id: string; source: string; translation: string | null;
  file_path: string; context: string | null; tags: string[];
  /** Engine extract tags e.g. binary_slot: "utf8" | "utf16le" | "sjis" */
  metadata?: Record<string, unknown>;
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
  /** Ordered fallbacks after the primary (optional; same chain rules as CLI --fallback). */
  fallback_provider_ids?: string[];
  options: {
    source_lang: string; target_lang: string; batch_size: number;
    max_concurrent: number; cost_limit_usd: number | null;
    game_context: string | null; use_glossary: boolean;
    use_memory: boolean; skip_approved: boolean;
  };
}

export interface InjectParams {
  project_path: string;
  format_id: string;
  /** Replace/Add; ignored when `direct` is true. */
  mode?: OutputMode;
  languages: string[];
  output_dir?: string;
  /** In-place inject + injection recording for Patch → Pack (CLI `--direct`). */
  direct?: boolean;
}

export interface MultiLangReport {
  mode: OutputMode | "direct" | string;
  languages_processed: string[];
  languages_failed: [string, string][];
  backup_id: string;
  /** Absolute backup path when direct inject created one. */
  backup_path?: string | null;
  files_modified?: number;
  strings_written?: number;
  strings_skipped?: number;
  warnings?: string[];
  reports: Record<string, any>;
}

/** Mirrors serde for core::models::ValidationKind (externally tagged). */
export type ValidationKind =
  | { MissingPlaceholder: { placeholder: string } }
  | { ExtraPlaceholder: { placeholder: string } }
  | { ExceedsCharLimit: { limit: number; actual: number } }
  | { ExceedsBinarySlot: { encoding: string; limit: number; actual: number } }
  | "EmptyTranslation"
  | "IdenticalToSource";

export interface ValidationIssue {
  entry_id: string;
  kind: ValidationKind;
  message: string;
  /** Optional source snippet (UI); not always present. */
  source?: string | null;
}

export interface ValidationReport {
  total_checked: number;
  issues_found: number;
  entries_with_issues: number;
  /** Counts by kind name, e.g. ExceedsBinarySlot, MissingPlaceholder */
  by_kind: Record<string, number>;
  issues: ValidationIssue[];
}

/** Mirrors core::font_validation::FontCoverageReport */
export interface FontCoverageReport {
  font_path: string;
  font_name: string | null;
  total_unique_chars: number;
  /** JSON chars as single-codepoint strings */
  missing_chars: string[];
  missing_count: number;
  coverage_percent: number;
  has_full_coverage: boolean;
}

export interface ValidationResponse {
  validation: ValidationReport;
  fonts: FontCoverageReport[];
}

/** Human label for a ValidationKind discriminant. */
export function validationKindLabel(kind: ValidationKind): string {
  if (typeof kind === "string") return kind;
  if ("MissingPlaceholder" in kind) return "MissingPlaceholder";
  if ("ExtraPlaceholder" in kind) return "ExtraPlaceholder";
  if ("ExceedsCharLimit" in kind) return "ExceedsCharLimit";
  if ("ExceedsBinarySlot" in kind) return "ExceedsBinarySlot";
  return "Unknown";
}

/** UTF-8 / UTF-16LE byte length of text for inject-slot UI.
 *  JS strings are UTF-16 code units, so utf16le = length × 2.
 *  Shift-JIS needs a native encoder — live UI returns null; full check is Rust `validate`.
 */
export function encodedByteLen(encoding: string, text: string): number | null {
  switch (encoding) {
    case "utf8":
      return new TextEncoder().encode(text).length;
    case "utf16le":
      return text.length * 2;
    case "sjis":
    case "shift_jis":
    case "shift-jis":
      return null;
    default:
      return null;
  }
}

export function binarySlotOf(entry: StringEntry): string | null {
  const v = entry.metadata?.binary_slot;
  return typeof v === "string" ? v : null;
}

export interface ProgressEventStarted { type: "started"; total: number; job_id: string }
export interface ProgressEventBatchCompleted { type: "batch_completed"; completed: number; total: number; cost_so_far: number; language: string | null }
export interface ProgressEventStringTranslated { type: "string_translated"; entry_id: string; translation: string }
export interface ProgressEventCompleted { type: "completed"; total_translated: number; total_cost: number; duration_secs: number }
export interface ProgressEventFailed { type: "failed"; entry_id: string | null; error: string }
export interface ProgressEventProviderSwitched {
  type: "provider_switched";
  provider_id: string;
  provider_name: string;
  remaining_pending: number;
}

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

export interface BatchPatchResult {
  requested: number;
  applied: number;
  skipped: number;
}

/** Bulk update translations in one transaction (search-replace). */
export const batchPatchStrings = (
  updates: { id: string; translation: string }[],
  provider = "manual"
): Promise<BatchPatchResult> =>
  IS_TAURI
    ? invoke("batch_patch_strings", { data: { updates, provider } })
    : request("/strings/batch", {
        method: "POST",
        body: JSON.stringify({ updates, provider }),
      });

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

/** Register a language in RM multi-lang UI (Iavra / VisuMZ / Map choices). */
export interface RegisterLangParams {
  game_path: string;
  lang: string;
  label: string;
}

export interface RegisterLangReport {
  plugins_js: boolean;
  iavra_languages: boolean;
  visumz_options: boolean;
  maps_patched: string[];
  backups: string[];
  notes: string[];
}

export const registerLang = (params: RegisterLangParams): Promise<RegisterLangReport> =>
  IS_TAURI
    ? invoke("register_lang", { params })
    : request("/register-lang", { method: "POST", body: JSON.stringify(params) });

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
export const importPo = (content: string) =>
  request<{ imported: number }>(`/import/po`, {
    method: "POST",
    body: content,
    headers: { "Content-Type": "text/plain" },
  });

export const importXliff = (content: string) =>
  request<{ imported: number }>(`/import/xliff`, {
    method: "POST",
    body: content,
    headers: { "Content-Type": "text/plain" },
  });

export type ExportFormat = "po" | "xliff";

export interface ExportResult {
  path: string;
  format: string;
  lang: string;
  entries: number;
  bytes: number;
}

export interface ImportResult {
  path: string;
  format: string;
  imported: number;
  skipped: number;
}

/** Save translations to a PO or XLIFF file.
 *  Tauri: writes via `export_translations` after the UI picks a path.
 *  Browser: fetches text and triggers a download (path ignored).
 */
export async function exportTranslations(
  format: ExportFormat,
  lang: string,
  path?: string
): Promise<ExportResult> {
  if (IS_TAURI) {
    if (!path) throw new Error("path required for Tauri export");
    return invoke<ExportResult>("export_translations", { format, lang, path });
  }
  const text = format === "po" ? await exportPo(lang) : await exportXliff(lang);
  const filename = path?.split(/[/\\]/).pop() || `translation_${lang}.${format === "po" ? "po" : "xliff"}`;
  const blob = new Blob([text], {
    type: format === "po" ? "text/plain;charset=utf-8" : "application/xml;charset=utf-8",
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
  return { path: filename, format, lang, entries: 0, bytes: text.length };
}

/** Import translations from a PO/XLIFF file path (Tauri) or raw content (browser). */
export async function importTranslations(
  format: ExportFormat,
  pathOrContent: string
): Promise<ImportResult> {
  if (IS_TAURI) {
    return invoke<ImportResult>("import_translations", {
      format,
      path: pathOrContent,
    });
  }
  const content = pathOrContent;
  const res =
    format === "po" ? await importPo(content) : await importXliff(content);
  return {
    path: "(browser upload)",
    format,
    imported: res.imported,
    skipped: 0,
  };
}

// ─── Translation run history ─────────────────────────────────────────────

/** Mirrors core::database::TranslationRun (all ledger columns). */
export interface TranslationRun {
  id: number;
  started_at: string;
  duration_secs: number;
  /** Single provider or chain like "mock→deepl". */
  provider: string;
  source_lang: string;
  target_lang: string;
  strings_translated: number;
  tokens_used: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number;
}

/** Newest-first list of translation runs for the open project. */
export const getTranslationRuns = (): Promise<TranslationRun[]> =>
  request("/runs");

export const getConfig = (): Promise<AppConfig> =>
  IS_TAURI ? invoke("get_config") : request("/config");

export const updateConfig = (partial: Partial<AppConfig>): Promise<AppConfig> =>
  IS_TAURI
    ? invoke("save_config", { partial })
    : request("/config", { method: "PATCH", body: JSON.stringify(partial) });

export const getBackups = (): Promise<BackupEntry[]> =>
  IS_TAURI ? invoke("get_backups") : request("/backups");

export const restoreBackup = (id: string) =>
  request<void>(`/backups/${encodeURIComponent(id)}/restore`, { method: "POST" });

/** Delete a backup by id (HTTP DELETE — uses baseUrl for Tauri port). */
export const deleteBackup = (id: string): Promise<void> =>
  request<void>(`/backups/${encodeURIComponent(id)}`, { method: "DELETE" });

// ─── Patch apply / rollback (HTTP — server embeds the core engine) ────────

export interface PatchPathsParams {
  game_path: string;
  zip_path?: string;
  /** http(s) URL of a patch zip — server downloads then applies/verifies. */
  zip_url?: string;
  force?: boolean;
  confirm_legacy?: boolean;
  dry_run?: boolean;
}

export interface PatchVerifyResult {
  outcome: string;
  tier: string | null;
  replaced: string[];
  added: string[];
  conflicts: string[];
  backup_compromised: boolean;
  messages: string[];
  manifest: any;
}

export interface PatchApplyResult {
  patch_id: string;
  patch_version: string;
  replaced: number;
  added: number;
  forced: boolean;
  baseline: string;
  dry_run: boolean;
  user_edits_overwritten: string[];
  messages: string[];
}

export interface PatchRollbackResult {
  restored: number;
  deleted: number;
  baseline: string | null;
  messages: string[];
  aborted_edited: string[];
  torn_deleted: string[];
}

export interface PatchStatusResult {
  status: "not_patched" | "patched" | "interrupted" | "unknown";
  patch_id?: string;
  patch_version?: string;
  engine?: string;
  language?: string;
  baseline?: string;
  forced?: boolean;
  applied_at?: string;
  replaced?: number;
  added?: number;
  state?: string;
}

export const patchVerify = (params: PatchPathsParams): Promise<PatchVerifyResult> =>
  request("/patch/verify", { method: "POST", body: JSON.stringify(params) });

export const patchApply = (params: PatchPathsParams): Promise<PatchApplyResult> =>
  request("/patch/apply", { method: "POST", body: JSON.stringify(params) });

export const patchRollback = (params: PatchPathsParams): Promise<PatchRollbackResult> =>
  request("/patch/rollback", { method: "POST", body: JSON.stringify(params) });

export const patchStatus = (params: Pick<PatchPathsParams, "game_path">): Promise<PatchStatusResult> =>
  request("/patch/status", { method: "POST", body: JSON.stringify(params) });

export interface PatchPackParams {
  game_path: string;
  output_path: string;
  /** At most one language; empty = auto when a single recording exists. */
  languages?: string[];
  /** Require pristine hashes (.locust/backup or pristine_path). */
  pristine?: boolean;
  pristine_path?: string;
}

export interface PatchPackResult {
  output_path: string;
  recording_lang: string | null;
  recorded_root: string;
  files_packed: number;
  translated_strings: number;
  size_bytes: number;
  patch_id: string;
  patch_version: string;
  engine: string;
  language: string;
  tier: string;
  messages: string[];
}

export const patchPack = (params: PatchPackParams): Promise<PatchPackResult> =>
  request("/patch/pack", { method: "POST", body: JSON.stringify(params) });

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
