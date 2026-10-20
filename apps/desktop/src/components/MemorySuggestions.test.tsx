import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { setLocale } from "../lib/i18n";
import type { MemorySuggestion, StringEntry } from "../lib/api";
import { useProjectStore } from "../stores/projectStore";
import { draftProjectKey } from "../stores/draftStore";

(globalThis as any).__memorySuggestions = { useProjectStore };
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    let source: string | undefined;
    if (context.parentURL?.endsWith("/components/MemorySuggestions.tsx") && specifier === "../stores/projectStore") {
      source = "export const useProjectStore=selector=>selector(globalThis.__memorySuggestions.useProjectStore.getState());";
    }
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      if (specifier === "./runtime") source = "export const IS_TAURI=true;";
      if (specifier === "@tauri-apps/api/core") source = "export const invoke=async()=>7842;";
    }
    if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    return nextResolve(specifier, context);
  },
});
const { default: MemorySuggestions } = await import("./MemorySuggestions");
const { getMemorySuggestions } = await import("../lib/api");
const project = { path: "C:/fixture", name: "Fixture", format_id: "html-game" };
const projectKey = draftProjectKey(project);
const entry: StringEntry = { id: "row", source: "Hello", translation: "Current", status: "translated", tags: [], metadata: {}, created_at: "2026-01-01", file_path: "story.html", context: null, provider_used: null, char_limit: null, translated_at: null, reviewed_at: null };
const key = ["memory-suggestions", projectKey, entry.id, "en-es", entry.source, entry.translation];
const suggestion: MemorySuggestion = { source: "Hallo", translation: "Hola", score: 0.8, origin: "project", uses: 3 };
const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, staleTime: Infinity, gcTime: Infinity } } });
beforeEach(() => {
  client.clear();
  client.setQueryData(["config"], { default_source_lang: "en", default_target_lang: "es" });
  useProjectStore.setState({ project });
  setLocale("en");
});
after(() => { client.clear(); hooks.deregister(); Reflect.deleteProperty(globalThis, "__memorySuggestions"); useProjectStore.setState({ project: null }); setLocale("en"); });
function render(scope = projectKey, row = entry, busy = false) {
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(MemorySuggestions, {
    projectKey: scope, entry: row, busy, onUse() {},
  })));
}

for (const [locale, title, origin, use, current, matched, loading] of [
  ["en", "Memory suggestions", "Project", "Use", "Current source", "Memory source", "Loading memory suggestions…"],
  ["es", "Sugerencias de memoria", "Proyecto", "Usar", "Texto de origen actual", "Texto de origen en memoria", "Cargando sugerencias de memoria…"],
] as const) {
  test(`${locale}: suggestions show score, origins, compact source diff, translation and Use`, () => {
    setLocale(locale);
    client.setQueryData(key, { suggestions: [suggestion, { ...suggestion, translation: "Otra", origin: "global" }] });
    const html = render();
    for (const text of [title, origin, "Global", use, current, matched, "80%", "Hola", "Otra"]) assert.ok(html.includes(text), text);
    assert.match(html, /bg-danger-muted text-danger/);
    assert.match(html, /bg-success-muted text-success/);
    assert.match(html, /border border-border rounded p-2/);
    assert.match(render(projectKey, entry, true), /disabled=""/);
  });
  test(`${locale}: no suggestions hide the entire block; loading uses muted text`, () => {
    setLocale(locale);
    assert.ok(render().includes(loading));
    assert.match(render(), /role="status" class="text-caption text-text-muted"/);
    client.setQueryData(key, { suggestions: [] });
    assert.equal(render(), "");
  });
}

test("suggestion queries are scoped by project, entry and pair; absent project or pair disables fetching", () => {
  client.setQueryData(key, { suggestions: [suggestion] });
  assert.ok(render().includes("Hola"));
  assert.ok(!render(projectKey, { ...entry, id: "other" }).includes("Hola"));
  client.setQueryData(["config"], { default_source_lang: "en", default_target_lang: "fr" });
  assert.ok(!render().includes("Hola"));
  assert.equal(render("other-project"), "");
  assert.equal(render(projectKey, { ...entry, id: "" }), "");
  client.setQueryData(["config"], { default_source_lang: "en", default_target_lang: "auto" });
  assert.equal(render(), "");
  const noPair = client.getQueryCache().find({ queryKey: ["memory-suggestions", projectKey, entry.id, null, entry.source, entry.translation], exact: true });
  assert.equal((noPair?.options as { enabled?: boolean }).enabled, false);
  useProjectStore.setState({ project: null });
  assert.equal(render(), "");
});

test("desktop suggestions use embedded HTTP, encoded query parameters and cancellation", async () => {
  const originalFetch = globalThis.fetch;
  const calls: { url: string; signal: AbortSignal | null | undefined }[] = [];
  globalThis.fetch = (async (url: string | URL | Request, options?: RequestInit) => {
    calls.push({ url: String(url), signal: options?.signal });
    return new Response(JSON.stringify({ suggestions: [suggestion] }), { status: 200 });
  }) as typeof fetch;
  const controller = new AbortController();
  try {
    assert.deepEqual(await getMemorySuggestions("file#row/1", "en-pt-BR", controller.signal), { suggestions: [suggestion] });
    assert.deepEqual(calls, [{ url: "http://localhost:7842/api/memory/suggestions?entry_id=file%23row%2F1&lang_pair=en-pt-BR", signal: controller.signal }]);
  } finally { globalThis.fetch = originalFetch; }
});
