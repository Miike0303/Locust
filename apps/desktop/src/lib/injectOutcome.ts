/**
 * Classify an inject HTTP/Tauri report so the UI never celebrates a no-op.
 * Engine/CLI already surface zero-write and failed languages; the modal used to
 * toast success on any non-throwing response.
 */

export type InjectOutcomeKind = "success" | "partial" | "empty";

export type InjectLangReport = {
  strings_written?: number;
  files_modified?: number;
  strings_skipped?: number;
  warnings?: string[];
  files_written?: string[];
};

/** Serde externally-tagged RecordOutcome from core. */
export type RecordOutcomeJson =
  | { Recorded: { files: number } }
  | { KeptPrevious: { recorded_at: string } }
  | "NothingRecorded";

export type InjectReportLike = {
  languages_processed?: string[];
  languages_failed?: [string, string][] | Array<[string, string]>;
  strings_written?: number;
  files_modified?: number;
  strings_skipped?: number;
  warnings?: string[];
  files_written?: string[];
  reports?: Record<string, InjectLangReport | undefined>;
  outcomes?: Array<[string, RecordOutcomeJson]> | Array<[string, unknown]>;
};

function asNonNegIntAllowZero(n: unknown): number {
  return typeof n === "number" && Number.isFinite(n) && n >= 0 ? Math.trunc(n) : 0;
}

export function sumStringsWritten(report: InjectReportLike): number {
  if (typeof report.strings_written === "number" && Number.isFinite(report.strings_written)) {
    return asNonNegIntAllowZero(report.strings_written);
  }
  let sum = 0;
  if (report.reports) {
    for (const r of Object.values(report.reports)) {
      if (r && typeof r.strings_written === "number") {
        sum += asNonNegIntAllowZero(r.strings_written);
      }
    }
  }
  return sum;
}

export function collectInjectWarnings(report: InjectReportLike): string[] {
  const out: string[] = [];
  if (Array.isArray(report.warnings)) {
    for (const w of report.warnings) {
      if (typeof w === "string" && w.trim()) out.push(w);
    }
  }
  if (report.reports) {
    for (const [lang, r] of Object.entries(report.reports)) {
      if (!r || !Array.isArray(r.warnings)) continue;
      for (const w of r.warnings) {
        if (typeof w === "string" && w.trim()) {
          out.push(`${lang}: ${w}`);
        }
      }
    }
  }
  return out;
}

export function collectFilesWritten(report: InjectReportLike): string[] {
  const out: string[] = [];
  const pushPath = (p: unknown) => {
    if (typeof p === "string" && p.trim()) out.push(p);
  };
  if (Array.isArray(report.files_written)) {
    for (const p of report.files_written) pushPath(p);
  }
  if (report.reports) {
    for (const r of Object.values(report.reports)) {
      if (!r || !Array.isArray(r.files_written)) continue;
      for (const p of r.files_written) pushPath(p);
    }
  }
  return [...new Set(out)];
}

function isNothingRecorded(o: unknown): boolean {
  return o === "NothingRecorded";
}

function isKeptPrevious(o: unknown): boolean {
  return (
    typeof o === "object" &&
    o !== null &&
    "KeptPrevious" in (o as object)
  );
}

export function outcomeRecordingIssues(
  report: InjectReportLike,
): Array<{ lang: string; kind: "nothing" | "kept"; detail?: string }> {
  const issues: Array<{ lang: string; kind: "nothing" | "kept"; detail?: string }> = [];
  if (!Array.isArray(report.outcomes)) return issues;
  for (const row of report.outcomes) {
    if (!Array.isArray(row) || row.length < 2) continue;
    const lang = String(row[0] ?? "");
    const outcome = row[1];
    if (isNothingRecorded(outcome)) {
      issues.push({ lang, kind: "nothing" });
    } else if (isKeptPrevious(outcome)) {
      const kp = outcome as { KeptPrevious: { recorded_at?: string } };
      issues.push({
        lang,
        kind: "kept",
        detail: kp.KeptPrevious?.recorded_at,
      });
    }
  }
  return issues;
}

/**
 * - empty: nothing useful written (all failed, or zero strings across the run)
 * - partial: some languages failed, or recording kept/nothing while some text landed
 * - success: at least one string written and no language failures
 */
export function classifyInjectReport(report: InjectReportLike): InjectOutcomeKind {
  const failed = Array.isArray(report.languages_failed)
    ? report.languages_failed.length
    : 0;
  const processed = Array.isArray(report.languages_processed)
    ? report.languages_processed.length
    : 0;
  const written = sumStringsWritten(report);
  const recording = outcomeRecordingIssues(report);
  const hasRecordingIssue = recording.length > 0;

  if (processed === 0 && failed > 0) return "empty";
  if (written === 0) {
    if (failed > 0 && processed > 0) return "partial";
    return "empty";
  }
  if (failed > 0) return "partial";
  if (hasRecordingIssue) return "partial";
  return "success";
}

export type ToastLevel = "success" | "warning" | "error";

export function injectToastLevel(kind: InjectOutcomeKind): ToastLevel {
  switch (kind) {
    case "success":
      return "success";
    case "partial":
      return "warning";
    case "empty":
      return "error";
  }
}

/** Pack CTA only when direct inject actually recorded usable work. */
export function shouldOfferPackAfterInject(report: InjectReportLike): boolean {
  if (classifyInjectReport(report) === "empty") return false;
  const issues = outcomeRecordingIssues(report);
  if (issues.length === 0) return true;
  // All keys nothing-recorded → packing will refuse.
  return !issues.every((i) => i.kind === "nothing");
}

// Keep unused import surface honest for call sites that only need counts.
export function failedLanguageCount(report: InjectReportLike): number {
  return Array.isArray(report.languages_failed) ? report.languages_failed.length : 0;
}
