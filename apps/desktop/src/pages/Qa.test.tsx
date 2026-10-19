import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ProjectInfo, StoredValidationIssue } from "../lib/api.ts";
import { ApiError } from "../lib/apiError.ts";
import { setLocale, t } from "../lib/i18n/index.ts";

const fixture: ProjectInfo = { path: "C:\\Games\\Demo", database_path: "C:\\Projects\\demo.db", format_id: "html", name: "Demo" };
const issue: StoredValidationIssue = { id: 1, entry_id: "entry-a", kind: "EmptyTranslation", message: "Translation is empty", ignored: false, source: "Hello <player>", translation: null, file_path: "C:\\Games\\Demo\\data\\a.json" };
const h = { project: null as ProjectInfo | null };
(globalThis as any).__qaTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    let source: string | undefined;
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      if (specifier === "./runtime") source = "export const IS_TAURI=true;";
      if (specifier === "@tauri-apps/api/core") source = "export const invoke=async cmd=>{if(cmd!=='get_server_port')throw Error('Unexpected IPC: '+cmd);return 7842;};";
    }
    if (context.parentURL?.endsWith("/pages/Qa.tsx") && specifier === "../stores/projectStore") source = "export const useProjectStore=select=>select(globalThis.__qaTest);";
    if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    return nextResolve(specifier, context);
  },
});
const { default: Qa } = await import("./Qa.tsx");
const { getValidationIssues, getValidationSummary, setValidationIssueIgnored } = await import("../lib/api.ts");
const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, staleTime: Infinity, gcTime: Infinity } } });
beforeEach(() => { client.clear(); h.project = null; setLocale("en"); });
after(() => { client.clear(); hooks.deregister(); Reflect.deleteProperty(globalThis, "__qaTest"); setLocale("en"); });

function render() {
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(MemoryRouter, {}, createElement(Qa))));
}
function seed(issues: StoredValidationIssue[] = [issue], total = issues.length) {
  h.project ??= fixture;
  render();
  for (const query of client.getQueryCache().getAll()) {
    client.setQueryData(query.queryKey, query.queryKey[0] === "validation-issues"
      ? { issues, total, offset: 0, limit: 50 }
      : { EmptyTranslation: { open: 2, ignored: 1 } });
  }
}

for (const [locale, heading] of [["en", "Quality assurance"], ["es", "Control de calidad"]] as const) {
  test(`${locale}: QA renders localized headings, chips, issue actions and relative paths`, () => {
    setLocale(locale);
    seed([issue, { ...issue, id: 2, ignored: true }], 51);
    const html = render();
    assert.match(html, new RegExp(`<h1[^>]*>${heading}</h1>`));
    for (const key of ["qa.run", "qa.all", "qa.showIgnored", "qa.openEditor", "qa.ignore", "qa.restore", "qa.col.file", "qa.col.source", "qa.col.message"] as const) assert.ok(html.includes(t(key)), key);
    assert.ok(html.includes(`${t("validate.kind.emptyLabel")} (2)`));
    assert.ok(html.includes("data/a.json"));
    assert.ok(html.includes('title="C:\\Games\\Demo\\data\\a.json"'));
    assert.ok(html.includes("Hello &lt;player&gt;"));
    assert.ok(html.includes(t("qa.showing", { from: 1, to: 50, total: 51 })));
    assert.ok(html.includes("1 / 2"));
    assert.match(html, /aria-pressed="true"/);
  });

  test(`${locale}: QA no-project state disables requests and links Home`, () => {
    setLocale(locale);
    const html = render();
    assert.ok(html.includes(t("qa.noProject.description")));
    assert.match(html, /href="\/"/);
    assert.ok(client.getQueryCache().getAll().every(query => query.isDisabled()));
  });

  test(`${locale}: QA empty state shows no open issues`, () => {
    setLocale(locale);
    seed([]);
    assert.ok(render().includes(t("qa.empty")));
  });
}

test("QA loading and failure states expose status, localized API error and retry", () => {
  h.project = fixture;
  assert.match(render(), /role="status"/);
  const query = client.getQueryCache().getAll().find(q => q.queryKey[0] === "validation-issues")!;
  query.setState({ status: "error", error: new ApiError("404: entry not found") });
  const html = render();
  assert.match(html, /role="alert"/);
  assert.ok(html.includes(t("common.retry")));
  assert.ok(html.includes("Entry not found"));
  query.setState({ status: "error", error: new ApiError("400: no project open") });
  assert.ok(render().includes(t("qa.noProject.description")));
});

test("QA query keys isolate databases even for the same game path", () => {
  seed();
  const firstKeys = client.getQueryCache().getAll().map(q => JSON.stringify(q.queryKey));
  h.project = { ...fixture, database_path: "C:\\Projects\\other.db" };
  const html = render();
  assert.ok(!html.includes(issue.message));
  const next = client.getQueryCache().getAll().filter(q => !firstKeys.includes(JSON.stringify(q.queryKey)));
  assert.equal(next.length, 2);
  assert.ok(next.every(q => q.state.data === undefined));
});

test("QA clients use embedded HTTP, serialize filters, toggle ignored and forward cancellation", async () => {
  const originalFetch = globalThis.fetch;
  const signal = new AbortController().signal;
  const calls: { url: string; init?: RequestInit }[] = [];
  globalThis.fetch = async (input, init) => {
    calls.push({ url: String(input), init });
    if (init?.method === "PATCH") return new Response(null, { status: 204 });
    return new Response(JSON.stringify(String(input).includes("summary") ? { EmptyTranslation: { open: 1, ignored: 0 } } : { issues: [issue], total: 1, offset: 50, limit: 50 }));
  };
  try {
    assert.equal((await getValidationIssues({ kind: "MissingPlaceholder", include_ignored: true, offset: 50, limit: 50 }, signal)).total, 1);
    assert.deepEqual(await getValidationSummary(signal), { EmptyTranslation: { open: 1, ignored: 0 } });
    await setValidationIssueIgnored(1, true);
    await setValidationIssueIgnored(1, false);
    assert.equal(calls[0].url, "http://localhost:7842/api/validation/issues?kind=MissingPlaceholder&include_ignored=true&offset=50&limit=50");
    assert.equal(calls[0].init?.signal, signal);
    assert.equal(calls[1].init?.signal, signal);
    assert.equal(calls[2].url, "http://localhost:7842/api/validation/issues/1");
    assert.equal(calls[2].init?.method, "PATCH");
    assert.deepEqual(JSON.parse(String(calls[2].init?.body)), { ignored: true });
    assert.deepEqual(JSON.parse(String(calls[3].init?.body)), { ignored: false });
  } finally { globalThis.fetch = originalFetch; }
});

test("QA is additive in the router, follows Review in navigation and starts the editor worklist", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const layout = readFileSync(new URL("../components/Layout.tsx", import.meta.url), "utf8");
  const page = readFileSync(new URL("./Qa.tsx", import.meta.url), "utf8");
  assert.ok(app.includes('<Route path="/qa" element={<Qa />} />'));
  assert.match(layout, /to: "\/review"[^\n]*\n[^\n]*to: "\/qa", icon: ShieldCheck, labelKey: "nav.qa"/);
  assert.ok(page.includes("startValidationWorklist([issue.entry_id]); navigate(\"/editor\")"));
  assert.match(page, /setKind\(counts.kind\); setOffset\(0\)/);
  assert.match(page, /setIncludeIgnored\(e.target.checked\); setOffset\(0\)/);
});
