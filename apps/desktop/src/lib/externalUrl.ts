import { IS_TAURI } from "./runtime";

/**
 * Open a web page in the user's browser. The desktop webview gives no guarantee
 * that window.open reaches the system browser, so Tauri goes through the shell plugin.
 */
export async function openExternalUrl(url: string): Promise<void> {
  if (IS_TAURI) {
    const { open } = await import("@tauri-apps/plugin-shell");
    await open(url);
    return;
  }
  window.open(url, "_blank", "noopener,noreferrer");
}
