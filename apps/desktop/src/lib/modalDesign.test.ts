import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
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

test("shared panel adds only semantic chrome and preserves caller layout extras", () => {
  const base = modalPanelClass();
  assert.deepEqual(new Set(base.split(/\s+/)), new Set([
    "bg-surface", "text-text", "border", "border-border", "rounded-lg", "shadow-xl", "w-full",
  ]));
  const extras = "max-w-2xl max-h-[85vh] flex flex-col";
  assert.equal(modalPanelClass(extras), `${base} ${extras}`);
});

test("shared footer changes only its border color", () => {
  assert.equal(MODAL_FOOTER_CLASS, "flex justify-end gap-2 border-t border-border px-5 py-3");
});

test("shared backdrop retains its translucent black overlay", () => {
  assert.equal(MODAL_BACKDROP_CLASS, "fixed inset-0 bg-black/50 flex items-center justify-center z-50");
});
