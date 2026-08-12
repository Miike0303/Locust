export interface KeyboardTargetLike {
  tagName?: unknown;
  isContentEditable?: unknown;
}

export interface HotkeyPolicyContext {
  overlayOpen: boolean;
  target: KeyboardTargetLike | null;
}

export const HELP_ACTIONS: readonly string[] = [
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
  "navHome",
  "navEditor",
  "navReview",
  "navMemory",
  "navSettings",
];

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
