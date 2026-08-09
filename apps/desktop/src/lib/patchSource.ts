/**
 * Resolve local zip vs URL for patch verify/apply (CLI/server parity).
 * Mutual exclusion + http(s) only for URLs.
 */

export type PatchSourceOk = { zip_path: string } | { zip_url: string };

export type PatchSourceResult = PatchSourceOk | { error: string } | null;

/** True when `url` is a non-empty http(s) absolute URL (scheme only; no fetch). */
export function isHttpPatchUrl(url: string): boolean {
  const t = url.trim().toLowerCase();
  return t.startsWith("http://") || t.startsWith("https://");
}

/**
 * @returns `null` if neither path nor URL provided;
 *          `{ error }` on mutual exclusion or bad scheme;
 *          `{ zip_path }` or `{ zip_url }` when valid.
 */
export function resolvePatchSource(
  zipPath: string,
  zipUrl: string
): PatchSourceResult {
  const path = zipPath.trim();
  const url = zipUrl.trim();
  if (path && url) {
    return { error: "Use either a local zip path or a URL, not both" };
  }
  if (path) return { zip_path: path };
  if (url) {
    if (!isHttpPatchUrl(url)) {
      return { error: "Patch URL must start with http:// or https://" };
    }
    return { zip_url: url };
  }
  return null;
}

export function patchSourceReady(src: PatchSourceResult): src is PatchSourceOk {
  return src !== null && !("error" in src);
}

const LS_ZIP_URL = "locust.patch.zipUrl";
const LS_ZIP_PATH = "locust.patch.zipPath";

export function loadRememberedPatchSource(): { zipPath: string; zipUrl: string } {
  try {
    return {
      zipPath: localStorage.getItem(LS_ZIP_PATH) || "",
      zipUrl: localStorage.getItem(LS_ZIP_URL) || "",
    };
  } catch {
    return { zipPath: "", zipUrl: "" };
  }
}

/** Remember last successful source (path and URL are mutually exclusive). */
export function rememberPatchSource(src: PatchSourceOk): void {
  try {
    if ("zip_path" in src) {
      localStorage.setItem(LS_ZIP_PATH, src.zip_path);
      localStorage.removeItem(LS_ZIP_URL);
    } else {
      localStorage.setItem(LS_ZIP_URL, src.zip_url);
      localStorage.removeItem(LS_ZIP_PATH);
    }
  } catch {
    /* ignore quota / private mode */
  }
}
