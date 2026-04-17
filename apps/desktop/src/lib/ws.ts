import { getWsUrl } from "./api";
import type {
  ProgressEventStarted,
  ProgressEventBatchCompleted,
  ProgressEventStringTranslated,
  ProgressEventCompleted,
  ProgressEventFailed,
} from "./api";

interface JobHandlers {
  onStarted?: (e: ProgressEventStarted) => void;
  onBatchCompleted?: (e: ProgressEventBatchCompleted) => void;
  onStringTranslated?: (e: ProgressEventStringTranslated) => void;
  onCompleted?: (e: ProgressEventCompleted) => void;
  onFailed?: (e: ProgressEventFailed) => void;
  onPaused?: () => void;
}

interface WaitOptions {
  onProgress?: (completed: number, total: number, costSoFar: number) => void;
}

export function waitForJob(jobId: string, opts?: WaitOptions): Promise<void> {
  return new Promise((resolve, reject) => {
    const unsub = subscribeToJob(jobId, {
      onBatchCompleted: (e) => opts?.onProgress?.(e.completed, e.total, e.cost_so_far),
      onCompleted: () => { unsub(); resolve(); },
      onFailed: (e) => { unsub(); reject(new Error(e.error)); },
    });
  });
}

export function subscribeToJob(jobId: string, handlers: JobHandlers): () => void {
  let ws: WebSocket | null = null;

  getWsUrl(jobId).then((url) => {
    ws = new WebSocket(url);

    ws.onmessage = (event) => {
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
        }
      } catch (err) {
        console.error("Failed to parse WS message:", err);
      }
    };

    ws.onerror = (err) => {
      console.error("WebSocket error:", err);
    };
  });

  return () => {
    ws?.close();
  };
}
