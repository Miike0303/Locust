import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ProjectInfo, ProjectStats, TranslationRun } from "../lib/api.ts";
import { ApiError } from "../lib/apiError.ts";
import { setLocale, t } from "../lib/i18n/index.ts";

const fixture: ProjectInfo = { path: "/games/Example", database_path: "/projects/example.db", format_id: "html", name: "Example" };
const stats: ProjectStats = { total: 10, pending: 3, translated: 2, reviewed: 1, approved: 3, error: 1, total_cost_usd: 0.0123 };
const h = { project: null as ProjectInfo | null };
(globalThis as any).__dashboardTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    let source: string | undefined;
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      if (specifier === "./runtime") source = "export const IS_TAURI=true;";
      if (specifier === "@tauri-apps/api/core") source = "export const invoke=async()=>7842;";
    }
    if (context.parentURL?.endsWith("/pages/Dashboard.tsx") && specifier === "../stores/projectStore") {
      source = "export const useProjectStore=select=>select(globalThis.__dashboardTest);";
    }
    if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    return nextResolve(specifier, context);
  },
});
const { default: Dashboard } = await import("./Dashboard.tsx");
const { getFileStats } = await import("../lib/api.ts");
const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, staleTime: Infinity, gcTime: Infinity } } });
beforeEach(() => { client.clear(); h.project = null; setLocale("en"); });
after(() => { client.clear(); hooks.deregister(); Reflect.deleteProperty(globalThis, "__dashboardTest"); setLocale("en"); });

function render() {
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(MemoryRouter, {}, createElement(Dashboard))));
}
function seed(overrides: Record<string, unknown> = {}) {
  h.project ??= fixture;
  render();
  const data: Record<string, unknown> = {
    "current-project": h.project, stats, "file-stats": [], "translation-runs": [],
    "injection-status": { game_root: h.project.path, pending: null }, ...overrides,
  };
  for (const query of client.getQueryCache().getAll()) client.setQueryData(query.queryKey, data[String(query.queryKey[0])]);
}

for (const [locale, heading, progress, aria] of [
  ["en", "Dashboard", "Progress", "Pending: 3; Translated: 2; Reviewed: 1; Approved: 3; Error: 1"],
  ["es", "Panel del proyecto", "Progreso", "Pendientes: 3; Traducidas: 2; Revisadas: 1; Aprobadas: 3; Errores: 1"],
] as const) {
  test(`${locale}: localized headings, progress image and project metadata`, () => {
    setLocale(locale);
    seed();
    const html = render();
    assert.match(html, new RegExp(`<h1[^>]*>${heading}</h1>`));
    assert.ok(html.includes(progress));
    assert.ok(html.includes(`role="img" aria-label="${aria}"`));
    assert.ok(html.includes("60%"));
    assert.match(html, /<p class="text-page font-bold tabular-nums">\$0\.01<\/p>/);
    for (const key of ["dashboard.total", "dashboard.status.pending", "dashboard.donePercent"] as const) {
      assert.match(html, new RegExp(`<th scope="col" class="[^"]*text-right[^"]*whitespace-nowrap[^"]*">${t(key)}</th>`));
    }
    assert.ok(html.includes(fixture.database_path!));
    assert.match(html, /href="\/editor"/);
    assert.ok(!html.includes(locale === "en" ? ">Attention<" : ">Atención<"));
  });

  test(`${locale}: no project uses EmptyState with Home link and no requests`, () => {
    setLocale(locale);
    const html = render();
    assert.ok(html.includes(locale === "en" ? "Open a project to see its dashboard." : "Abra un proyecto para ver su panel."));
    assert.match(html, /href="\/"/);
    assert.ok(client.getQueryCache().getAll().every(query => query.isDisabled()));
  });
}

test("loading and API failures provide status, shared localized error and retry", () => {
  h.project = fixture;
  assert.match(render(), /role="status"/);
  const query = client.getQueryCache().getAll().find(q => q.queryKey[0] === "stats")!;
  query.setState({ status: "error", error: new ApiError("404: entry not found") });
  const html = render();
  assert.match(html, /role="alert"/);
  assert.ok(html.includes("Retry"));
  assert.ok(html.includes("Entry not found"));
});

test("attention bounds warnings, lists file errors and offers the existing recovery modal", () => {
  h.project = { ...fixture, extraction_warnings: Array.from({ length: 12 }, (_, i) => `Warning-${i}`) };
  seed({
    "file-stats": [{ file_path: "/games/Example/data/a.json", total: 10, pending: 3, translated: 2, reviewed: 1, approved: 3, error: 1 }],
    "injection-status": { game_root: fixture.path, pending: { transaction_id: "tx" } },
  });
  const html = render();
  assert.ok(html.includes("Attention"));
  assert.ok(html.includes("Warning-9"));
  assert.ok(!html.includes("Warning-10"));
  assert.ok(html.includes("+2 more"));
  assert.ok(html.includes("data/a.json"));
  assert.ok(html.includes("Interrupted injection"));
  assert.ok(html.includes(t("recovery.action")));
  const source = readFileSync(new URL("./Dashboard.tsx", import.meta.url), "utf8");
  assert.match(source, /<InjectionRecoveryModal/);
});

test("run costs preserve unknown metadata, including a zero subtotal", () => {
  const run = { provider: "mock", started_at: "2026-10-19T12:00:00Z", cost_usd: 0 } as TranslationRun;
  seed({ "translation-runs": [run] });
  assert.ok(render().includes("unknown"));
  seed({ "translation-runs": [{ ...run, cost_usd: 0.0012, cost_is_complete: true }] });
  assert.ok(render().includes("$0.0012"));
});

test("switching databases cannot reuse another project's dashboard data", () => {
  seed();
  const firstKeys = client.getQueryCache().getAll().map(q => q.queryKey);
  h.project = { ...fixture, database_path: "/projects/other.db" };
  const html = render();
  assert.ok(!html.includes("60%"));
  const nextQueries = client.getQueryCache().getAll().filter(q => !firstKeys.some(key => JSON.stringify(key) === JSON.stringify(q.queryKey)));
  assert.equal(nextQueries.length, 5);
  assert.ok(nextQueries.every(q => q.state.data === undefined));
});

test("file stats use embedded HTTP and unwrap the server response even in Tauri", async () => {
  const originalFetch = globalThis.fetch;
  const files = [{ file_path: "a.json", total: 1, pending: 1, translated: 0, reviewed: 0, approved: 0, error: 0 }];
  globalThis.fetch = async input => {
    assert.equal(String(input), "http://localhost:7842/api/stats/files");
    return new Response(JSON.stringify({ files }));
  };
  try { assert.deepEqual(await getFileStats(), files); }
  finally { globalThis.fetch = originalFetch; }
});

for (const locale of ["en", "es"] as const) {
  test(`${locale}: Dashboard preserves segment widths and labels with the shared palette`, () => {
    setLocale(locale);
    seed();
    const html = render();
    const progress = html.match(/<div role="img"[^>]*>([\s\S]*?)<\/div>/)?.[1];
    assert.ok(progress);
    const segments = [...progress.matchAll(/class="([^"]+)" style="width:([^"]+)"/g)];
    assert.deepEqual(segments.map(([, classes, width]) => [classes, width]), [
      ["bg-border", "30%"], ["bg-accent/60", "20%"], ["bg-accent", "10%"],
      ["bg-success", "30%"], ["bg-danger", "10%"],
    ]);
    const legend = html.match(/<ul[^>]*>([\s\S]*?)<\/ul>/)?.[1];
    assert.ok(legend);
    assert.doesNotMatch(legend, /bg-warning|text-warning/);
    assert.equal((legend.match(/lucide-check/g) ?? []).length, 1);
    for (const status of ["pending", "translated", "reviewed", "approved", "error"] as const) {
      assert.ok(legend.includes(t(`dashboard.status.${status}`)));
    }
  });
}
