/** Preserve caller options and normalize every supported HeadersInit format. */
export function mergeRequestInit(options?: RequestInit): RequestInit {
  const headers = new Headers(options?.headers);
  if (!headers.has("Content-Type")) headers.set("Content-Type", "application/json");
  return { ...options, headers };
}
