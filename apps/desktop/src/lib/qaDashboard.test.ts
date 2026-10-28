import assert from "node:assert/strict";
import { test } from "node:test";
import { buildQaSummary, qaHasBeenChecked, qaIsAdvisoryKind, qaKindBadgeClass, qaLastRunKey, qaPagination, qaRelativePath } from "./qaDashboard.ts";

test("consistency warnings appear in summaries and filters alongside identical-source advisories", () => {
  const summary = { InconsistentTranslation: { open: 42, ignored: 2 }, IdenticalToSource: { open: 1, ignored: 0 } };
  assert.deepEqual(buildQaSummary(summary), {
    open: 43,
    kinds: [
      { kind: "IdenticalToSource", open: 1, ignored: 0 },
      { kind: "InconsistentTranslation", open: 42, ignored: 2 },
    ],
  });
  assert.deepEqual(buildQaSummary({}, "InconsistentTranslation"), {
    open: 0, kinds: [{ kind: "InconsistentTranslation", open: 0, ignored: 0 }],
  });
  for (const kind of ["InconsistentTranslation", "IdenticalToSource"]) {
    assert.equal(qaIsAdvisoryKind(kind), true);
    assert.equal(qaKindBadgeClass(kind), "bg-warning-muted text-warning");
  }
  for (const kind of ["MissingPlaceholder", "StaleTranslation", "FutureKind"]) {
    assert.equal(qaIsAdvisoryKind(kind), false);
    assert.equal(qaKindBadgeClass(kind), "bg-accent-muted text-accent-fg");
  }
});

test("QA chips count only open issues and preserve an empty selected kind", () => {
  const summary = { StaleTranslation: { open: 2, ignored: 3 }, EmptyTranslation: { open: 0, ignored: 4 } };
  const model = buildQaSummary(summary, "MissingPlaceholder");
  assert.equal(model.open, 2);
  assert.deepEqual(model.kinds, [
    { kind: "EmptyTranslation", open: 0, ignored: 4 },
    { kind: "MissingPlaceholder", open: 0, ignored: 0 },
    { kind: "StaleTranslation", open: 2, ignored: 3 },
  ]);
  assert.equal(Object.keys(summary).length, 2);
  assert.deepEqual(buildQaSummary({}), { open: 0, kinds: [] });
});

test("QA paths are relative across Windows separators and case without stripping neighbouring roots", () => {
  assert.equal(qaRelativePath("c:\\Games\\Demo\\data\\a.json", "C:\\games\\demo\\"), "data/a.json");
  assert.equal(qaRelativePath("/games/demo/data/a.json", "/games/demo/"), "data/a.json");
  assert.equal(qaRelativePath("/games/demo-other/a.json", "/games/demo"), "/games/demo-other/a.json");
  assert.equal(qaRelativePath("data/a.json", "/games/demo"), "data/a.json");
  assert.equal(qaRelativePath("C:\\Games\\Story.html", "c:\\games\\story.html"), "Story.html");
  assert.equal(qaRelativePath("/Games/demo/a.json", "/games/demo"), "/Games/demo/a.json");
  assert.equal(qaRelativePath("\\\\Server\\Games\\Demo\\a.json", "\\\\server\\games\\demo"), "a.json");
});

test("QA pagination recovers after ignore and validation shrink the result set", () => {
  assert.deepEqual(qaPagination(50, 51), { offset: 50, from: 51, to: 51, page: 2, pages: 2 });
  assert.deepEqual(qaPagination(50, 50), { offset: 0, from: 1, to: 50, page: 1, pages: 1 });
  assert.deepEqual(qaPagination(100, 0), { offset: 0, from: 0, to: 0, page: 1, pages: 1 });
  assert.equal(qaPagination(50, 151).offset, 50);
});


test("QA validation evidence requires a completed run or persisted open/ignored rows", () => {
  assert.equal(qaHasBeenChecked({}, null), false);
  assert.equal(qaHasBeenChecked({ EmptyTranslation: { open: 0, ignored: 0 } }, null), false);
  assert.equal(qaHasBeenChecked({}, 1792771200000), true);
  assert.equal(qaHasBeenChecked({ MissingPlaceholder: { open: 1, ignored: 0 } }, null), true);
  assert.equal(qaHasBeenChecked({ MissingPlaceholder: { open: 0, ignored: 1 } }, null), true);
});

test("QA completed-run keys isolate project databases", () => {
  assert.deepEqual(qaLastRunKey("project-a"), ["validation-last-run", "project-a"]);
  assert.notDeepEqual(qaLastRunKey("project-a"), qaLastRunKey("project-b"));
});
