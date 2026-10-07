import type { PatchPackParams } from "./api";

/** Keep the destination's separator style, including Windows paths on the web. */
export function defaultEntryPath(zipPath: string): string {
  const path = zipPath.trim();
  if (!path) return "";
  const separator = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const dot = path.lastIndexOf(".");
  return `${dot > separator + 1 ? path.slice(0, dot) : path}.md`;
}

export function publishCommand(zipPath: string, entryPath: string): string {
  const quote = (path: string) => `"${path.replaceAll('"', '\\"')}"`;
  return `npm run publish-patch -- ${quote(zipPath)} ${quote(entryPath)}`;
}

export interface PatchPublishOptions {
  rjCode: string;
  gameVersion: string;
  detectId: boolean;
  createEntry: boolean;
  entryPath: string;
}

export function patchPublishFields(options: PatchPublishOptions): Pick<
  PatchPackParams, "rj_code" | "game_version" | "detect_id" | "entry_path"
> {
  const fields: ReturnType<typeof patchPublishFields> = { detect_id: options.detectId };
  if (options.rjCode.trim()) fields.rj_code = options.rjCode.trim();
  if (options.gameVersion.trim()) fields.game_version = options.gameVersion.trim();
  if (options.createEntry) fields.entry_path = options.entryPath.trim();
  return fields;
}
