import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { setLocale, t } from "./i18n/index.ts";

const statuses = ["pending", "translated", "reviewed", "approved", "error"] as const;

test("status map covers every string status with semantic text, marker, chip and bar styles", async () => {
  const { statusStyles } = await import("./statusStyles.ts");
  assert.deepEqual(Object.keys(statusStyles), statuses);
  const expected = [
    ["text-text-muted", "bg-transparent ring-1 ring-inset ring-text-muted", "bg-surface-muted", "bg-border", false],
    ["text-accent-fg", "bg-transparent ring-1 ring-inset ring-accent", "bg-accent-muted", "bg-accent/60", false],
    ["text-accent-fg", "bg-accent", "bg-accent-muted", "bg-accent", false],
    ["text-success", "bg-success", "bg-success-muted", "bg-success", true],
    ["text-danger", "bg-danger", "bg-danger-muted", "bg-danger", false],
  ];
  for (const [index, status] of statuses.entries()) {
    const style = statusStyles[status];
    assert.deepEqual([style.text, style.dot, style.chip, style.bar, style.check], expected[index], status);
    assert.equal(style.labelKey, `detail.status.${status}`);
    assert.doesNotMatch(Object.values(style).join(" "), /warning|amber|orange|brown|blue/);
  }
});

for (const locale of ["en", "es"] as const) {
  test(`${locale}: every badge keeps a visible label and only approved has a 10px check`, async () => {
    const { StatusBadge } = await import("./statusStyles.ts");
    setLocale(locale);
    try {
      for (const status of statuses) {
        const html = renderToStaticMarkup(createElement(StatusBadge, { status }));
        assert.ok(html.includes(t(`detail.status.${status}`)), html);
        assert.match(html, /aria-hidden="true"/);
        assert.equal(html.includes("lucide-check"), status === "approved", html);
        if (status === "approved") assert.match(html, /width="10" height="10"/);
      }
    } finally { setLocale("en"); }
  });
}

test("unknown status retains its text with a neutral marker", async () => {
  const { StatusBadge } = await import("./statusStyles.ts");
  const html = renderToStaticMarkup(createElement(StatusBadge, { status: "future-status" }));
  assert.ok(html.includes("future-status"));
  assert.ok(html.includes("text-text-muted"));
  assert.ok(html.includes("ring-text-muted"));
});

for (const file of ["../components/StringTable.tsx", "../components/FilterBar.tsx", "../components/DetailPanel.tsx", "../pages/Dashboard.tsx", "../pages/Review.tsx", "../pages/Editor.tsx"]) {
  test(`${file} consumes the shared status palette`, () => {
    const source = readFileSync(new URL(file, import.meta.url), "utf8");
    assert.match(source, /from "\.\.\/lib\/statusStyles"/);
    if (!file.endsWith("Editor.tsx")) assert.match(source, /<StatusBadge/);
    if (file.endsWith("Dashboard.tsx")) assert.match(source, /statusStyles\[segment.status\]\.bar/);
    if (file.endsWith("Editor.tsx")) {
      for (const status of ["translated", "reviewed", "approved"]) assert.ok(source.includes(`statusStyles.${status}.bar`));
    }
    assert.doesNotMatch(source, /const (?:statusColors|statusDot|statusText|SEGMENT_CLASS)\b/);
  });
}

for (const file of ["../components/ConfirmDialog.tsx", "../components/ToastContainer.tsx"]) {
  test(`${file} uses semantic notification and destructive colors`, () => {
    const source = readFileSync(new URL(file, import.meta.url), "utf8");
    assert.doesNotMatch(source, /(?:bg|text|ring)-(?:red|blue|amber|orange|green|emerald)-\d+/);
    if (file.endsWith("ConfirmDialog.tsx")) assert.ok(source.includes("bg-danger text-danger-contrast"));
    else {
      for (const pair of ["bg-success-muted text-success", "bg-danger-muted text-danger", "bg-accent-muted text-accent-fg", "bg-warning-muted text-warning"]) assert.ok(source.includes(pair));
      assert.ok(source.includes("focus:ring-current"));
    }
  });
}
