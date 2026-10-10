import { IS_TAURI } from "../lib/runtime";
import { useEffect, useState } from "react";
import { Download, CheckCircle, AlertCircle } from "lucide-react";
import { t as translateStandalone, useT } from "../lib/i18n";
import { addLog } from "../stores/logStore";

type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "upToDate" }
  | { kind: "available"; version: string; notes: string }
  | { kind: "downloading"; progress: number }
  | { kind: "ready" }
  | { kind: "error"; message: string };

export default function UpdateChecker() {
  const t = useT();
  const [state, setState] = useState<UpdateState>({ kind: "idle" });

  const checkForUpdate = async (silent = false) => {
    if (!IS_TAURI) {
      if (!silent) setState({ kind: "error", message: t("update.desktopOnly") });
      return;
    }
    if (!silent) setState({ kind: "checking" });
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
        setState({ kind: silent ? "idle" : "upToDate" });
      }
    } catch (err: any) {
      const message = err?.message ?? String(err);
      if (silent) {
        addLog("warning", t("update.checkFailed"), message, "updater");
        setState({ kind: "idle" });
      } else {
        setState({ kind: "error", message });
      }
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
        <div className="bg-blue-50 dark:bg-blue-900/30 border border-blue-200 dark:border-blue-800 rounded-lg shadow-lg p-3 flex items-center gap-2 text-body">
          <div className="animate-spin h-4 w-4 border-2 border-blue-500 dark:border-blue-400 border-t-transparent dark:border-t-transparent rounded-full" />
          <span>{t("update.checking")}</span>
        </div>
      )}

      {state.kind === "upToDate" && (
        <div className="bg-success-muted text-success border border-success rounded-lg shadow-lg p-3 flex items-center gap-2 text-body">
          <CheckCircle size={16} className="text-success" />
          <span>{t("update.upToDate")}</span>
        </div>
      )}

      {state.kind === "available" && (
        <div className="bg-surface text-text border border-accent-fg rounded-lg shadow-xl p-4">
          <div className="flex items-start gap-2">
            <Download className="text-accent-fg flex-shrink-0 mt-0.5" size={20} />
            <div className="flex-1">
              <div className="font-semibold text-body">
                {t("update.available", { version: state.version })}
              </div>
              {state.notes && (
                <div className="text-caption text-text-muted mt-1 max-h-32 overflow-y-auto whitespace-pre-wrap">
                  {state.notes}
                </div>
              )}
              <div className="flex gap-2 mt-3">
                <button
                  onClick={downloadAndInstall}
                  className="px-3 py-1.5 bg-accent hover:bg-accent-hover text-white rounded text-body font-medium"
                >
                  {t("update.download")}
                </button>
                <button
                  onClick={() => setState({ kind: "idle" })}
                  className="px-3 py-1.5 bg-surface-muted text-text rounded text-body"
                >
                  {t("common.later")}
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {state.kind === "downloading" && (
        <div className="bg-surface text-text border border-border rounded-lg shadow-xl p-4">
          <div className="text-body font-semibold mb-2">{t("update.downloading")}</div>
          <div className="w-full bg-border rounded-full h-2">
            <div
              className="bg-accent h-2 rounded-full transition-all"
              style={{ width: `${state.progress}%` }}
            />
          </div>
          <div className="text-caption text-text-muted mt-1">
            {state.progress.toFixed(0)}%
          </div>
        </div>
      )}

      {state.kind === "ready" && (
        <div className="bg-success-muted text-success border border-success rounded-lg shadow-lg p-3 flex items-center gap-2 text-body">
          <CheckCircle size={16} className="text-success" />
          <span>{t("update.installed")}</span>
        </div>
      )}

      {state.kind === "error" && (
        <div className="bg-danger-muted text-danger border border-danger rounded-lg shadow-lg p-3 flex items-center gap-2 text-body">
          <AlertCircle size={16} className="text-danger flex-shrink-0" />
          <div className="flex-1">
            <div className="font-semibold">{t("update.checkFailed")}</div>
            <div className="text-caption text-text-muted">{state.message}</div>
          </div>
          <button
            onClick={() => setState({ kind: "idle" })}
            className="text-text-muted hover:text-text"
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
    return { kind: "error", message: translateStandalone("update.desktopOnly") };
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
