import type { PatchVerifyResult } from "./api";
import type { TranslateFn } from "./i18n";

/** The current HTTP endpoint returns a Rust Debug outcome; keep it out of UI copy. */
function debugString(value: string): string | null {
  try {
    return JSON.parse(value.replace(/\\u\{([0-9a-f]+)\}/gi, (_, hex: string) =>
      JSON.stringify(String.fromCodePoint(parseInt(hex, 16))).slice(1, -1),
    ).replace(/\\0/g, "\\u0000"));
  } catch {
    return null;
  }
}

export function patchVerificationSummary(report: PatchVerifyResult, t: TranslateFn): {
  severity: "success" | "warning";
  summary: string;
  files: string[];
} {
  const mismatches = new Map<string, "exists" | "missing" | "changed">();
  if (report.outcome.startsWith("Mismatch(")) {
    const quoted = String.raw`"(?:[^"\\]|\\.)*"`;
    const fields = new RegExp(String.raw`FileMismatch \{ path: (${quoted}), expected: (${quoted}), found: (${quoted}) \}`, "g");
    for (const match of report.outcome.matchAll(fields)) {
      const path = debugString(match[1]);
      const expected = debugString(match[2]);
      const found = debugString(match[3]);
      if (path !== null && expected !== null && found !== null) {
        const missing = found === "absent" || found === "missing";
        mismatches.set(path, expected === "absent (added path)" ? "exists" : missing ? "missing" : "changed");
      }
    }
  }
  for (const path of report.conflicts ?? []) {
    if (!mismatches.has(path)) mismatches.set(path, "exists");
  }
  const files = Array.from(mismatches, ([path, reason]) => t(`patch.verify.file.${reason}`, { path }));
  if (files.length > 0) {
    return { severity: "warning", summary: t("patch.verify.conflicts", { count: files.length }), files };
  }
  if (report.outcome.startsWith("Mismatch(")) {
    return { severity: "warning", summary: t("patch.verify.mismatch"), files };
  }
  if (report.outcome.startsWith("DowngradeBlocked")) {
    return { severity: "warning", summary: t("api.error.patchDowngradeBlocked"), files };
  }
  if (report.outcome === "Interrupted") {
    return { severity: "warning", summary: t("api.error.patchInterrupted"), files };
  }
  const known = {
    Clean: "patch.value.clean",
    AlreadyApplied: "patch.value.alreadyApplied",
    UpgradeAvailable: "patch.value.upgradeAvailable",
  } as const;
  const key = Object.prototype.hasOwnProperty.call(known, report.outcome)
    ? known[report.outcome as keyof typeof known] : null;
  return {
    severity: key && !report.backup_compromised ? "success" : "warning",
    summary: key ? t("patch.verifyLine", { outcome: t(key) }) : t("patch.verify.unknown"),
    files,
  };
}
