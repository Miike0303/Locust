/**
 * Apply persisted UI appearance (theme + font size) to the document root.
 * Called from Settings on change and from App at boot so preferences
 * survive a restart. Missing/partial config keeps browser defaults.
 */

export interface AppearanceConfig {
  theme?: string;
  font_size?: number;
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
