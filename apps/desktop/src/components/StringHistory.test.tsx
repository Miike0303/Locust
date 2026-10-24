import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { setLocale } from "../lib/i18n";
import { useProjectStore } from "../stores/projectStore";
import type { StringRevision } from "../lib/api";

const h = { open: undefined as boolean | undefined };
(globalThis as any).__stringHistory = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    let source: string | undefined;
    if (context.parentURL?.endsWith("/components/StringHistory.tsx") && specifier === "react") {
      source = "export const useState=initial=>[globalThis.__stringHistory.open ?? initial,()=>{}];";
    }
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      if (specifier === "./runtime") source = "export const IS_TAURI=true;";
      if (specifier === "@tauri-apps/api/core") source = "export const invoke=async()=>7842;";
    }
    if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    return nextResolve(specifier, context);
  },
});
const { default: StringHistory } = await import("./StringHistory");
const { getStringHistory } = await import("../lib/api");
const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, staleTime: Infinity, gcTime: Infinity } } });
const key = ["string-history", "project", "row"];
const revision: StringRevision = { id: 2, entry_id: "row", old_translation: "Old", new_translation: "New", old_status: "pending", new_status: "translated", provider: "fixture-provider", changed_at: "2026-10-19T12:00:00.123Z" };
beforeEach(() => { client.clear(); useProjectStore.setState({ confirmedOpenProjectKey: "project", projectTransitionInProgress: false }); h.open = undefined; setLocale("en"); });
after(() => { client.clear(); hooks.deregister(); Reflect.deleteProperty(globalThis, "__stringHistory"); setLocale("en"); });
function render(projectKey = "project", entryId = "row", translation: string | null = "Current") {
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(StringHistory, {
    projectKey, entryId, translation, busy: false, onRestore: async () => {},
  })));
}

for (const [locale, title, empty, restore, statuses] of [
  ["en", "History", "No changes recorded yet", "Restore this version", "Pending → Translated"],
  ["es", "Historial", "Aún no hay cambios registrados", "Restaurar esta versión", "Pendiente → Traducida"],
] as const) {
  test(`${locale}: closed by default, project/entry query stays disabled`, () => {
    setLocale(locale);
    const html = render();
    assert.ok(html.includes(title));
    assert.match(html, /aria-expanded="false"/);
    assert.ok(!html.includes(empty));
    assert.ok(client.getQueryCache().find({ queryKey: key, exact: true })?.isDisabled());
    assert.equal((client.getQueryCache().find({ queryKey: key, exact: true })?.options as { enabled: () => boolean }).enabled(), false);
  });
  test(`${locale}: open history renders localized empty state`, () => {
    setLocale(locale); h.open = true;
    client.setQueryData(key, []);
    assert.ok(render().includes(empty));
    assert.equal((client.getQueryCache().find({ queryKey: key, exact: true })?.options as { enabled: () => boolean }).enabled(), true);
  });
  test(`${locale}: revisions render time, status, provider, diff and eligible restore actions`, () => {
    setLocale(locale); h.open = true;
    client.setQueryData(key, [revision, { ...revision, id: 1, new_translation: null, provider: null }]);
    const html = render();
    assert.match(html, /aria-expanded="true"/);
    assert.ok(html.includes(statuses));
    assert.ok(html.includes("fixture-provider"));
    assert.match(html, /<time dateTime="2026-10-19T12:00:00.123Z"[^>]*>/);
    assert.ok(html.includes(new Date(revision.changed_at).toLocaleString(locale)));
    assert.ok(html.includes("bg-danger-muted text-danger"));
    assert.ok(html.includes("bg-success-muted text-success"));
    assert.equal(html.split(restore).length - 1, 1);
    assert.ok(!render("project", "row", "New").includes(restore));
  });
}

test("history caches are isolated by both project and entry", () => {
  h.open = true;
  client.setQueryData(key, [revision]);
  assert.ok(render().includes("fixture-provider"));
  assert.ok(!render("other-project").includes("fixture-provider"));
  assert.ok(!render("project", "other-entry").includes("fixture-provider"));
});

test("loading and failed requests show localized feedback and retry", () => {
  h.open = true;
  assert.match(render(), /role="status"/);
  const query = client.getQueryCache().find({ queryKey: key, exact: true })!;
  query.setState({ status: "error", error: new Error("offline") });
  const html = render();
  assert.match(html, /role="alert"/);
  assert.ok(html.includes("offline"));
  assert.ok(html.includes("Retry"));
});

test("desktop history uses embedded HTTP and escapes the entry id", async () => {
  const originalFetch = globalThis.fetch;
  const urls: string[] = [];
  globalThis.fetch = (async (url: string | URL | Request) => {
    urls.push(String(url));
    return new Response(JSON.stringify([revision]), { status: 200 });
  }) as typeof fetch;
  try {
    assert.deepEqual(await getStringHistory("file#row/1"), [revision]);
    assert.deepEqual(urls, ["http://localhost:7842/api/strings/file%23row%2F1/history"]);
  } finally { globalThis.fetch = originalFetch; }
});


test("open history's gate blocks transitions immediately and re-enables only the confirmed scope", () => {
  h.open = true;
  render();
  const captured = (client.getQueryCache().find({ queryKey: key, exact: true })!.options as { enabled: () => boolean }).enabled;
  assert.equal(captured(), true);
  useProjectStore.getState().beginProjectTransition();
  assert.equal(captured(), false, "the old render cannot fetch while an open is pending");
  useProjectStore.setState({ confirmedOpenProjectKey: "other-project", projectTransitionInProgress: false });
  render();
  assert.equal((client.getQueryCache().find({ queryKey: key, exact: true })!.options as { enabled: () => boolean }).enabled(), false);
  useProjectStore.setState({ confirmedOpenProjectKey: "project" });
  render();
  assert.equal((client.getQueryCache().find({ queryKey: key, exact: true })!.options as { enabled: () => boolean }).enabled(), true);
});
