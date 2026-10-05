/**
 * Lightweight asserts for projectOpenMerge
 * (run: npx --yes tsx src/lib/projectOpenMerge.test.ts).
 */
import assert from "node:assert/strict";
import { setLocale, t } from "./i18n/index.ts";
import {
  PENDING_AFTER_STALE_FILTER,
  projectOpenMergeNotice,
  shouldFocusPendingAfterOpen,
  shouldToastProjectOpenMerge,
  type ProjectOpenMergeInput,
} from "./projectOpenMerge.ts";

setLocale("en");

function open(partial: Partial<ProjectOpenMergeInput> = {}): ProjectOpenMergeInput {
  return {
    project_name: "Title",
    format_name: "Ren'Py",
    total_strings: 100,
    added: 0,
    updated: 0,
    stale_source_reset: 0,
    removed: 0,
    preserved_translations: 0,
    ...partial,
  };
}

const firstOpen = projectOpenMergeNotice(
  open({ added: 100, total_strings: 100 }),
  t,
);
assert.equal(shouldToastProjectOpenMerge({ stale_source_reset: 0, removed: 0 }), false);
assert.equal(firstOpen.toast, false);
assert.equal(firstOpen.toastMessage, null);
assert.match(firstOpen.logMessage, /100 strings added/);
assert.match(firstOpen.logMessage, /0 translations kept/);

const staleOpen = projectOpenMergeNotice(
  open({
    added: 2,
    updated: 1,
    stale_source_reset: 12,
    preserved_translations: 85,
  }),
  t,
);
assert.equal(shouldToastProjectOpenMerge({ stale_source_reset: 12, removed: 0 }), true);
assert.equal(shouldFocusPendingAfterOpen({ stale_source_reset: 12 }), true);
assert.equal(PENDING_AFTER_STALE_FILTER.status, "pending");
assert.equal(PENDING_AFTER_STALE_FILTER.offset, 0);
assert.equal(staleOpen.toast, true);
assert.ok(staleOpen.toastMessage);
assert.match(staleOpen.toastMessage, /12/);
assert.match(staleOpen.toastMessage, /pending/);
assert.doesNotMatch(staleOpen.toastMessage, /removed/);
assert.match(staleOpen.logMessage, /12 translations reset to pending/);
assert.match(staleOpen.logMessage, /85 translations kept/);

const removedOnly = projectOpenMergeNotice(
  open({
    removed: 4,
    preserved_translations: 96,
  }),
  t,
);
assert.equal(shouldToastProjectOpenMerge({ stale_source_reset: 0, removed: 4 }), true);
// Negative: removed-only toasts but must not jump to pending (those rows are gone).
assert.equal(shouldFocusPendingAfterOpen({ stale_source_reset: 0 }), false);
assert.equal(removedOnly.toast, true);
assert.ok(removedOnly.toastMessage);
assert.match(removedOnly.toastMessage, /4/);
assert.match(removedOnly.toastMessage, /no longer exist/);
assert.doesNotMatch(removedOnly.toastMessage, /pending/);

const unchangedReopen = projectOpenMergeNotice(
  open({ preserved_translations: 100 }),
  t,
);
assert.equal(unchangedReopen.toast, false);
assert.equal(shouldFocusPendingAfterOpen({ stale_source_reset: 0 }), false);
assert.equal(unchangedReopen.toastMessage, null);
assert.match(unchangedReopen.logMessage, /100 translations kept/);
assert.match(unchangedReopen.logMessage, /0 strings added/);

console.log("projectOpenMerge.test.ts: ok");
