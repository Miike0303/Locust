import type { ProjectInfo, ProjectOpenResponse, ProjectStats } from "./api";

/** POST /api/project/open-db — never POST /api/project/open (that extracts). */
export const PIVOT_OPEN_DB_HTTP_PATH = "/project/open-db";
/** Tauri command — never `open_project` (that takes a game path). */
export const PIVOT_OPEN_DB_TAURI_CMD = "open_project_db";

export type PivotOpenDbArgs = {
  databasePath: string;
  gamePath: string;
  formatId: string;
};

/**
 * A pivoted .locust.db has no game of its own. Open-db needs the game folder
 * and format from the project the user pivoted from — not the database path.
 */
export function pivotOpenDbArgs(
  databasePath: string,
  sourceProject: Pick<ProjectInfo, "path" | "format_id">,
): PivotOpenDbArgs {
  return {
    databasePath,
    gamePath: sourceProject.path,
    formatId: sourceProject.format_id,
  };
}

export function pivotOpenDbTauriArgs(args: PivotOpenDbArgs): PivotOpenDbArgs {
  return {
    databasePath: args.databasePath,
    gamePath: args.gamePath,
    formatId: args.formatId,
  };
}

export function pivotOpenDbHttpBody(args: PivotOpenDbArgs): {
  database_path: string;
  game_path: string;
  format_id: string;
} {
  return {
    database_path: args.databasePath,
    game_path: args.gamePath,
    format_id: args.formatId,
  };
}

export function pivotCarryOverCount(
  stats: Pick<ProjectStats, "translated" | "reviewed" | "approved"> | null | undefined,
): number {
  if (!stats) return 0;
  return stats.translated + stats.reviewed + stats.approved;
}

export function defaultPivotFileName(projectName: string): string {
  const stem = projectName.trim() || "project";
  return `${stem}-pivot.locust.db`;
}

export function isExistingOutputError(message: string): boolean {
  const m = message.toLowerCase();
  return /already exists|file exists|output (file )?exists|eexist/.test(m);
}

export function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Keep the game folder when open_project was pointed at a .locust.db file. */
export function projectInfoAfterPivotOpen(
  previous: ProjectInfo,
  opened: ProjectOpenResponse,
): ProjectInfo {
  const openedLooksLikeDb = /\.locust\.db$/i.test(opened.project_path);
  return {
    path: openedLooksLikeDb ? previous.path : opened.project_path,
    format_id: opened.format_id || previous.format_id,
    name: opened.project_name || previous.name,
    supported_modes: opened.supported_modes?.length
      ? opened.supported_modes
      : previous.supported_modes,
  };
}
