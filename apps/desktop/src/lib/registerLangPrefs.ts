/**
 * Persist Inject modal register-lang preferences (menu label override).
 * Mirrors CLI `--label` convenience across sessions.
 */

import { browserStorage, type StringStorage } from "./storage";
export type { StringStorage } from "./storage";

const LS_REG_LABEL = "locust.inject.regLabel";

/** Last non-empty menu label override, or `""`. */
export function loadRegLabelOverride(storage?: StringStorage | null): string {
  const s = storage === undefined ? browserStorage() : storage;
  if (!s) return "";
  try {
    return (s.getItem(LS_REG_LABEL) || "").trim();
  } catch {
    return "";
  }
}

/**
 * Remember optional register-lang `--label` text.
 * Empty / whitespace clears the stored value.
 */
export function rememberRegLabelOverride(
  label: string,
  storage?: StringStorage | null
): void {
  const s = storage === undefined ? browserStorage() : storage;
  if (!s) return;
  try {
    const t = label.trim();
    if (t) s.setItem(LS_REG_LABEL, t);
    else s.removeItem(LS_REG_LABEL);
  } catch {
    /* ignore quota / private mode */
  }
}
