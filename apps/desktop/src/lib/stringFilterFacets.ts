import type { StringFilter } from "./api";

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

/** Drop blank facet values; the server already returns distinct sorted lists. */
export function facetOptions(values: readonly string[] | undefined): string[] {
  if (!values) return [];
  return values.map((value) => value.trim()).filter(Boolean);
}
