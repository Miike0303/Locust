/**
 * Lightweight asserts for hotkeyPolicy (run: npx --yes tsx src/lib/hotkeyPolicy.test.ts).
 */
import {
  HELP_ACTIONS,
  isEditableKeyboardTarget,
  shouldHandleEscape,
  shouldRunActionHotkey,
} from "./hotkeyPolicy.ts";

const assert = {
  equal(actual: unknown, expected: unknown, message?: string) {
    if (actual !== expected) throw new Error(message ?? `${actual} !== ${expected}`);
  },
  deepEqual(actual: unknown, expected: unknown, message?: string) {
    if (JSON.stringify(actual) !== JSON.stringify(expected)) {
      throw new Error(message ?? "values differ");
    }
  },
};

const input = { tagName: "INPUT" };
const textarea = { tagName: "textarea" };
const select = { tagName: "SELECT" };
const contentEditable = { tagName: "DIV", isContentEditable: true };
const button = { tagName: "BUTTON" };
const div = { tagName: "DIV" };

assert.equal(isEditableKeyboardTarget(input), true, "input is editable");
assert.equal(isEditableKeyboardTarget(textarea), true, "textarea is editable");
assert.equal(isEditableKeyboardTarget(select), true, "select is editable");
assert.equal(isEditableKeyboardTarget(contentEditable), true);
assert.equal(isEditableKeyboardTarget(button), false, "button is not editable");
assert.equal(isEditableKeyboardTarget(div), false, "div is not editable");
assert.equal(isEditableKeyboardTarget(null), false, "null is not editable");

assert.equal(shouldHandleEscape({ overlayOpen: true, target: input }), true);
assert.equal(shouldHandleEscape({ overlayOpen: false, target: input }), false);
assert.equal(shouldHandleEscape({ overlayOpen: false, target: button }), true);

assert.equal(shouldRunActionHotkey({ overlayOpen: true, target: button }), false);
assert.equal(shouldRunActionHotkey({ overlayOpen: false, target: textarea }), false);
assert.equal(shouldRunActionHotkey({ overlayOpen: false, target: div }), true);

const wiredHelpActions = [
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
  "navHome",
  "navEditor",
  "navReview",
  "navMemory",
  "navSettings",
].sort();

assert.deepEqual(
  [...HELP_ACTIONS].sort(),
  wiredHelpActions,
  "help registry contains exactly the user-facing wired actions"
);
assert.equal(HELP_ACTIONS.includes("openProject"), true, "Ctrl+O is advertised");
assert.equal(HELP_ACTIONS.includes("save"), false, "Ctrl+S is not advertised");

console.log("hotkeyPolicy.test.ts: ok");
