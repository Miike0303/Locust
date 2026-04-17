import { useEffect, useCallback, useRef } from "react";
import { useNavigate } from "react-router-dom";

export interface HotkeyBinding {
  key: string;
  ctrl?: boolean;
  shift?: boolean;
  alt?: boolean;
  meta?: boolean;
  description: string;
  group: "Navigation" | "Editor" | "Review" | "General";
}

export const HOTKEY_MAP: Record<string, HotkeyBinding> = {
  openProject:    { key: "o", ctrl: true, description: "Open project", group: "General" },
  translate:      { key: "t", ctrl: true, description: "Start translation", group: "Editor" },
  inject:         { key: "i", ctrl: true, description: "Inject translations", group: "Editor" },
  exportFile:     { key: "e", ctrl: true, description: "Export translations", group: "Editor" },
  validate:       { key: "v", ctrl: true, shift: true, description: "Validate translations", group: "Editor" },
  search:         { key: "f", ctrl: true, description: "Focus search / filter", group: "Editor" },
  searchReplace:  { key: "f", ctrl: true, shift: true, description: "Search & replace", group: "Editor" },
  reviewMode:     { key: "r", ctrl: true, shift: true, description: "Open review mode", group: "Navigation" },
  settings:       { key: "s", ctrl: true, shift: true, description: "Open settings", group: "Navigation" },
  memory:         { key: "m", ctrl: true, shift: true, description: "Translation memory", group: "Navigation" },
  save:           { key: "s", ctrl: true, description: "Save current edit", group: "Editor" },
  closePanel:     { key: "Escape", description: "Close panel / modal", group: "General" },
  showHelp:       { key: "?", description: "Show keyboard shortcuts", group: "General" },
  showHelpF1:     { key: "F1", description: "Show keyboard shortcuts", group: "General" },
  navHome:        { key: "1", alt: true, description: "Go to Home", group: "Navigation" },
  navEditor:      { key: "2", alt: true, description: "Go to Editor", group: "Navigation" },
  navReview:      { key: "3", alt: true, description: "Go to Review", group: "Navigation" },
  navMemory:      { key: "4", alt: true, description: "Go to Memory", group: "Navigation" },
  navSettings:    { key: "5", alt: true, description: "Go to Settings", group: "Navigation" },
};

function matchesEvent(e: KeyboardEvent, binding: HotkeyBinding): boolean {
  const isMac = navigator.platform.includes("Mac");
  const ctrl = isMac ? (binding.meta ?? binding.ctrl ?? false) : (binding.ctrl ?? false);

  if (ctrl !== (isMac ? e.metaKey : e.ctrlKey)) return false;
  if ((binding.shift ?? false) !== e.shiftKey) return false;
  if ((binding.alt ?? false) !== e.altKey) return false;

  if (binding.key === "?") {
    return e.key === "?" || (e.shiftKey && e.key === "/");
  }
  return e.key.toLowerCase() === binding.key.toLowerCase();
}

export function useHotkey(
  action: string,
  callback: () => void,
  enabled = true
) {
  const cbRef = useRef(callback);
  cbRef.current = callback;

  useEffect(() => {
    if (!enabled) return;
    const binding = HOTKEY_MAP[action];
    if (!binding) return;

    const handler = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement) return;
      if (matchesEvent(e, binding)) {
        e.preventDefault();
        cbRef.current();
      }
    };

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [action, enabled]);
}

export function useGlobalHotkeys(onShowHelp: () => void) {
  const navigate = useNavigate();

  useHotkey("navHome", () => navigate("/"));
  useHotkey("navEditor", () => navigate("/editor"));
  useHotkey("navReview", () => navigate("/review"));
  useHotkey("navMemory", () => navigate("/memory"));
  useHotkey("navSettings", () => navigate("/settings"));
  useHotkey("reviewMode", () => navigate("/review"));
  useHotkey("settings", () => navigate("/settings"));
  useHotkey("memory", () => navigate("/memory"));
  useHotkey("showHelp", onShowHelp);
  useHotkey("showHelpF1", onShowHelp);
}

export function formatKey(binding: HotkeyBinding): string {
  const isMac = navigator.platform.includes("Mac");
  const parts: string[] = [];
  if (binding.ctrl) parts.push(isMac ? "\u2318" : "Ctrl");
  if (binding.shift) parts.push(isMac ? "\u21E7" : "Shift");
  if (binding.alt) parts.push(isMac ? "\u2325" : "Alt");

  let key = binding.key;
  if (key === "Escape") key = "Esc";
  else if (key === "?") key = "?";
  else if (key === "F1") key = "F1";
  else key = key.toUpperCase();

  parts.push(key);
  return parts.join(isMac ? "" : "+");
}

export function getGroupedHotkeys(): Record<string, { action: string; binding: HotkeyBinding }[]> {
  const groups: Record<string, { action: string; binding: HotkeyBinding }[]> = {};
  for (const [action, binding] of Object.entries(HOTKEY_MAP)) {
    if (action === "showHelpF1") continue; // skip duplicate
    if (!groups[binding.group]) groups[binding.group] = [];
    groups[binding.group].push({ action, binding });
  }
  return groups;
}
