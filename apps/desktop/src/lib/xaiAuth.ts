/**
 * Pure xAI device-code poll decisions. Interval is the OAuth RFC 8628 default
 * (5s); the start payload does not include `interval`.
 */

export const GROK_SUB_PROVIDER_ID = "grok-sub";

/** OAuth device-code default when the start payload has no interval. */
export const XAI_POLL_INTERVAL_MS = 5_000;

export type XaiAuthPollStatus = "pending" | "complete" | "denied" | "expired";

export type XaiPollAction =
  | { action: "poll" }
  | { action: "stop"; outcome: Exclude<XaiAuthPollStatus, "pending"> };

export function nextXaiPollAction(
  status: XaiAuthPollStatus,
  opts?: { startedAtMs: number; expiresInSecs: number; nowMs: number },
): XaiPollAction {
  if (status === "complete" || status === "denied" || status === "expired") {
    return { action: "stop", outcome: status };
  }
  if (
    opts &&
    opts.nowMs >= opts.startedAtMs + opts.expiresInSecs * 1000
  ) {
    return { action: "stop", outcome: "expired" };
  }
  return { action: "poll" };
}

/** grok-sub is ready once it appears in the provider list as configured. */
export function grokSubIsReady(
  providers:
    | readonly { id: string; configured?: boolean }[]
    | null
    | undefined,
): boolean {
  const p = providers?.find((x) => x.id === GROK_SUB_PROVIDER_ID);
  if (!p) return false;
  return p.configured !== false;
}
