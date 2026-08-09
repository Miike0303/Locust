/**
 * Inject UI mode selection from format `supported_modes`.
 * `direct` is always offered when Replace is supported (CLI `--direct` / in-place).
 */

export type OutputMode = "replace" | "add";
export type InjectUiMode = OutputMode | "direct";

/** Modes the user may pick for this format. Unknown modes → all three (legacy). */
export function availableInjectModes(
  supported: readonly OutputMode[] | null | undefined
): InjectUiMode[] {
  if (!supported || supported.length === 0) {
    return ["replace", "add", "direct"];
  }
  const hasReplace = supported.includes("replace");
  const hasAdd = supported.includes("add");
  const out: InjectUiMode[] = [];
  if (hasReplace) {
    out.push("replace", "direct");
  }
  if (hasAdd) {
    out.push("add");
  }
  return out.length > 0 ? out : ["replace", "add", "direct"];
}

/**
 * Default mode:
 * - Replace-only engines (Unity, KiriKiri, YU-RIS, …) → `direct` (in-place + patch recording)
 * - Formats with Add (Ren'Py, RPG Maker) → `add`
 * - Unknown supported_modes → `add` (historical desktop default)
 */
export function defaultInjectMode(
  supported: readonly OutputMode[] | null | undefined
): InjectUiMode {
  if (!supported || supported.length === 0) {
    return "add";
  }
  const avail = availableInjectModes(supported);
  if (!supported.includes("add") && avail.includes("direct")) {
    return "direct";
  }
  if (avail.includes("add")) {
    return "add";
  }
  return avail[0] ?? "replace";
}

/** Keep current mode if still valid; otherwise fall back to default. */
export function coerceInjectMode(
  current: InjectUiMode,
  supported: readonly OutputMode[] | null | undefined
): InjectUiMode {
  const avail = availableInjectModes(supported);
  if (avail.includes(current)) {
    return current;
  }
  return defaultInjectMode(supported);
}
