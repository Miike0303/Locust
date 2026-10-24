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

test("editor table and progress consume the shared status palette", () => {
  const table = readFileSync(new URL("../components/StringTable.tsx", import.meta.url), "utf8");
  assert.match(table, /<StatusBadge status=\{status\}/);
  const editor = readFileSync(new URL("../pages/Editor.tsx", import.meta.url), "utf8");
  for (const status of ["approved", "reviewed", "translated"]) {
    assert.ok(editor.includes(`statusStyles.${status}.bar`));
    assert.ok(editor.includes(`width(stats.${status})`));
  }
});

test("removed welcome guide has no runtime exports or catalog keys", () => {
  const guide = readFileSync(new URL("./workflowGuide.ts", import.meta.url), "utf8");
  assert.doesNotMatch(guide, /WELCOME_WORKFLOW_STEPS|WelcomeGuideDismissed/);
  for (const locale of ["en", "es"]) {
    const catalog = readFileSync(new URL(`./i18n/${locale}.ts`, import.meta.url), "utf8");
    assert.doesNotMatch(catalog, /"welcome\.guide\./);
  }
});

test("editor returns detail focus to the roving table row on close and Escape", () => {
  const editor = readFileSync(new URL("../pages/Editor.tsx", import.meta.url), "utf8");
  assert.match(editor, /const tableRef = useRef<HTMLTableElement>\(null\)/);
  assert.match(editor, /const detailRef = useRef<HTMLElement>\(null\)/);
  assert.match(editor, /const closeDetail = useCallback\(\(\) => \{[\s\S]*?setSelected\(null\)[\s\S]*?tableRef\.current\?\.querySelector<HTMLElement>\('tbody tr\[tabindex="0"\]'\) \?\? tableRef\.current;[\s\S]*?target\?\.focus\(\)/);
  assert.match(editor, /else if \(selectedEntryId\) closeDetail\(\)/);
  assert.match(editor, /<StringTable[\s\S]*?tableRef=\{tableRef\}/);
  assert.match(editor, /<DetailPanel[\s\S]*?panelRef=\{detailRef\}[\s\S]*?onClose=\{closeDetail\}/);
  assert.match(editor, /detailRef\.current\?\.querySelector<HTMLTextAreaElement>\("textarea:not\(\[readonly\]\)"\)\?\.focus\(\)/);
});

test("compact details overlay the table with viewport-bounded width and a dismissible backdrop", () => {
  const source = readFileSync(new URL("../components/DetailPanel.tsx", import.meta.url), "utf8");
  assert.match(source, /fixed inset-0 z-40/);
  assert.match(source, /w-\[min\(420px,92vw\)\]/);
  assert.match(source, /shadow-xl/);
  assert.match(source, /bg-black\/20/);
  assert.match(source, /data-detail-backdrop[\s\S]*?onClick=\{onClose\}/);
});
