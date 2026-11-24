import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

// Inspect every authored branch, including conditional status and update states.
const files = [
  "../components/Layout.tsx",
  "../components/BottomBar.tsx",
  "../components/ActivityLog.tsx",
  "../components/ToastContainer.tsx",
  "../components/UpdateChecker.tsx",
  "../components/WorkflowGuideBanner.tsx",
  "../components/RouteErrorBoundary.tsx",
  "../components/EmptyState.tsx",
  "../components/PatchStatusIndicator.tsx",
  "../components/DiffView.tsx",
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

test("sidebar is muted in light mode and a panel above the dark body base", () => {
  const source = readFileSync(new URL("../components/Layout.tsx", import.meta.url), "utf8");
  const sidebar = source.match(/<aside className="([^"]+)"/);
  assert.ok(sidebar, "sidebar retains its authored className");
  const classes = sidebar[1].split(/\s+/);
  assert.ok(classes.includes("bg-surface-muted"));
  assert.ok(classes.includes("dark:bg-surface"));
  assert.ok(classes.includes("border-border"));
});

test("body uses tokens without changing the white light and recessed dark bases", () => {
  const css = readFileSync(new URL("../index.css", import.meta.url), "utf8");
  assert.match(css, /body\s*\{\s*@apply bg-surface text-text dark:bg-surface-muted;\s*\}/);
});

test("sidebar shortcuts come from the hotkey map, one Alt action per item in sidebar order", () => {
  const source = readFileSync(new URL("../components/Layout.tsx", import.meta.url), "utf8");
  const navItems = source.match(/const navItems = \[([\s\S]*?)\] as const;/);
  assert.ok(navItems, "navItems retains its authored list");
  const items = [...navItems[1].matchAll(/\{ to: "([^"]+)"[^}]*hotkey: "(\w+)"[^}]*\}/g)].map(([, to, hotkey]) => [to, hotkey]);
  // hotkeys.test.ts proves navX opens its route with Alt+1..Alt+8 in this same order.
  assert.deepEqual(items, [
    ["/", "navHome"],
    ["/dashboard", "navDashboard"],
    ["/editor", "navEditor"],
    ["/review", "navReview"],
    ["/qa", "navQa"],
    ["/speakers", "navSpeakers"],
    ["/memory", "navMemory"],
    ["/settings", "navSettings"],
  ]);
  assert.doesNotMatch(source, /"Alt\+\d"/, "no hard-coded Alt labels");
  assert.match(source, /formatKey\(HOTKEY_MAP\[hotkey\]\)/);
});

test("collapsed sidebar is a 64px rail with hover and keyboard-focus tooltips", () => {
  const source = readFileSync(new URL("../components/Layout.tsx", import.meta.url), "utf8");
  assert.match(source, /data-\[collapsed=true\]:w-16/);
  assert.match(source, /group-hover:visible/);
  assert.match(source, /group-focus-visible:visible/);
  assert.match(source, /PanelLeftOpen/);
  assert.match(source, /PanelLeftClose/);
});
