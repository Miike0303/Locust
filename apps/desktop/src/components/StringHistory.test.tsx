import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { createElement, isValidElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { setLocale } from "../lib/i18n";
import { en } from "../lib/i18n/en";
import { es } from "../lib/i18n/es";
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
// The parent owns catalog additions; supply the pending labels only in this fixture.
const pendingLabels = [[en, "Restore before"], [es, "Restaurar anterior"]] as const;
const addedLabels = pendingLabels.filter(([catalog]) => !Object.prototype.hasOwnProperty.call(catalog, "detail.history.restoreBefore"));
for (const [catalog, label] of addedLabels) Object.assign(catalog, { "detail.history.restoreBefore": label });
beforeEach(() => { client.clear(); useProjectStore.setState({ confirmedOpenProjectKey: "project", projectTransitionInProgress: false }); h.open = undefined; setLocale("en"); });
after(() => {
  client.clear(); hooks.deregister(); Reflect.deleteProperty(globalThis, "__stringHistory"); setLocale("en");
  for (const [catalog] of addedLabels) Reflect.deleteProperty(catalog, "detail.history.restoreBefore");
});
type HistoryProps = Parameters<typeof StringHistory>[0];
let rendered: ReactNode;
function CaptureHistory(props: HistoryProps) {
  const view = StringHistory(props);
  rendered = view;
  return view;
}
function render(projectKey = "project", entryId = "row", translation: string | null = "Current", options: Partial<Pick<HistoryProps, "busy" | "onRestore">> = {}) {
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(CaptureHistory, {
    projectKey, entryId, translation, busy: false, onRestore: async () => {}, ...options,
  })));
}
type HistoryButton = ReactElement<{ children?: ReactNode; disabled?: boolean; onClick: () => Promise<unknown> }>;
function restoreButtons() {
  const buttons: HistoryButton[] = [];
  function visit(node: ReactNode) {
    if (Array.isArray(node)) node.forEach(visit);
    else if (isValidElement<{ children?: ReactNode }>(node)) {
      if (node.type === "button" && !("aria-expanded" in node.props)) buttons.push(node as HistoryButton);
      visit(node.props.children);
    }
  }
  visit(rendered);
  return buttons;
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

for (const [locale, beforeLabel] of [["en", "Restore before"], ["es", "Restaurar anterior"]] as const) {
  test(`${locale}: a single V1 → V2 revision offers Restore before and restores V1 through onRestore`, async () => {
    setLocale(locale); h.open = true;
    client.setQueryData(key, [{ ...revision, old_translation: "V1", new_translation: "V2" }]);
    const restored: string[] = [];
    const html = render("project", "row", "V2", { onRestore: async value => { restored.push(value); } });
    assert.ok(html.includes(beforeLabel));
    const buttons = restoreButtons();
    assert.deepEqual(buttons.map(button => button.props.children), [beforeLabel]);
    await buttons[0].props.onClick();
    assert.deepEqual(restored, ["V1"]);
  });
}

for (const current of ["V2", null] as const) {
  test(`Restore before restores an empty old translation over ${JSON.stringify(current)}`, async () => {
    h.open = true;
    client.setQueryData(key, [{ ...revision, old_translation: "", new_translation: current }]);
    const restored: string[] = [];
    render("project", "row", current, { onRestore: async value => { restored.push(value); } });
    const buttons = restoreButtons();
    assert.deepEqual(buttons.map(button => button.props.children), ["Restore before"]);
    await buttons[0].props.onClick();
    assert.deepEqual(restored, [""]);
  });
}

test("a null old translation offers no Restore before action", () => {
  h.open = true;
  client.setQueryData(key, [{ ...revision, old_translation: null }]);
  render();
  assert.deepEqual(restoreButtons().map(button => button.props.children), ["Restore this version"]);
});

for (const current of ["Old", ""] as const) {
  test(`old text equal to current ${JSON.stringify(current)} hides Restore before while keeping Restore after`, async () => {
    h.open = true;
    client.setQueryData(key, [{ ...revision, old_translation: current }]);
    const restored: string[] = [];
    render("project", "row", current, { onRestore: async value => { restored.push(value); } });
    const buttons = restoreButtons();
    assert.deepEqual(buttons.map(button => button.props.children), ["Restore this version"]);
    await buttons[0].props.onClick();
    assert.deepEqual(restored, ["New"]);
  });
}

test("the oldest retained revision can restore its before text without an earlier history row", async () => {
  h.open = true;
  client.setQueryData(key, [
    { ...revision, id: 50, old_translation: "V2", new_translation: "V3" },
    { ...revision, id: 49, old_translation: "V1", new_translation: "V2", changed_at: "2026-10-18T12:00:00.123Z" },
  ]);
  const restored: string[] = [];
  render("project", "row", "V3", { onRestore: async value => { restored.push(value); } });
  const beforeButtons = restoreButtons().filter(button => button.props.children === "Restore before");
  assert.equal(beforeButtons.length, 2);
  await beforeButtons[1].props.onClick();
  assert.deepEqual(restored, ["V1"]);
});

test("distinct before and after actions restore their respective values through the same callback", async () => {
  h.open = true;
  client.setQueryData(key, [revision]);
  const restored: string[] = [];
  render("project", "row", "Current", { onRestore: async value => { restored.push(value); } });
  const buttons = restoreButtons();
  assert.equal(buttons.length, 2);
  for (const [label, value] of [["Restore before", "Old"], ["Restore this version", "New"]]) {
    const button = buttons.find(button => button.props.children === label);
    assert.ok(button, label);
    await button.props.onClick();
    assert.equal(restored[restored.length - 1], value);
  }
});

for (const target of ["Same", ""] as const) {
  test(`identical before and after text ${JSON.stringify(target)} produces only the existing Restore action`, async () => {
    h.open = true;
    client.setQueryData(key, [{ ...revision, old_translation: target, new_translation: target }]);
    const restored: string[] = [];
    render("project", "row", "Current", { onRestore: async value => { restored.push(value); } });
    const buttons = restoreButtons();
    assert.deepEqual(buttons.map(button => button.props.children), ["Restore this version"]);
    await buttons[0].props.onClick();
    assert.deepEqual(restored, [target]);
  });
}

test("busy history disables both before and after restore actions", () => {
  h.open = true;
  client.setQueryData(key, [revision]);
  render("project", "row", "Current", { busy: true });
  const buttons = restoreButtons();
  assert.equal(buttons.length, 2);
  assert.ok(buttons.every(button => button.props.disabled === true));
});

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
