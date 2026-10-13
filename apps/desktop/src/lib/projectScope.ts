import type { ProjectInfo } from "./api";

/** Shared identity for editor caches and drafts; different databases never share rows. */
export function draftProjectKey(project: ProjectInfo): string {
  return JSON.stringify([project.database_path ?? null, project.path, project.format_id]);
}
