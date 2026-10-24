import { browserStorage } from "./storage";

const SIDEBAR_COLLAPSED_KEY = "locust.ui.sidebarCollapsed";
const COMPACT_LAYOUT_QUERY = "(width < 1200px)";

/** Null follows the viewport default; false is an explicit expanded preference. */
export function readSidebarCollapsed(): boolean | null {
  try {
    const value = browserStorage()?.getItem(SIDEBAR_COLLAPSED_KEY);
    return value === "true" ? true : value === "false" ? false : null;
  } catch {
    return null;
  }
}

export function saveSidebarCollapsed(collapsed: boolean): void {
  try {
    browserStorage()?.setItem(SIDEBAR_COLLAPSED_KEY, String(collapsed));
  } catch {
    // Layout controls remain usable when browser storage is unavailable.
  }
}

export function isCompactLayout(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia(COMPACT_LAYOUT_QUERY).matches
    : false;
}

export function subscribeCompactLayout(onChange: (compact: boolean) => void): () => void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return () => {};
  const media = window.matchMedia(COMPACT_LAYOUT_QUERY);
  const handleChange = (event: MediaQueryListEvent) => onChange(event.matches);
  media.addEventListener("change", handleChange);
  return () => media.removeEventListener("change", handleChange);
}
