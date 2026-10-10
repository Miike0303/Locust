import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ProjectInfo } from "../lib/api.ts";
import { setLocale, t } from "../lib/i18n/index.ts";
import { ApiError } from "../lib/apiError.ts";
import { useLogStore } from "../stores/logStore.ts";

// Keep React and React Query real; only select the active project for SSR.
const h = { project: null as ProjectInfo | null };
(globalThis as any).__memoryPageTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      const source = specifier === "./runtime"
        ? "export const IS_TAURI=true;"
        : specifier === "@tauri-apps/api/core" ? "export const invoke=async()=>7842;" : null;
      if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    }
    if (context.parentURL?.endsWith("/pages/TranslationMemory.tsx") && specifier === "../stores/projectStore") {
      return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(
        "export const useProjectStore=select=>select(globalThis.__memoryPageTest);",
      )}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: TranslationMemory } = await import("./TranslationMemory.tsx");
const api = await import("../lib/api.ts");
const client = new QueryClient({
  defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } },
});
beforeEach(() => { client.clear(); h.project = null; setLocale("en"); });
after(() => { client.clear(); hooks.deregister(); Reflect.deleteProperty(globalThis, "__memoryPageTest"); setLocale("en"); });

function render() {
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(TranslationMemory)));
}
function memoryQueries() {
  return client.getQueryCache().getAll().filter(query => String(query.queryKey[0]).startsWith("tm-"));
}
function project(database: string | undefined, path = "/games/shared"): ProjectInfo {
  return { path, database_path: database, format_id: "html", name: "Fixture" };
}
function seedEntries(source: string) {
  const query = memoryQueries().find(query => query.queryKey[0] === "tm-entries")!;
  assert.ok(query, "the page must register its entry query");
  client.setQueryData(query.queryKey, {
    total: 1, limit: 50, offset: 0,
    entries: [{ source_hash: source, source, translation: "Translation", lang_pair: "en-es", uses: 1, last_used: "2026-01-01" }],
  });
}

for (const locale of ["en", "es"] as const) {
  test(`${locale}: no project renders guidance and disables project queries`, () => {
    setLocale(locale);
    const html = render();
    const hint = locale === "en"
      ? "Open a project to see its translation memory."
      : "Abra un proyecto para ver su memoria de traducción.";
    assert.ok(html.includes(hint));
    assert.ok(!html.includes(t("memory.loadError")));
    assert.ok(!html.includes(t("memory.empty.description")));
    assert.ok(!html.includes(t("memory.clearAll")), "no destructive action without a project");
    assert.ok(memoryQueries().every(query => query.isDisabled()), "no background project requests");
  });
}

test("project-switch cancellation reaches every memory read without logging a connection failure", async () => {
  const originalFetch = globalThis.fetch;
  const controller = new AbortController();
  controller.abort();
  const cancellation = controller.signal.reason;
  const paths: string[] = [];
  useLogStore.getState().clear();
  globalThis.fetch = async (input, init) => {
    assert.equal(init?.signal, controller.signal);
    paths.push(new URL(String(input)).pathname);
    throw cancellation;
  };
  try {
    for (const read of [
      () => api.getTranslationMemory({}, controller.signal),
      () => api.getTranslationMemoryStats(controller.signal),
      () => api.getTranslationMemoryLangPairs(controller.signal),
    ]) {
      await assert.rejects(read, error => error === cancellation);
    }
    assert.deepEqual(paths, ["/api/memory", "/api/memory/stats", "/api/memory/lang-pairs"]);
    assert.equal(useLogStore.getState().entries.length, 0);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("entry, stats and language-pair caches include the database scope on project switches", () => {
  h.project = project("/projects/first.locust.db");
  render();
  const firstKeys = memoryQueries().map(query => query.queryKey);
  assert.equal(firstKeys.length, 3);
  for (const key of firstKeys) assert.ok(key.includes(h.project.database_path!), `${key[0]} must include the database`);
  seedEntries("Only in first project");
  assert.ok(render().includes("Only in first project"));
  // Pivot projects may share the same game path but have different databases.
  h.project = project("/projects/second.locust.db");
  const second = render();
  assert.ok(!second.includes("Only in first project"));
  const secondQueries = memoryQueries().filter(query => query.queryKey.includes(h.project!.database_path!));
  assert.equal(secondQueries.length, 3);
  assert.ok(secondQueries.every(query => query.state.data === undefined), "new scope must fetch its own data");
});

test("projects without a database path scope queries by their game path", () => {
  h.project = project(undefined, "/games/first");
  render();
  assert.equal(memoryQueries().length, 3);
  assert.ok(memoryQueries().every(query => query.queryKey.includes("/games/first")));
  h.project = project(undefined, "/games/second");
  render();
  assert.equal(memoryQueries().filter(query => query.queryKey.includes("/games/second")).length, 3);
});

test("closing a project hides cached entries and destructive actions", () => {
  h.project = project("/projects/first.locust.db");
  render();
  seedEntries("Previously open project");
  assert.ok(render().includes("Previously open project"));
  h.project = null;
  const html = render();
  assert.ok(html.includes("Open a project to see its translation memory."));
  assert.ok(!html.includes("Previously open project"));
  assert.ok(!html.includes(t("memory.clearAll")));
});

for (const locale of ["en", "es"] as const) {
  test(`${locale}: a server no-project response shows guidance when the local project is stale`, () => {
    setLocale(locale);
    h.project = project("/projects/stale.locust.db");
    render();
    const query = memoryQueries().find(query => query.queryKey[0] === "tm-entries")!;
    query.setState({ status: "error", error: new ApiError("400: no project open") });
    const html = render();
    assert.ok(html.includes(locale === "en"
      ? "Open a project to see its translation memory."
      : "Abra un proyecto para ver su memoria de traducción."));
    assert.ok(!html.includes(t("memory.loadError")));
    assert.ok(!html.includes(t("memory.clearAll")));
  });
}
