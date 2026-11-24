import { browserStorage } from "./storage";

const SIDEBAR_COLLAPSED_KEY = "locust.ui.sidebarCollapsed";
const COMPACT_LAYOUT_QUERY = "(width < 1200px)";
/** Below this width a saved expanded sidebar squeezes the editor table. */
const NARROW_LAYOUT_QUERY = "(width < 1000px)";

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

function matchesQuery(query: string): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia(query).matches
    : false;
}

function subscribeQuery(query: string, onChange: (matches: boolean) => void): () => void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return () => {};
  const media = window.matchMedia(query);
  const handleChange = (event: MediaQueryListEvent) => onChange(event.matches);
  media.addEventListener("change", handleChange);
  return () => media.removeEventListener("change", handleChange);
}

export function isCompactLayout(): boolean {
  return matchesQuery(COMPACT_LAYOUT_QUERY);
}

export function subscribeCompactLayout(onChange: (compact: boolean) => void): () => void {
  return subscribeQuery(COMPACT_LAYOUT_QUERY, onChange);
}

export function isNarrowLayout(): boolean {
  return matchesQuery(NARROW_LAYOUT_QUERY);
}

export function subscribeNarrowLayout(onChange: (narrow: boolean) => void): () => void {
  return subscribeQuery(NARROW_LAYOUT_QUERY, onChange);
}
