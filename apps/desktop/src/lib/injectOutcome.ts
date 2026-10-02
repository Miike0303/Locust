/**
 * Classify an inject HTTP/Tauri report so the UI never celebrates a no-op.
 * Engine/CLI already surface zero-write and failed languages; the modal used to
 * toast success on any non-throwing response.
 */

export type InjectOutcomeKind = "success" | "partial" | "empty" | "unchanged";

export type InjectLangReport = {
  skip_reasons?: Record<string, number>;
  strings_written?: number;
  files_modified?: number;
  strings_skipped?: number;
  warnings?: string[];
  files_written?: string[];
};

/** Per-language detail avoids counting a direct report twice. Old engines
 * retain an explicit unclassified remainder instead of guessing its cause. */
export function collectSkipReasons(report: InjectReportLike): Array<{lang: string; reason: string; count: number}> {
  const out: Array<{lang: string; reason: string; count: number}> = [];
  const sources = report.reports && Object.keys(report.reports).length > 0
    ? Object.entries(report.reports) : [["", report] as const];
  for (const [lang, details] of sources) {
    if (!details) continue;
    let remaining = asNonNegIntAllowZero(details.strings_skipped);
    for (const [reason, value] of Object.entries(details.skip_reasons ?? {})) {
      const count = Math.min(remaining, asNonNegIntAllowZero(value));
      if (count > 0) out.push({ lang, reason, count });
      remaining -= count;
    }
    if (remaining > 0) {
      const unclassified = out.find(row => row.lang === lang && row.reason === "unclassified");
      if (unclassified) unclassified.count += remaining;
      else out.push({lang, reason: "unclassified", count: remaining});
    }
  }
  return out;
}

/** Serde externally-tagged RecordOutcome from core. */
export type RecordOutcomeJson =
  | { Recorded: { files: number } }
  | { KeptPrevious: { recorded_at: string } }
  | "NothingRecorded";

export type InjectReportLike = {
  skip_reasons?: Record<string, number>;
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

type ReportCount = "strings_written" | "files_modified" | "strings_skipped";

/** Direct reports carry top-level counts; multi-language (Add/Replace)
 * reports carry them only per language, so fall back to the sum. */
function sumReportCount(report: InjectReportLike, field: ReportCount): number {
  const top = report[field];
  if (typeof top === "number" && Number.isFinite(top)) {
    return asNonNegIntAllowZero(top);
  }
  let sum = 0;
  if (report.reports) {
    for (const r of Object.values(report.reports)) {
      const value = r?.[field];
      if (typeof value === "number") {
        sum += asNonNegIntAllowZero(value);
      }
    }
  }
  return sum;
}

export function sumStringsWritten(report: InjectReportLike): number {
  return sumReportCount(report, "strings_written");
}

export function sumFilesModified(report: InjectReportLike): number {
  return sumReportCount(report, "files_modified");
}

export function sumStringsSkipped(report: InjectReportLike): number {
  return sumReportCount(report, "strings_skipped");
}

export function collectInjectWarnings(report: InjectReportLike): string[] {
  const out: string[] = [];
  if (Array.isArray(report.warnings)) {
    for (const w of report.warnings) {
      if (typeof w === "string" && w.trim()) out.push(w);
    }
  }
  const topLevel = new Set(out);
  if (report.reports) {
    for (const [lang, r] of Object.entries(report.reports)) {
      if (!r || !Array.isArray(r.warnings)) continue;
      for (const w of r.warnings) {
        if (typeof w === "string" && w.trim() && !topLevel.has(w)) {
          out.push(`${lang}: ${w}`);
        }
      }
    }
  }
  return [...new Set(out)];
}

/** Compatibility with the core's successful journal-retention diagnostic.
 * Only this exact notice is informational; unknown diagnostics stay warnings. */
export function injectionRecoveryPath(message: string): string | null {
  return /^(?:[A-Za-z0-9_-]+: )?injection [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12} completed; verified originals retained at (.+)$/.exec(message)?.[1] ?? null;
}

/** Exact backend notice, not a general suppression of backup diagnostics. */
export function isRedundantBackupRemoved(message: string): boolean {
  return /^(?:[A-Za-z0-9_-]+: )?unchanged injection: redundant new backup removed$/.test(message);
}

function isVerifiedUnchanged(report: InjectReportLike): boolean {
  const langs = report.languages_processed ?? [];
  if (!langs.length || new Set(langs).size !== langs.length || report.languages_failed?.length) return false;
  if (report.strings_written !== undefined && report.strings_written !== 0) return false;
  if (report.files_modified !== undefined && report.files_modified !== 0) return false;
  if (collectFilesWritten(report).length) return false;
  if (collectInjectWarnings(report).some(w => !injectionRecoveryPath(w) && !isRedundantBackupRemoved(w))) return false;
  const details = report.reports && Object.keys(report.reports).length
    ? report.reports : langs.length === 1 ? { [langs[0]]: report } : {};
  if (Object.keys(details).length !== langs.length) return false;
  let skipped = 0;
  for (const lang of langs) {
    const r = details[lang];
    if (!r || r.strings_written !== 0 || r.files_modified !== 0) return false;
    const count = r.strings_skipped;
    if (typeof count !== "number" || !Number.isSafeInteger(count) || count <= 0) return false;
    if (r.skip_reasons?.unchanged !== count) return false;
    if (Object.entries(r.skip_reasons).some(([reason, n]) => reason !== "unchanged" && n !== 0)) return false;
    skipped += count;
  }
  if (report.strings_skipped !== undefined && report.strings_skipped !== skipped) return false;
  if (report.outcomes && (report.outcomes.length !== langs.length ||
    new Set(report.outcomes.map(([lang]) => lang)).size !== langs.length ||
    report.outcomes.some(([lang, outcome]) => !langs.includes(lang) ||
      !(isNothingRecorded(outcome) || isKeptPrevious(outcome))))) return false;
  return true;
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
 * - unchanged: explicit, complete evidence that every candidate already matches
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
    if (isVerifiedUnchanged(report)) return "unchanged";
    return "empty";
  }
  if (failed > 0) return "partial";
  if (hasRecordingIssue) return "partial";
  if (collectSkipReasons(report).some(r => !["unchanged", "duplicate", "untranslated"].includes(r.reason))) return "partial";
  return "success";
}

export type ToastLevel = "success" | "warning" | "error" | "info";

export function injectToastLevel(kind: InjectOutcomeKind): ToastLevel {
  switch (kind) {
    case "unchanged":
      return "info";
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
  if (sumStringsWritten(report) === 0 || classifyInjectReport(report) === "empty") return false;
  const issues = outcomeRecordingIssues(report);
  if (issues.length === 0) return true;
  // All keys nothing-recorded → packing will refuse.
  return !issues.every((i) => i.kind === "nothing");
}

// Keep unused import surface honest for call sites that only need counts.
export function failedLanguageCount(report: InjectReportLike): number {
  return Array.isArray(report.languages_failed) ? report.languages_failed.length : 0;
}
