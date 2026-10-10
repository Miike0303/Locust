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
