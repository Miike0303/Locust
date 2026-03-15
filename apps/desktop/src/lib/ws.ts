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

export function subscribeToJob(jobId: string, handlers: JobHandlers): () => void {
  const ws = new WebSocket(`ws://localhost:7842/api/translate/ws/${jobId}`);

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

  return () => {
    ws.close();
  };
}
