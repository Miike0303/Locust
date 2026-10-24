import type { PatchVerifyResult } from "./api";
import type { TranslateKey, Vars } from "./i18n";

type ConflictReasonKey = `patch.verify.reason.${"exists" | "missing" | "changed" | "unknown"}`;

export interface PatchVerificationSummary {
  severity: "success" | "warning" | "error";
  summaryKey: TranslateKey;
  params: Vars;
  conflicts: { path: string; reasonKey: ConflictReasonKey }[];
  technical: string[];
}

/** The HTTP API currently serializes outcomes with Rust Debug, not JSON. */
function debugString(value: string): string | null {
  try {
    // Consume each escape once so an escaped backslash stays literal.
    return JSON.parse(value.replace(/\\(?:u\{([0-9a-f]+)\}|(0)|.)/gi, (escape, hex: string | undefined, nul: string | undefined) => {
      if (hex) return JSON.stringify(String.fromCodePoint(parseInt(hex, 16))).slice(1, -1);
      return nul ? "\\u0000" : escape;
    }));
  } catch {
    return null;
  }
}

function conflictReason(expected: string, found: string): ConflictReasonKey {
  // These are the backend's current reason sentinels; unfamiliar diagnostics
  // stay generic in translated copy and remain intact in technical details.
  if (expected === "absent (added path)") return "patch.verify.reason.exists";
  if (found === "absent" || found === "missing") return "patch.verify.reason.missing";
  if (expected.startsWith("original=") && expected.includes(" or patched=")) {
    return "patch.verify.reason.changed";
  }
  return "patch.verify.reason.unknown";
}

export function patchVerificationSummary(report: PatchVerifyResult): PatchVerificationSummary {
  const reasons = new Map<string, ConflictReasonKey>();
  if (report.outcome.startsWith("Mismatch(")) {
    const quoted = String.raw`"(?:[^"\\]|\\.)*"`;
    const fields = new RegExp(String.raw`FileMismatch \{ path: (${quoted}), expected: (${quoted}), found: (${quoted}) \}`, "g");
    for (const match of report.outcome.matchAll(fields)) {
      const path = debugString(match[1]);
      const expected = debugString(match[2]);
      const found = debugString(match[3]);
      if (path !== null && expected !== null && found !== null && !reasons.has(path)) {
        reasons.set(path, conflictReason(expected, found));
      }
    }
  }
  // This API field specifically means added paths already occupied by other content.
  for (const path of report.conflicts ?? []) {
    if (!reasons.has(path)) reasons.set(path, "patch.verify.reason.exists");
  }
  const conflicts = Array.from(reasons, ([path, reasonKey]) => ({ path, reasonKey }));
  const technical = [report.outcome, ...(report.messages ?? [])];
  const result = (
    severity: PatchVerificationSummary["severity"], summaryKey: TranslateKey, params: Vars = {},
  ): PatchVerificationSummary => ({ severity, summaryKey, params, conflicts, technical });

  // Interrupted writes and an unreliable recovery backup require intervention
  // (error). Read-only refusals, conflicts and uncertain results warrant warning.
  if (report.backup_compromised) return result("error", "patch.verify.backupCompromised");
  if (report.outcome === "Interrupted") return result("error", "patch.verify.interrupted");
  if (report.outcome.startsWith("DowngradeBlocked")) return result("warning", "patch.verify.downgrade");
  if (conflicts.length > 0) return result("warning", "patch.verify.conflicts", { count: conflicts.length });
  if (report.outcome.startsWith("Mismatch(")) return result("warning", "patch.verify.mismatch");

  // These are the only known clean outcomes. Never infer success from an
  // unfamiliar outcome or from the absence of parsable per-file diagnostics.
  switch (report.outcome) {
    case "Clean": return result("success", "patch.verify.clean");
    case "AlreadyApplied": return result("success", "patch.verify.alreadyApplied");
    case "UpgradeAvailable": return result("success", "patch.verify.upgradeAvailable");
    default: return result("warning", "patch.verify.unknown");
  }
}
