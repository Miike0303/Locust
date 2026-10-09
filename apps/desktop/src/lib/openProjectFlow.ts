import { isTauri } from "./runtime";
import type { QueryClient } from "@tanstack/react-query";
import type { ProjectInfo, ProjectOpenPreflight, ProjectOpenResponse } from "./api";
import { t } from "./i18n";
import type { TranslateFn } from "./i18n";

/** Backend detection failure (Tauri: "Could not detect game format"; HTTP 422: "format not detected"). */
export function isDetectionFailure(msg: string): boolean {
  return /detect/i.test(msg) && /format/i.test(msg);
}

export function projectFromOpenResponse(result: ProjectOpenResponse): ProjectInfo {
  return {
    path: result.project_path,
    format_id: result.format_id,
    name: result.project_name,
    supported_modes: result.supported_modes,
    database_path: result.database_path || undefined,
    extraction_warnings: result.extraction_warnings ?? [],
    persistence_warning: result.persistence_warning,
  };
}

/** Prefer open-db when a recent entry (or picker) names a project database. */
export function shouldOpenProjectDb(databasePath: string | null | undefined): boolean {
  return typeof databasePath === "string" && /\.locust\.db$/i.test(databasePath.trim());
}

export function projectOpenHttpBody(
  path: string,
  formatId?: string,
  preferSaved = false,
): { path: string; format_id?: string; prefer_saved?: true } {
  return preferSaved
    ? { path, format_id: formatId, prefer_saved: true }
    : { path, format_id: formatId };
}

export function projectOpenTauriArgs(
  path: string,
  formatId?: string,
  preferSaved = false,
): { path: string; formatId?: string; preferSaved?: true } {
  return preferSaved
    ? { path, formatId, preferSaved: true }
    : { path, formatId };
}

/** Ready to call open-db: real .locust.db, non-empty game folder, concrete format (not auto). */
export function openDbCanConfirm(
  databasePath: string,
  gamePath: string,
  formatId: string,
): boolean {
  return (
    shouldOpenProjectDb(databasePath) &&
    gamePath.trim().length > 0 &&
    formatId.trim().length > 0 &&
    formatId !== "auto"
  );
}

export const PROJECT_QUERY_KEYS = [
  "strings",
  "stats",
  "string",
  "review-strings",
  "string-facets",
] as const;

export function dropProjectQueries(queryClient: QueryClient): void {
  for (const key of PROJECT_QUERY_KEYS) {
    void queryClient.removeQueries({ queryKey: [key] });
  }
}

export async function pickGameFolder(t: TranslateFn): Promise<string | null> {
  if (isTauri()) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selected = await open({
      title: t("welcome.dialog.selectFolder"),
      directory: true,
    });
    return typeof selected === "string" ? selected : null;
  }
  return prompt(t("welcome.prompt.folderPath"));
}

/**
 * Run a Welcome open action whose native file/folder dialog may reject.
 * A rejection is reported through `onFailed` instead of escaping as an
 * unhandled promise rejection; the returned promise never rejects.
 */
export async function runOpenAction(
  action: () => Promise<void>,
  onFailed: (message: string) => void,
): Promise<void> {
  try {
    await action();
  } catch (err: unknown) {
    onFailed(err instanceof Error ? err.message : String(err));
  }
}

/** Pick an existing `.locust.db` (CLI extract / pivot) without opening a bare game folder. */
export type PickLocustDbResult =
  | { status: "picked"; path: string }
  | { status: "cancelled" }
  | { status: "invalid" };

export async function pickLocustDbFile(t: TranslateFn): Promise<PickLocustDbResult> {
  if (isTauri()) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selected = await open({
      title: t("welcome.dialog.selectDb"),
      filters: [
        {
          name: t("welcome.dialog.locustDb"),
          extensions: ["db"],
        },
        { name: t("welcome.dialog.allFiles"), extensions: ["*"] },
      ],
    });
    if (typeof selected !== "string") return { status: "cancelled" };
    if (!shouldOpenProjectDb(selected)) return { status: "invalid" };
    return { status: "picked", path: selected };
  }
  const typed = prompt(t("welcome.prompt.dbPath"));
  if (typed === null) return { status: "cancelled" };
  if (!shouldOpenProjectDb(typed)) return { status: "invalid" };
  return { status: "picked", path: typed.trim() };
}

export type ProjectOpenChoice = "resume" | "refresh" | "cancel";
export interface ProjectOpenChoiceRequest {
  gamePath: string;
  preflight: Exclude<ProjectOpenPreflight, { kind: "extract" }>;
}

// One app-wide choice host also serves store-driven queue opening. A second
// caller cancels rather than replacing somebody else's unanswered decision.
let choiceSequence = 0;
let pendingChoice: (ProjectOpenChoiceRequest & { id: number }) | null = null;
let settleChoice: ((choice: ProjectOpenChoice) => void) | null = null;
const choiceListeners = new Set<() => void>();
export const getProjectOpenChoice = () => pendingChoice;
export function subscribeProjectOpenChoice(listener: () => void): () => void {
  choiceListeners.add(listener);
  return () => { choiceListeners.delete(listener); };
}
export function resolveProjectOpenChoice(request: ProjectOpenChoiceRequest, choice: ProjectOpenChoice): void {
  if (pendingChoice === request) settleChoice?.(choice);
}

export function requestProjectOpenChoice(
  request: ProjectOpenChoiceRequest,
  signal?: AbortSignal,
): Promise<ProjectOpenChoice> {
  if (pendingChoice || signal?.aborted) return Promise.resolve("cancel");
  return new Promise((resolve) => {
    const cancel = () => settleChoice?.("cancel");
    pendingChoice = { ...request, id: ++choiceSequence };
    settleChoice = (choice) => {
      signal?.removeEventListener("abort", cancel);
      pendingChoice = null;
      settleChoice = null;
      for (const listener of choiceListeners) listener();
      resolve(choice);
    };
    signal?.addEventListener("abort", cancel, { once: true });
    for (const listener of choiceListeners) listener();
  });
}

export interface FolderOpenOptions {
  choose?: (request: ProjectOpenChoiceRequest, signal?: AbortSignal) => Promise<ProjectOpenChoice>;
  signal?: AbortSignal;
}

/** Folder opening only. Explicit DB and legacy recent opening bypass this flow. */
export async function openFolderProject(
  path: string,
  formatId?: string,
  options: FolderOpenOptions = {},
): Promise<ProjectOpenResponse | null> {
  if (options.signal?.aborted) return null;
  const { preflightProjectOpen, openProject, resumeProject } = await import("./api");
  const preflight = await preflightProjectOpen(path, formatId);
  if (options.signal?.aborted) return null;
  if (preflight.kind === "extract") return openProject(path, formatId);
  if (preflight.kind !== "resume_available" && preflight.kind !== "needs_attention") {
    throw new Error(t("resume.invalidPreflight"));
  }
  const choice = await (options.choose ?? requestProjectOpenChoice)({ gamePath: path, preflight }, options.signal);
  if (options.signal?.aborted || choice === "cancel") return null;
  if (choice === "refresh") return openProject(path, formatId);
  // An erroneous/custom chooser cannot promote unverified evidence to resume.
  if (choice !== "resume" || preflight.kind !== "resume_available") return null;
  return resumeProject(preflight.database_path, preflight.project_path, preflight.format_id);
}

export async function completeOpenProject(
  path: string,
  formatId: string | undefined,
  deps: {
    setProject: (p: ProjectInfo) => void;
    queryClient: QueryClient;
  } & FolderOpenOptions,
  preferSaved = false,
): Promise<ProjectOpenResponse | null> {
  const { openProject } = await import("./api");
  const result = preferSaved
    ? await openProject(path, formatId, true)
    : await openFolderProject(path, formatId, deps);
  if (!result) return null;
  deps.setProject(projectFromOpenResponse(result));
  dropProjectQueries(deps.queryClient);
  return result;
}

/** Open an existing .locust.db without extract/merge (pivot, CLI db, recents). */
export async function completeOpenProjectDb(
  databasePath: string,
  gamePath: string,
  formatId: string,
  deps: {
    setProject: (p: ProjectInfo) => void;
    queryClient: QueryClient;
  },
): Promise<ProjectOpenResponse> {
  const { openProjectDb } = await import("./api");
  const result = await openProjectDb(databasePath, gamePath, formatId);
  deps.setProject(projectFromOpenResponse(result));
  dropProjectQueries(deps.queryClient);
  return result;
}

export type FormatPickerLocationState = {
  formatPickerPath?: string;
};

export function formatPickerPathFromState(state: unknown): string | null {
  if (!state || typeof state !== "object") return null;
  const path = (state as FormatPickerLocationState).formatPickerPath;
  return typeof path === "string" && path.trim() ? path : null;
}
