export interface KeyboardTargetLike {
  tagName?: unknown;
  isContentEditable?: unknown;
}

export interface HotkeyPolicyContext {
  overlayOpen: boolean;
  target: KeyboardTargetLike | null;
}

export const HELP_ACTIONS: readonly string[] = [
  "openProject",
  "translate",
  "inject",
  "applyPatch",
  "exportFile",
  "validate",
  "search",
  "searchReplace",
  "reviewMode",
  "settings",
  "memory",
  "closePanel",
  "showHelp",
  // Sidebar order (components/Layout.tsx), so Alt+1..Alt+8 read top to bottom.
  "navHome",
  "navDashboard",
  "navEditor",
  "navReview",
  "navQa",
  "navSpeakers",
  "navMemory",
  "navSettings",
  // Handled by pages/Review.tsx through reviewKeyAction.
  "reviewApprove",
  "reviewApproveCtrl",
  "reviewSkip",
  "reviewPrevious",
  "reviewEdit",
  "reviewExit",
];

export type ReviewKeyAction = "approve" | "skip" | "previous" | "edit";

/** Single-key Review actions; the Shortcuts help and the Review buttons show these keys. */
export const REVIEW_KEYS = {
  approve: "a",
  skip: "s",
  previous: "p",
  edit: "e",
} as const satisfies Record<ReviewKeyAction, string>;

export interface ReviewKeyEventLike {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
}

/** Ctrl+Enter approves; single keys act only without Ctrl, Meta or Alt (so Ctrl+S or Alt+P do nothing). */
export function reviewKeyAction(e: ReviewKeyEventLike): ReviewKeyAction | null {
  const ctrl = e.ctrlKey === true;
  const otherModifier = e.metaKey === true || e.altKey === true;
  if (e.key === "Enter") return ctrl && !otherModifier ? "approve" : null;
  if (ctrl || otherModifier) return null;
  const match = (Object.keys(REVIEW_KEYS) as ReviewKeyAction[]).find(action => REVIEW_KEYS[action] === e.key);
  return match ?? null;
}

export function isEditableKeyboardTarget(target: KeyboardTargetLike | null): boolean {
  if (!target) return false;
  if (target.isContentEditable === true) return true;
  if (typeof target.tagName !== "string") return false;
  return ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName.toUpperCase());
}

export function shouldHandleEscape({ overlayOpen, target }: HotkeyPolicyContext): boolean {
  return overlayOpen || !isEditableKeyboardTarget(target);
}

export function shouldRunActionHotkey({ overlayOpen, target }: HotkeyPolicyContext): boolean {
  return !overlayOpen && !isEditableKeyboardTarget(target);
}

export interface HotkeyBinding {
  key: string;
  ctrl?: boolean;
  shift?: boolean;
  alt?: boolean;
  meta?: boolean;
  description: string;
  group: "Navigation" | "Editor" | "Review" | "General";
}

export const HOTKEY_MAP = {
  openProject:    { key: "o", ctrl: true, description: "Open project", group: "General" },
  translate:      { key: "t", ctrl: true, description: "Start translation", group: "Editor" },
  inject:         { key: "i", ctrl: true, description: "Inject translations", group: "Editor" },
  applyPatch:     { key: "p", ctrl: true, shift: true, description: "Apply patch zip", group: "Editor" },
  exportFile:     { key: "e", ctrl: true, description: "Export translations", group: "Editor" },
  validate:       { key: "v", ctrl: true, shift: true, description: "Validate translations", group: "Editor" },
  search:         { key: "f", ctrl: true, description: "Focus search / filter", group: "Editor" },
  searchReplace:  { key: "f", ctrl: true, shift: true, description: "Search & replace", group: "Editor" },
  reviewMode:     { key: "r", ctrl: true, shift: true, description: "Open review mode", group: "Navigation" },
  settings:       { key: "s", ctrl: true, shift: true, description: "Open settings", group: "Navigation" },
  memory:         { key: "m", ctrl: true, shift: true, description: "Translation memory", group: "Navigation" },
  closePanel:     { key: "Escape", description: "Close panel / modal", group: "General" },
  showHelp:       { key: "?", description: "Show keyboard shortcuts", group: "General" },
  showHelpF1:     { key: "F1", description: "Show keyboard shortcuts", group: "General" },
  // Alt+N opens the Nth sidebar item (components/Layout.tsx navItems).
  navHome:        { key: "1", alt: true, description: "Go to Home", group: "Navigation" },
  navDashboard:   { key: "2", alt: true, description: "Go to Dashboard", group: "Navigation" },
  navEditor:      { key: "3", alt: true, description: "Go to Editor", group: "Navigation" },
  navReview:      { key: "4", alt: true, description: "Go to Review", group: "Navigation" },
  navQa:          { key: "5", alt: true, description: "Go to QA", group: "Navigation" },
  navSpeakers:    { key: "6", alt: true, description: "Go to Speakers", group: "Navigation" },
  navMemory:      { key: "7", alt: true, description: "Go to Memory", group: "Navigation" },
  navSettings:    { key: "8", alt: true, description: "Go to Settings", group: "Navigation" },
  // Listed in the help; pages/Review.tsx handles them through reviewKeyAction.
  reviewApprove:     { key: REVIEW_KEYS.approve, description: "Approve", group: "Review" },
  reviewApproveCtrl: { key: "Enter", ctrl: true, description: "Approve", group: "Review" },
  reviewSkip:        { key: REVIEW_KEYS.skip, description: "Skip", group: "Review" },
  reviewPrevious:    { key: REVIEW_KEYS.previous, description: "Previous", group: "Review" },
  reviewEdit:        { key: REVIEW_KEYS.edit, description: "Edit translation", group: "Review" },
  reviewExit:        { key: "Escape", description: "Return to Editor", group: "Review" },
} as const satisfies Record<string, HotkeyBinding>;

export type HotkeyActionId = keyof typeof HOTKEY_MAP;

export function formatKey(binding: HotkeyBinding): string {
  const isMac = navigator.platform.includes("Mac");
  const parts: string[] = [];
  if (binding.ctrl) parts.push(isMac ? "\u2318" : "Ctrl");
  if (binding.shift) parts.push(isMac ? "\u21E7" : "Shift");
  if (binding.alt) parts.push(isMac ? "\u2325" : "Alt");

  let key = binding.key;
  if (key === "Escape") key = "Esc";
  else if (key.length === 1) key = key.toUpperCase();

  parts.push(key);
  return parts.join(isMac ? "" : "+");
}
