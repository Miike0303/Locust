import type { QueryClient } from "@tanstack/react-query";
import type { ProjectInfo, ProjectOpenResponse } from "./api";
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
  };
}

/** Prefer open-db when a recent entry (or picker) names a project database. */
export function shouldOpenProjectDb(databasePath: string | null | undefined): boolean {
  return typeof databasePath === "string" && /\.locust\.db$/i.test(databasePath.trim());
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
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selected = await open({
      title: t("welcome.dialog.selectFolder"),
      directory: true,
    });
    return typeof selected === "string" ? selected : null;
  }
  return prompt(t("welcome.prompt.folderPath"));
}

export async function completeOpenProject(
  path: string,
  formatId: string | undefined,
  deps: {
    setProject: (p: ProjectInfo) => void;
    queryClient: QueryClient;
  },
): Promise<ProjectOpenResponse> {
  const { openProject } = await import("./api");
  const result = await openProject(path, formatId);
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
