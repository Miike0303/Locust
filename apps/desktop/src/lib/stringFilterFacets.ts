import type { StringFilter } from "./api";

interface FacetEntry {
  file_path: string;
  tags: string[];
}

export function uniqueSortedFilePaths(entries: readonly FacetEntry[]): string[] {
  return [...new Set(entries.map((entry) => entry.file_path.trim()).filter(Boolean))].sort();
}

export function uniqueSortedTags(entries: readonly FacetEntry[]): string[] {
  return [
    ...new Set(
      entries.flatMap((entry) => entry.tags.map((tag) => tag.trim())).filter(Boolean)
    ),
  ].sort();
}

export function coerceExactFilterValue(value: string): string | undefined {
  return value.trim() || undefined;
}

export function filePathFilterPatch(value: string): Partial<StringFilter> {
  return { file_path: coerceExactFilterValue(value), offset: 0 };
}

export function tagFilterPatch(value: string): Partial<StringFilter> {
  return { tag: coerceExactFilterValue(value), offset: 0 };
}

export function filePathOptionLabel(path: string): string {
  return path.split(/[/\\]/).pop() || path;
}
