/**
 * Lightweight asserts for modalA11y (run: npx --yes tsx src/lib/modalA11y.test.ts).
 */
import {
  buildModalDialogProps,
  buildModalTitleProps,
  canRestoreFocus,
  chooseInitialFocus,
  isTabFocusTrapKey,
  resolveFocusTrapTarget,
  shouldOwnModalEscape,
} from "./modalA11y.ts";

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

const labelled = buildModalDialogProps({ titleId: "translation-title" });
assert.deepEqual(labelled, {
  role: "dialog",
  "aria-modal": true,
  "aria-labelledby": "translation-title",
  tabIndex: -1,
  "data-hotkey-overlay": "",
});
assert.equal("aria-label" in labelled, false);

const directlyLabelled = buildModalDialogProps({ ariaLabel: "Keyboard shortcuts" });
assert.equal(directlyLabelled["aria-label"], "Keyboard shortcuts");
assert.equal("aria-labelledby" in directlyLabelled, false);
assert.deepEqual(buildModalTitleProps("translation-title"), { id: "translation-title" });

const preferred = { name: "preferred" };
const firstFocusable = { name: "first" };
const root = { name: "root" };
assert.equal(
  chooseInitialFocus({ preferredInRoot: true, preferred, firstFocusable, root }),
  preferred
);
assert.equal(
  chooseInitialFocus({ preferredInRoot: false, preferred, firstFocusable, root }),
  firstFocusable
);
assert.equal(
  chooseInitialFocus({ preferredInRoot: false, preferred, firstFocusable: null, root }),
  root
);
assert.equal(
  chooseInitialFocus({ preferredInRoot: false, preferred: null, firstFocusable: null, root: null }),
  null
);

assert.equal(canRestoreFocus(null), false);
assert.equal(canRestoreFocus({ isConnected: false }), false);
assert.equal(canRestoreFocus({ isConnected: true }), true);

assert.equal(shouldOwnModalEscape({ open: true, ownEscape: true }), true);
assert.equal(shouldOwnModalEscape({ open: true, ownEscape: false }), false);
assert.equal(shouldOwnModalEscape({ open: false, ownEscape: true }), false);
assert.equal(shouldOwnModalEscape({ open: false, ownEscape: false }), false);

assert.equal(isTabFocusTrapKey("Tab"), true);
assert.equal(isTabFocusTrapKey("Escape"), false);

const first = { name: "first" } as unknown as HTMLElement;
const middle = { name: "middle" } as unknown as HTMLElement;
const last = { name: "last" } as unknown as HTMLElement;
const focusable = [first, middle, last];
assert.equal(
  resolveFocusTrapTarget({ focusable, active: last, shiftKey: false }),
  first
);
assert.equal(
  resolveFocusTrapTarget({ focusable, active: first, shiftKey: true }),
  last
);
assert.equal(
  resolveFocusTrapTarget({ focusable, active: middle, shiftKey: false }),
  null
);
assert.equal(
  resolveFocusTrapTarget({ focusable, active: null, shiftKey: false }),
  first
);

console.log("modalA11y.test.ts: ok");
