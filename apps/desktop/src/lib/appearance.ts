/**
 * Apply persisted UI appearance (theme + font size) to the document root.
 * Called from Settings on change and from App at boot so preferences
 * survive a restart. Missing/partial config keeps browser defaults.
 */

export interface AppearanceConfig {
  theme?: string;
  font_size?: number;
}

export const TABLE_ROW_HEIGHT_MIN = 24;
export const TABLE_ROW_HEIGHT_MAX = 56;
export const TABLE_ROW_HEIGHT_DEFAULT = 36;

export function clampTableRowHeight(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return TABLE_ROW_HEIGHT_DEFAULT;
  return Math.min(
    TABLE_ROW_HEIGHT_MAX,
    Math.max(TABLE_ROW_HEIGHT_MIN, Math.round(value)),
  );
}

export function showSourceColumnEnabled(value: unknown): boolean {
  return value !== false;
}

export function applyAppearance(ui: AppearanceConfig | null | undefined): void {
  const root = document.documentElement;
  const theme = ui?.theme ?? "system";
  root.classList.remove("dark", "light");
  if (
    theme === "dark" ||
    (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches)
  ) {
    root.classList.add("dark");
  }
  const size = ui?.font_size;
  if (typeof size === "number" && Number.isFinite(size) && size > 0) {
    root.style.fontSize = `${size}px`;
  }
}
