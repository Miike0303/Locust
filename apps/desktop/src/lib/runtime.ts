/** Detect on demand for callers that need the current browser environment. */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Import-time snapshot for API routing and desktop-only UI controls. */
export const IS_TAURI = isTauri();
