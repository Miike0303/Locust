import { getWsUrl } from "./api";
import type {
  ProgressEventStarted,
  ProgressEventBatchCompleted,
  ProgressEventStringTranslated,
  ProgressEventCompleted,
  ProgressEventFailed,
  ProgressEventProviderSwitched,
} from "./api";

interface JobHandlers {
  onStarted?: (e: ProgressEventStarted) => void;
  onBatchCompleted?: (e: ProgressEventBatchCompleted) => void;
  onStringTranslated?: (e: ProgressEventStringTranslated) => void;
  onCompleted?: (e: ProgressEventCompleted) => void;
  onFailed?: (e: ProgressEventFailed) => void;
  onPaused?: () => void;
  onProviderSwitched?: (e: ProgressEventProviderSwitched) => void;
  /** Socket closed (server ended the stream or unsubscribe was called). */
  onClosed?: () => void;
}

interface WaitOptions {
  onProgress?: (completed: number, total: number, costSoFar: number) => void;
}

export const JOB_STREAM_LOST_MESSAGE =
  "connection to the translation job was lost";

export function waitForJob(jobId: string, opts?: WaitOptions): Promise<void> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const unsub = subscribeToJob(jobId, {
      onBatchCompleted: (e) => opts?.onProgress?.(e.completed, e.total, e.cost_so_far),
      onCompleted: () => settle(() => resolve()),
      onFailed: (e) => settle(() => reject(new Error(e.error))),
      onClosed: () =>
        settle(() => reject(new Error(JOB_STREAM_LOST_MESSAGE))),
    });
    function settle(done: () => void) {
      if (settled) return;
      settled = true;
      unsub();
      done();
    }
  });
}

export function subscribeToJob(jobId: string, handlers: JobHandlers): () => void {
  let ws: WebSocket | null = null;
  let cancelled = false;

  getWsUrl(jobId)
    .then((url) => {
      ws = new WebSocket(url);
      if (cancelled) {
        ws.close();
        return;
      }

      ws.onmessage = (event) => {
        if (cancelled) return;
        try {
          const data = JSON.parse(event.data);
          switch (data.type) {
            case "started":
              handlers.onStarted?.(data);
              break;
            case "batch_completed":
              handlers.onBatchCompleted?.(data);
              break;
            case "string_translated":
              handlers.onStringTranslated?.(data);
              break;
            case "completed":
              handlers.onCompleted?.(data);
              break;
            case "failed":
              handlers.onFailed?.(data);
              break;
            case "paused":
              handlers.onPaused?.();
              break;
            case "provider_switched":
              handlers.onProviderSwitched?.(data);
              break;
          }
        } catch (err) {
          console.error("Failed to parse WS message:", err);
        }
      };

      ws.onerror = (err) => {
        if (cancelled) return;
        console.error("WebSocket error:", err);
        handlers.onClosed?.();
      };

      ws.onclose = () => {
        if (cancelled) return;
        handlers.onClosed?.();
      };
    })
    .catch((err) => {
      if (cancelled) return;
      console.error("WebSocket error:", err);
      handlers.onClosed?.();
    });

  return () => {
    cancelled = true;
    ws?.close();
  };
}
