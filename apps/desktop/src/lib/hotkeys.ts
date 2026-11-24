import { useEffect, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import {
  HELP_ACTIONS,
  HOTKEY_MAP,
  shouldHandleEscape,
  shouldRunActionHotkey,
  type HotkeyActionId,
  type HotkeyBinding,
} from "./hotkeyPolicy";
import {
  completeOpenProject,
  isDetectionFailure,
  pickGameSource,
} from "./openProjectFlow";
import {
  PENDING_AFTER_STALE_FILTER,
  projectOpenMergeNotice,
  shouldFocusPendingAfterOpen,
} from "./projectOpenMerge";
import { useProjectStore } from "../stores/projectStore";
import { useEditorStore } from "../stores/editorStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import { t } from "./i18n";

// Pure bindings live in hotkeyPolicy.ts so views can show them without loading these hooks.
export { HOTKEY_MAP, formatKey } from "./hotkeyPolicy";
export type { HotkeyActionId, HotkeyBinding } from "./hotkeyPolicy";

export function matchesEvent(e: KeyboardEvent, binding: HotkeyBinding): boolean {
  const isMac = navigator.platform.includes("Mac");
  const ctrl = isMac ? (binding.meta ?? binding.ctrl ?? false) : (binding.ctrl ?? false);

  if (ctrl !== (isMac ? e.metaKey : e.ctrlKey)) return false;
  if ((binding.alt ?? false) !== e.altKey) return false;

  if (binding.key === "?") {
    // Most layouts need Shift to type "?", so Shift is not compared here.
    return e.key === "?" || (e.shiftKey && e.key === "/");
  }
  if ((binding.shift ?? false) !== e.shiftKey) return false;
  return e.key.toLowerCase() === binding.key.toLowerCase();
}

export function useHotkey(
  action: string,
  callback: () => void,
  enabled = true,
  allowInEditable = false,
  capture = false
) {
  const cbRef = useRef(callback);
  cbRef.current = callback;

  useEffect(() => {
    if (!enabled) return;
    const binding = HOTKEY_MAP[action as HotkeyActionId];
    if (!binding) return;

    const handler = (e: KeyboardEvent) => {
      const context = {
        overlayOpen: !!document.querySelector("[data-hotkey-overlay]"),
        target: e.target as HTMLElement | null,
      };
      const permitted = action === "closePanel"
        ? allowInEditable || shouldHandleEscape(context)
        : !context.overlayOpen && (allowInEditable || shouldRunActionHotkey(context));
      if (!permitted) return;
      if (matchesEvent(e, binding)) {
        e.preventDefault();
        e.stopImmediatePropagation();
        cbRef.current();
      }
    };

    window.addEventListener("keydown", handler, capture);
    return () => window.removeEventListener("keydown", handler, capture);
  }, [action, enabled, allowInEditable, capture]);
}

let openProjectInFlight = false;

export function useGlobalHotkeys(onShowHelp: () => void) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const setProject = useProjectStore((s) => s.setProject);
  const setFilter = useEditorStore((s) => s.setFilter);

  useHotkey("openProject", () => {
    if (openProjectInFlight) return;
    void (async () => {
      openProjectInFlight = true;
      try {
        const path = await pickGameSource(t);
        if (!path) return;
        try {
          const result = await completeOpenProject(path, undefined, {
            setProject,
            queryClient,
          });
          if (!result) return;
          const notice = projectOpenMergeNotice(result, t);
          addLog(
            notice.toast ? "warning" : "info",
            notice.logMessage,
            undefined,
            "project",
          );
          if (notice.toastMessage) {
            addToast("warning", notice.toastMessage);
          }
          if (shouldFocusPendingAfterOpen(result)) {
            setFilter(PENDING_AFTER_STALE_FILTER);
          }
          navigate("/editor");
        } catch (err: unknown) {
          const msg = err instanceof Error ? err.message : String(err);
          if (isDetectionFailure(msg)) {
            addLog("warning", t("activity.project.detectFailed"), path, "project");
            navigate("/", { state: { formatPickerPath: path } });
          } else {
            addLog("error", t("activity.project.openFailed"), msg, "project");
            addToast("error", t("welcome.toast.failedOpen", { error: msg }));
          }
        }
      } catch (err: unknown) {
        const msg = err instanceof Error ? err.message : String(err);
        addToast("error", t("welcome.toast.failedOpen", { error: msg }));
      } finally {
        openProjectInFlight = false;
      }
    })();
  });
  useHotkey("navHome", () => navigate("/"));
  useHotkey("navDashboard", () => navigate("/dashboard"));
  useHotkey("navEditor", () => navigate("/editor"));
  useHotkey("navReview", () => navigate("/review"));
  useHotkey("navQa", () => navigate("/qa"));
  useHotkey("navSpeakers", () => navigate("/speakers"));
  useHotkey("navMemory", () => navigate("/memory"));
  useHotkey("navSettings", () => navigate("/settings"));
  useHotkey("reviewMode", () => navigate("/review"));
  useHotkey("settings", () => navigate("/settings"));
  useHotkey("memory", () => navigate("/memory"));
  useHotkey("showHelp", onShowHelp);
  useHotkey("showHelpF1", onShowHelp);
}

export function getGroupedHotkeys(): Record<string, { action: string; binding: HotkeyBinding }[]> {
  const groups: Record<string, { action: string; binding: HotkeyBinding }[]> = {};
  for (const action of HELP_ACTIONS) {
    const binding = HOTKEY_MAP[action as HotkeyActionId];
    if (!binding) continue;
    if (!groups[binding.group]) groups[binding.group] = [];
    groups[binding.group].push({ action, binding });
  }
  return groups;
}
