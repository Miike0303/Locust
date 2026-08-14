/**
 * Resolve local zip vs URL for patch verify/apply (CLI/server parity).
 * Mutual exclusion + http(s) only for URLs.
 */

export type PatchSourceOk = { zip_path: string } | { zip_url: string };

/** Stable catalog keys — UI renders via `t()`, tests assert on the code. */
export const PATCH_SOURCE_ERROR = {
  both: "patch.source.both",
  badUrl: "patch.source.badUrl",
} as const;

export type PatchSourceErrorCode =
  (typeof PATCH_SOURCE_ERROR)[keyof typeof PATCH_SOURCE_ERROR];

export type PatchSourceResult =
  | PatchSourceOk
  | { error: PatchSourceErrorCode }
  | null;

/**
 * True when `url` is a non-empty absolute http(s) URL with a host (no fetch).
 * Rejects `https://` alone, `http:///path`, and non-http schemes.
 */
export function isHttpPatchUrl(url: string): boolean {
  const t = url.trim();
  if (!t) return false;
  try {
    const u = new URL(t);
    if (u.protocol !== "http:" && u.protocol !== "https:") return false;
    // hostname required (blocks "https://" and "http:///foo")
    return u.hostname.length > 0;
  } catch {
    return false;
  }
}

/**
 * Soft hint: path ends with `.zip` (query/hash ignored). Signed CDN links
 * without `.zip` in the path still download fine — UI may only warn.
 */
export function patchUrlLooksLikeZip(url: string): boolean {
  try {
    const u = new URL(url.trim());
    return u.pathname.toLowerCase().endsWith(".zip");
  } catch {
    return false;
  }
}

/**
 * @returns `null` if neither path nor URL provided;
 *          `{ error }` on mutual exclusion or bad scheme;
 *          `{ zip_path }` or `{ zip_url }` when valid (trimmed).
 */
export function resolvePatchSource(
  zipPath: string,
  zipUrl: string
): PatchSourceResult {
  const path = zipPath.trim();
  const url = zipUrl.trim();
  if (path && url) {
    return { error: PATCH_SOURCE_ERROR.both };
  }
  if (path) return { zip_path: path };
  if (url) {
    if (!isHttpPatchUrl(url)) {
      return { error: PATCH_SOURCE_ERROR.badUrl };
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
