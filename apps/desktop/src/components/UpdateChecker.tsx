import { useEffect, useState } from "react";
import { Download, CheckCircle, AlertCircle } from "lucide-react";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "upToDate" }
  | { kind: "available"; version: string; notes: string }
  | { kind: "downloading"; progress: number }
  | { kind: "ready" }
  | { kind: "error"; message: string };

export default function UpdateChecker() {
  const [state, setState] = useState<UpdateState>({ kind: "idle" });

  const checkForUpdate = async (silent = false) => {
    if (!IS_TAURI) {
      if (!silent) setState({ kind: "error", message: "Updates only available in desktop app" });
      return;
    }
    setState({ kind: "checking" });
    try {
      const { check } = await import("@tauri-apps/plugin-updater");
      const update = await check();
      if (update) {
        setState({
          kind: "available",
          version: update.version,
          notes: update.body ?? "",
        });
      } else {
        setState({ kind: "upToDate" });
        if (silent) setTimeout(() => setState({ kind: "idle" }), 3000);
      }
    } catch (err: any) {
      setState({ kind: "error", message: err.message ?? String(err) });
    }
  };

  const downloadAndInstall = async () => {
    if (!IS_TAURI) return;
    try {
      const { check } = await import("@tauri-apps/plugin-updater");
      const { relaunch } = await import("@tauri-apps/plugin-process");
      const update = await check();
      if (!update) return;

      let totalBytes = 0;
      let downloaded = 0;

      await update.downloadAndInstall((ev) => {
        if (ev.event === "Started") {
          totalBytes = ev.data.contentLength ?? 0;
        } else if (ev.event === "Progress") {
          downloaded += ev.data.chunkLength;
          const pct = totalBytes > 0 ? (downloaded / totalBytes) * 100 : 0;
          setState({ kind: "downloading", progress: pct });
        } else if (ev.event === "Finished") {
          setState({ kind: "ready" });
        }
      });

      await relaunch();
    } catch (err: any) {
      setState({ kind: "error", message: err.message ?? String(err) });
    }
  };

  // Check silently on mount
  useEffect(() => {
    if (IS_TAURI) {
      checkForUpdate(true).catch(() => {});
    }
  }, []);

  if (state.kind === "idle") return null;

  return (
    <div className="fixed bottom-4 right-4 z-50 max-w-md">
      {state.kind === "checking" && (
        <div className="bg-blue-50 dark:bg-blue-900/30 border border-blue-200 dark:border-blue-800 rounded-lg shadow-lg p-3 flex items-center gap-2 text-sm">
          <div className="animate-spin h-4 w-4 border-2 border-blue-500 border-t-transparent rounded-full" />
          <span>Checking for updates...</span>
        </div>
      )}

      {state.kind === "upToDate" && (
        <div className="bg-green-50 dark:bg-green-900/30 border border-green-200 dark:border-green-800 rounded-lg shadow-lg p-3 flex items-center gap-2 text-sm">
          <CheckCircle size={16} className="text-green-600" />
          <span>You're on the latest version</span>
        </div>
      )}

      {state.kind === "available" && (
        <div className="bg-white dark:bg-gray-800 border border-emerald-500 rounded-lg shadow-xl p-4">
          <div className="flex items-start gap-2">
            <Download className="text-emerald-500 flex-shrink-0 mt-0.5" size={20} />
            <div className="flex-1">
              <div className="font-semibold text-sm">
                Update available: v{state.version}
              </div>
              {state.notes && (
                <div className="text-xs text-gray-600 dark:text-gray-400 mt-1 max-h-32 overflow-y-auto whitespace-pre-wrap">
                  {state.notes}
                </div>
              )}
              <div className="flex gap-2 mt-3">
                <button
                  onClick={downloadAndInstall}
                  className="px-3 py-1.5 bg-emerald-600 hover:bg-emerald-700 text-white rounded text-sm font-medium"
                >
                  Download & install
                </button>
                <button
                  onClick={() => setState({ kind: "idle" })}
                  className="px-3 py-1.5 bg-gray-200 dark:bg-gray-700 rounded text-sm"
                >
                  Later
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {state.kind === "downloading" && (
        <div className="bg-white dark:bg-gray-800 border rounded-lg shadow-xl p-4">
          <div className="text-sm font-semibold mb-2">Downloading update...</div>
          <div className="w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2">
            <div
              className="bg-emerald-500 h-2 rounded-full transition-all"
              style={{ width: `${state.progress}%` }}
            />
          </div>
          <div className="text-xs text-gray-500 mt-1">
            {state.progress.toFixed(0)}%
          </div>
        </div>
      )}

      {state.kind === "ready" && (
        <div className="bg-emerald-50 dark:bg-emerald-900/30 border border-emerald-500 rounded-lg shadow-lg p-3 flex items-center gap-2 text-sm">
          <CheckCircle size={16} className="text-emerald-600" />
          <span>Update installed — restarting...</span>
        </div>
      )}

      {state.kind === "error" && (
        <div className="bg-red-50 dark:bg-red-900/30 border border-red-300 rounded-lg shadow-lg p-3 flex items-center gap-2 text-sm">
          <AlertCircle size={16} className="text-red-600 flex-shrink-0" />
          <div className="flex-1">
            <div className="font-semibold">Update check failed</div>
            <div className="text-xs text-gray-600 dark:text-gray-400">{state.message}</div>
          </div>
          <button
            onClick={() => setState({ kind: "idle" })}
            className="text-gray-500 hover:text-gray-700"
          >
            ✕
          </button>
        </div>
      )}
    </div>
  );
}

/** Manual "check for updates" trigger — can be used from a menu button */
export async function triggerUpdateCheck(): Promise<UpdateState> {
  if (!IS_TAURI) {
    return { kind: "error", message: "Updates only available in desktop app" };
  }
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    if (update) {
      return { kind: "available", version: update.version, notes: update.body ?? "" };
    }
    return { kind: "upToDate" };
  } catch (err: any) {
    return { kind: "error", message: err.message ?? String(err) };
  }
}
