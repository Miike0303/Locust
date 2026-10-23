import type { ValidationSummary } from "./api";

export const QA_PAGE_SIZE = 50;

export const qaLastRunKey = (projectKey: string) => ["validation-last-run", projectKey] as const;

/** Issue rows prove prior validation, but an empty summary carries no run metadata. */
export function qaHasBeenChecked(summary: ValidationSummary, lastCheckedAt: number | null): boolean {
  return lastCheckedAt !== null || Object.values(summary).some(counts => counts.open + counts.ignored > 0);
}

export function buildQaSummary(summary: ValidationSummary, selectedKind?: string) {
  // Keep an active filter reachable even after its final issue disappears.
  const kinds = [...new Set([...Object.keys(summary), ...(selectedKind ? [selectedKind] : [])])].sort();
  return {
    open: Object.values(summary).reduce((total, counts) => total + counts.open, 0),
    kinds: kinds.map(kind => ({ kind, ...(summary[kind] ?? { open: 0, ignored: 0 }) })),
  };
}

export function qaRelativePath(filePath: string, gamePath: string): string {
  const path = filePath.replace(/\\/g, "/");
  const game = gamePath.replace(/\\/g, "/").replace(/\/+$/, "");
  const root = game + "/";
  const windows = /^[a-z]:\//i.test(root) || root.startsWith("//");
  // A project can itself be a single game file rather than a directory.
  if (windows ? path.toLowerCase() === game.toLowerCase() : path === game) return path.split("/").pop() || path;
  const inside = windows ? path.toLowerCase().startsWith(root.toLowerCase()) : path.startsWith(root);
  return inside ? path.slice(root.length) : path;
}

/** Clamp after a run or ignore action removes the last row of a later page. */
export function qaPagination(offset: number, total: number) {
  const pages = Math.max(1, Math.ceil(total / QA_PAGE_SIZE));
  const safeOffset = Math.min(offset, (pages - 1) * QA_PAGE_SIZE);
  return {
    offset: safeOffset,
    from: total === 0 ? 0 : safeOffset + 1,
    to: Math.min(safeOffset + QA_PAGE_SIZE, total),
    page: Math.floor(safeOffset / QA_PAGE_SIZE) + 1,
    pages,
  };
}
