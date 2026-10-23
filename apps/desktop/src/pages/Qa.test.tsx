import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { MutationObserver, QueryClient, QueryClientProvider, type MutationObserverOptions } from "@tanstack/react-query";
import type { ProjectInfo, StoredValidationIssue, ValidationResponse, ValidationSummary } from "../lib/api.ts";
import { ApiError } from "../lib/apiError.ts";
import { setLocale, t } from "../lib/i18n/index.ts";

const fixture: ProjectInfo = { path: "C:\\Games\\Demo", database_path: "C:\\Projects\\demo.db", format_id: "html", name: "Demo" };
const issue: StoredValidationIssue = { id: 1, entry_id: "entry-a", kind: "EmptyTranslation", message: "Translation is empty", ignored: false, source: "Hello <player>", translation: null, file_path: "C:\\Games\\Demo\\data\\a.json" };
const clean: ValidationResponse = { validation: { total_checked: 1, issues_found: 0, entries_with_issues: 0, by_kind: {}, issues: [] }, fonts: [] };
const h = {
  project: null as ProjectInfo | null,
  runOptions: undefined as MutationObserverOptions<ValidationResponse, Error, void> | undefined,
  runObserver: undefined as MutationObserver<ValidationResponse, Error, void> | undefined,
  validate: async (): Promise<ValidationResponse> => clean,
};
(globalThis as any).__qaTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    let source: string | undefined;
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      if (specifier === "./runtime") source = "export const IS_TAURI=true;";
      if (specifier === "@tauri-apps/api/core") source = "export const invoke=async cmd=>{if(cmd==='run_validation')return globalThis.__qaTest.validate();if(cmd!=='get_server_port')throw Error('Unexpected IPC: '+cmd);return 7842;};";
    }
    if (context.parentURL?.endsWith("/pages/Qa.tsx") && specifier === "@tanstack/react-query") {
      const actual = nextResolve(specifier, context).url;
      source = `export * from ${JSON.stringify(actual)};import {useMutation as actualUseMutation} from ${JSON.stringify(actual)};export const useMutation=options=>{const h=globalThis.__qaTest;if(options.mutationKey?.[0]==='validation-run'){h.runOptions=options;if(h.runObserver)return h.runObserver.getCurrentResult();}return actualUseMutation(options);};`;
    }
    if (context.parentURL?.endsWith("/pages/Qa.tsx") && specifier === "../stores/projectStore") source = "export const useProjectStore=select=>select(globalThis.__qaTest);";
    if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    return nextResolve(specifier, context);
  },
});
const { default: Qa } = await import("./Qa.tsx");
const { getValidationIssues, getValidationSummary, setValidationIssueIgnored } = await import("../lib/api.ts");
const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, staleTime: Infinity, gcTime: Infinity } } });
beforeEach(() => { client.clear(); h.project = null; setLocale("en"); h.runOptions = undefined; h.runObserver = undefined; h.validate = async () => clean; });
after(() => { client.clear(); hooks.deregister(); Reflect.deleteProperty(globalThis, "__qaTest"); setLocale("en"); });

function render() {
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(MemoryRouter, {}, createElement(Qa))));
}
function seed(issues: StoredValidationIssue[] = [issue], total = issues.length, summary: ValidationSummary = issues.length ? { EmptyTranslation: { open: 2, ignored: 1 } } : {}) {
  h.project ??= fixture;
  render();
  for (const query of client.getQueryCache().getAll().filter(q => q.queryKey[0] !== "validation-last-run")) {
    client.setQueryData(query.queryKey, query.queryKey[0] === "validation-issues"
      ? { issues, total, offset: 0, limit: 50 }
      : summary);
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

  test(`${locale}: never-run QA shows a neutral explanation and two primary validation actions`, () => {
    setLocale(locale);
    seed([]);
    const html = render();
    assert.ok(html.includes(t("qa.notChecked")));
    assert.ok(html.includes(t("qa.notCheckedDescription")));
    assert.ok(!html.includes(t("qa.empty")));
    assert.match(html, /lucide-shield-question/);
    const actions = [...html.matchAll(/<button[^>]*class="([^"]*)"[^>]*>[^<]*<\/button>/g)].filter(match => match[0].includes(t("qa.run")));
    assert.equal(actions.length, 2);
    for (const action of actions) {
      assert.ok(action[1].split(" ").includes("bg-accent"));
      assert.ok(!action[1].split(" ").includes("text-accent-fg"));
      assert.ok(action[1].split(" ").includes("text-white"));
    }
  });

  test(`${locale}: completed clean run shows success with local completion time and survives remount`, async () => {
    setLocale(locale);
    seed([]);
    const completedAt = await completeRun();
    const html = render();
    assert.ok(html.includes(t("qa.empty")));
    assert.ok(html.includes(t("qa.checkedAt", { time: new Date(completedAt).toLocaleString(locale) })));
    assert.match(html, /lucide-shield-check[^>]*text-success/);
    assert.ok(!html.includes(t("qa.notChecked")));
    assert.match(html, /<button[^>]*bg-accent-muted[^>]*>/);
    assert.equal([...html.matchAll(/<button[^>]*>Run validation<\/button>/g)].length, locale === "en" ? 1 : 0);
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
  assert.equal(next.length, 3);
  assert.ok(next.every(q => q.state.data == null));
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


async function completeRun() {
  render();
  assert.ok(h.runOptions, "QA must expose its validation mutation");
  h.runObserver = new MutationObserver(client, h.runOptions);
  await h.runObserver.mutate();
  const query = client.getQueryCache().getAll().find(q => q.queryKey[0] === "validation-last-run")!;
  assert.equal(typeof query.state.data, "number");
  return query.state.data as number;
}

test("QA checking and failed runs never show a clean result or record a completion", async () => {
  seed([]);
  let reject!: (error: Error) => void;
  h.validate = () => new Promise((_resolve, fail) => { reject = fail; });
  render();
  assert.ok(h.runOptions);
  h.runObserver = new MutationObserver(client, h.runOptions);
  const run = h.runObserver.mutate();
  await new Promise(resolve => setImmediate(resolve));
  assert.ok(reject, "Validation must have started");
  const html = render();
  assert.match(html, /role="status"/);
  assert.ok(html.includes(t("qa.running")));
  assert.ok(!html.includes(t("qa.empty")));
  reject(new Error("Validation failed"));
  await assert.rejects(run);
  assert.ok(render().includes(t("qa.notChecked")));
  assert.ok(render().includes(t("qa.actionError")));
  assert.ok(client.getQueryCache().getAll().filter(q => q.queryKey[0] === "validation-last-run").every(q => q.state.data === null));
});

test("QA persisted ignored rows use a neutral filtered state without inventing a check time", () => {
  seed([], 0, { EmptyTranslation: { open: 0, ignored: 1 } });
  const html = render();
  assert.ok(html.includes(t("qa.noMatchingIssues")));
  assert.ok(!html.includes(t("qa.notChecked")));
  assert.doesNotMatch(html, /text-success/);
  assert.match(html, /<button[^>]*bg-accent-muted[^>]*>/);
});

test("QA clean completion does not leak to another database and is retained when returning", async () => {
  seed([]);
  await completeRun();
  h.runObserver = undefined;
  h.project = { ...fixture, database_path: "C:\\Projects\\other.db" };
  seed([]);
  assert.ok(render().includes(t("qa.notChecked")));
  h.project = fixture;
  assert.ok(render().includes(t("qa.empty")));
});

test("QA nonempty summary never gives a clean all-clear for an empty filtered page", async () => {
  seed([]);
  await completeRun();
  h.runObserver = undefined;
  seed([], 0, { MissingPlaceholder: { open: 1, ignored: 0 } });
  const html = render();
  assert.ok(html.includes(t("qa.noMatchingIssues")));
  assert.doesNotMatch(html, /text-success/);
});
