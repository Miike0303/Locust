import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import * as ts from "typescript";
import * as chrome from "./modalA11y.ts";
import { MODAL_BACKDROP_CLASS, MODAL_FOOTER_CLASS, modalPanelClass } from "./modalA11y.ts";

// Inspect every authored branch, including closed/conditional modal content.
// Guard each migrated modal's shared chrome and local styling.
const files = [
  "./modalA11y.ts",
  "../components/ConfirmDialog.tsx",
  "../components/ResumeProjectDialog.tsx",
  "../components/FontPatchDialog.tsx",
  "../components/InjectionRecoveryModal.tsx",
  "../components/ExportModal.tsx",
  "../components/PatchModal.tsx",
  "../components/InjectModal.tsx",
  "../components/PivotModal.tsx",
  "../components/SearchReplaceModal.tsx",
  "../components/HotkeyHelp.tsx",
  "../components/TranslationModal.tsx",
  "../components/ValidationResultsModal.tsx",
  "../components/QueuePanel.tsx",
] as const;

for (const file of files) {
  const source = readFileSync(new URL(file, import.meta.url), "utf8");

  test(`${file} has no loose gray palette utilities`, () => {
    const literals = source.match(/[\w:/-]*gray-[\w/.-]+/g) ?? [];
    assert.equal(literals.length, 0, `${file} still uses loose grays: ${literals.join(", ")}`);
  });

  test(`${file} has no arbitrary pixel text sizes`, () => {
    const literals = source.match(/text-\[\d+(?:\.\d+)?px\]/g) ?? [];
    assert.equal(literals.length, 0, `${file} still uses pixel text sizes: ${literals.join(", ")}`);
  });
}

test("shared panel bounds all dialogs to the dynamic viewport and preserves caller layout extras", () => {
  const base = modalPanelClass();
  assert.deepEqual(new Set(base.split(/\s+/)), new Set([
    "bg-surface", "text-text", "border", "border-border", "rounded-lg", "shadow-xl", "w-full",
    "max-h-[calc(100dvh-2rem)]", "overflow-y-auto", "overscroll-contain",
  ]));
  const extras = "max-w-2xl flex flex-col";
  assert.equal(modalPanelClass(extras), `${base} ${extras}`);
});

test("shared footer stays outside the shrinking scroll area and wraps actions", () => {
  assert.equal(MODAL_FOOTER_CLASS, "flex shrink-0 flex-wrap justify-end gap-2 border-t border-border px-5 py-3");
});

for (const file of files.slice(1)) test(`${file} cannot override the shared viewport height bound`, () => {
  const source = readFileSync(new URL(file, import.meta.url), "utf8");
  for (const match of source.matchAll(/modalPanelClass\("([^"]*)"\)/g)) {
    assert.ok(!match[1].includes("max-h-"), match[0]);
  }
});

test("Translation keeps its header and primary actions outside the shared internal scroll body", () => {
  assert.equal(chrome.MODAL_BODY_CLASS, "min-h-0 flex-1 overflow-y-auto overscroll-contain");
  const source = readFileSync(new URL("../components/TranslationModal.tsx", import.meta.url), "utf8");
  const ast = ts.createSourceFile("TranslationModal.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const sections = new Map<string, string[]>();
  const visit = (node: ts.Node) => {
    if (ts.isJsxElement(node)) {
      for (const attr of node.openingElement.attributes.properties) {
        if (ts.isJsxAttribute(attr) && ["data-modal-body", "data-modal-actions", "data-modal-header"].includes(attr.name.getText(ast))) {
          const name = attr.name.getText(ast);
          sections.set(name, [...(sections.get(name) ?? []), node.getText(ast)]);
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(ast);
  const bodies = sections.get("data-modal-body") ?? [];
  assert.equal(bodies.length, 2, "configuration and progress each have an internal scroll body");
  for (const body of bodies) {
    assert.match(body, /MODAL_BODY_CLASS/);
    assert.ok(!body.includes('t("translate.start")'));
    assert.ok(!body.includes('onClick={handleCancel}'));
  }
  assert.match(sections.get("data-modal-actions")?.[0] ?? "", /t\("translate.start"\)/);
  assert.match(sections.get("data-modal-actions")?.[0] ?? "", /shrink-0/);
  assert.match(sections.get("data-modal-header")?.[0] ?? "", /shrink-0/);
});

test("shared backdrop retains its translucent black overlay", () => {
  assert.equal(MODAL_BACKDROP_CLASS, "fixed inset-0 bg-black/50 flex items-center justify-center p-4 z-50");
});
