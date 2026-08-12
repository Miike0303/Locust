/**
 * Persist Inject modal register-lang preferences (menu label override).
 * Mirrors CLI `--label` convenience across sessions.
 */

const LS_REG_LABEL = "locust.inject.regLabel";

/** Minimal storage surface (localStorage or test double). */
export type StringStorage = {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
};

function browserStorage(): StringStorage | null {
  try {
    if (typeof localStorage === "undefined") return null;
    return localStorage;
  } catch {
    return null;
  }
}

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
