import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// Inspect every authored branch, including inline editing and conditional detail states.
const files = [
  "../pages/Editor.tsx",
  "../pages/Review.tsx",
  "../pages/TranslationMemory.tsx",
  "../components/StringTable.tsx",
  "../components/DetailPanel.tsx",
  "../components/FilterBar.tsx",
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

  test(`${file} has no emerald-500 CSS variable`, () => {
    assert.doesNotMatch(source, /var\(--color-emerald-500\)/);
  });
}

test("selected table row retains its inset indicator using the accent token", () => {
  const source = readFileSync(new URL("../components/StringTable.tsx", import.meta.url), "utf8");
  assert.match(source, /shadow-\[inset_2px_0_0_0_var\(--color-accent\)\]/);
});

test("visible workflow guide leaves exactly one primary-styled Translate control", () => {
  const editor = readFileSync(new URL("../pages/Editor.tsx", import.meta.url), "utf8");
  const banner = readFileSync(new URL("../components/WorkflowGuideBanner.tsx", import.meta.url), "utf8");
  assert.match(editor, /!guideDismissed && workflowStep && \([\s\S]*?<WorkflowGuideBanner/);
  assert.match(editor, /const BTN_PRIMARY = `[^`]* bg-accent text-white/);
  const toolbarButtons = editor.match(/<button\b[\s\S]*?<\/button>/g) ?? [];
  const translate = toolbarButtons.filter((button) => button.includes('t("editor.translate")'));
  assert.equal(translate.length, 1);
  assert.match(translate[0], /className=\{BTN_PRIMARY\}/);
  const bannerButtons = banner.match(/<button\b[\s\S]*?<\/button>/g) ?? [];
  const solid = /(?:^|\s)bg-accent(?:\s|$)/;
  const solidBannerButtons = bannerButtons.filter((button) => {
    const classes = button.match(/className="([^"]*)"/)?.[1];
    assert.ok(classes, "inspect every banner button's styling");
    return solid.test(classes);
  });
  assert.equal(1 + solidBannerButtons.length, 1, "toolbar Translate is the sole primary");
});

test("toolbar and guide translation actions retain the same modal path and dismissal", () => {
  const editor = readFileSync(new URL("../pages/Editor.tsx", import.meta.url), "utf8");
  const banner = readFileSync(new URL("../components/WorkflowGuideBanner.tsx", import.meta.url), "utf8");
  assert.match(editor, /onClick=\{\(\) => setShowTranslateModal\(true\)\}[\s\S]*?t\("editor.translate"\)/);
  assert.match(editor, /workflowStep === "translate"\) setShowTranslateModal\(true\)/);
  assert.match(editor, /onPrimaryAction=\{handleGuidePrimaryAction\}/);
  assert.match(editor, /<TranslationModal\s+open=\{showTranslateModal\}/);
  assert.match(banner, /onClick=\{onPrimaryAction\}/);
  assert.match(banner, /\{content.description\}/);
  assert.match(banner, /onClick=\{onDismiss\}/);
});

test("translated and approved status use accent tokens with hollow versus filled dots", () => {
  const source = readFileSync(new URL("../components/StringTable.tsx", import.meta.url), "utf8");
  const dots = source.match(/const statusDot[^=]*= \{([\s\S]*?)\};/)?.[1] ?? "";
  const text = source.match(/const statusText[^=]*= \{([\s\S]*?)\};/)?.[1] ?? "";
  assert.doesNotMatch(dots + text, /(?:bg|text)-blue-/);
  assert.match(dots, /translated: "bg-transparent ring-1 ring-inset ring-accent"/);
  assert.match(dots, /approved: "bg-accent"/);
  assert.match(text, /translated: "text-accent-fg"/);
  assert.match(text, /approved: "text-accent-fg"/);
  assert.match(source, /statusLabel\[status\] \? t\(statusLabel\[status\]\) : status/);
});
