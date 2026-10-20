import type { ProjectInfo } from "./api";

export interface SpeakerOwner {
  database_path: string;
  project_path: string;
  format_id: string;
}
export interface SpeakerGroup {
  id: string;
  name: string;
  count: number;
  entry_ids: string[];
  note: string;
}
export interface SpeakerGroups {
  groups: SpeakerGroup[];
  unassigned_entry_ids: string[];
}

/** Echo current-project identity verbatim; never infer a database filename. */
export function speakerOwner(project: ProjectInfo | null): SpeakerOwner | null {
  return project?.database_path ? {
    database_path: project.database_path,
    project_path: project.path,
    format_id: project.format_id,
  } : null;
}
