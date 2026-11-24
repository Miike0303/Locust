import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { setLocale, t } from "../lib/i18n";
import type { QueueItem } from "../stores/queueStore";

const item: QueueItem = { id: "error", projectName: "Game", projectPath: "/game", formatId: null,
  status: "error", error: "queue.interrupted", validationIssues: null, validationError: null,
  progress: { completed: 3, total: 5, costSoFar: 9, costIsComplete: false, startedAt: null } };
const state = {
  items: [item, { ...item, id: "cancelled", status: "cancelled", error: null }], isPanelOpen: true, isRunning: false,
  disconnected: false, reconnectJob() {}, cancelQueue() {},
  persistenceError: false, recoveryAvailable: false, retrySave() {},
  translationParams: null,
  history: [{ id: "history", projectName: "Previous game", projectPath: "/previous", status: "done",
    finishedAt: "2026-10-19T12:00:00.000Z", completed: 3, total: 5, costSoFar: 9, costIsComplete: false, validationIssues: 2 }],
};
(globalThis as any).__queuePanel = state;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/QueuePanel.tsx") && specifier === "../stores/queueStore") {
      return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("export const useQueueStore=()=>globalThis.__queuePanel;")}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: QueuePanel } = await import("./QueuePanel");
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__queuePanel"); setLocale("en"); });
function render(seed?: (client: QueryClient) => void) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seed?.(client);
  try {
    return renderToStaticMarkup(createElement(QueryClientProvider, { client },
      createElement(MemoryRouter, null, createElement(QueuePanel))));
  } finally { client.clear(); }
}
for (const locale of ["en", "es"] as const) {
  test(`QueuePanel renders accessible retry controls and collapsed history in ${locale}`, () => {
    setLocale(locale); state.isRunning = false;
    const html = render();
    assert.ok(html.includes(locale === "en" ? "Retry failed (2)" : "Reintentar fallidos (2)"));
    assert.equal((html.match(new RegExp(`aria-label="${t("queue.retryItem", { name: "Game" })}"`, "g")) ?? []).length, 2);
    assert.ok(html.includes(t("queue.interrupted")));
    assert.ok(html.includes(locale === "en" ? "Recent batches" : "Lotes recientes"));
    assert.match(html, /<details[\s>]/); assert.doesNotMatch(html, /<details[^>]*\bopen[\s=>]/);
    assert.ok(html.includes(t("queue.clearHistory")));
    assert.ok(html.includes("Previous game"));
    assert.ok(html.includes(t("queue.status.done")));
    assert.ok(html.includes("3/5"));
    assert.ok(html.includes(t("cost.unknown"))); assert.ok(!html.includes("$9.0000"));
    assert.ok(html.includes(new Date(state.history[0].finishedAt).toLocaleString(locale)));
    state.history[0].costIsComplete = true;
    assert.ok(render().includes("$9.0000"));
    state.history[0].costIsComplete = false;
  });
  test(`QueuePanel disables all retry buttons during a run in ${locale}`, () => {
    setLocale(locale); state.isRunning = true;
    const buttons = render().match(/<button\b[^>]*>[\s\S]*?<\/button>/g) ?? [];
    const retries = buttons.filter(button => button.includes(t("queue.retryItem", { name: "Game" })) || button.includes(t("queue.retryFailed", { count: 2 })));
    assert.equal(retries.length, 3);
    assert.ok(retries.every(button => button.includes('disabled=""')));
    state.isRunning = false;
  });
}

for (const locale of ["en", "es"] as const) {
  test(`QueuePanel keeps Reconnect and Cancel enabled while disconnected in ${locale}`, () => {
    setLocale(locale); state.isRunning = true; state.disconnected = true;
    try {
      const html = render();
      assert.ok(html.includes(locale === "en" ? "Connection lost" : "Conexión perdida"));
      assert.ok(html.includes(t("queue.connectionLostHint")));
      const buttons = html.match(/<button\b[^>]*>[\s\S]*?<\/button>/g) ?? [];
      for (const label of [locale === "en" ? "Reconnect" : "Reconectar", t("queue.cancel")]) {
        const button = buttons.find(button => button.includes(label));
        assert.ok(button, `expected ${label} action`);
        assert.ok(!button.includes('disabled=""'));
      }
      assert.ok(!buttons.some(button => button.includes(t("queue.start", { count: 0 }))));
      state.disconnected = false;
      assert.ok(!render().includes(t("queue.connectionLostHint")));
    } finally { state.isRunning = false; state.disconnected = false; }
  });
}


for (const locale of ["en", "es"] as const) {
  test(`QueuePanel keeps a localized unsaved warning and save retry available during execution in ${locale}`, () => {
    setLocale(locale); state.persistenceError = true; state.isRunning = true;
    try {
      const html = render();
      assert.match(html, /role="alert"/);
      assert.ok(html.includes(locale === "en" ? "Queue not saved" : "Cola sin guardar"));
      assert.ok(html.includes(t("queue.notSavedHint")));
      const button = (html.match(/<button\b[^>]*>[\s\S]*?<\/button>/g) ?? []).find(b => b.includes(t("queue.retrySave")));
      assert.ok(button); assert.ok(!button.includes('disabled=""'));
      state.persistenceError = false;
      assert.ok(!render().includes(t("queue.notSavedHint")));
      state.recoveryAvailable = true;
      assert.ok(render().includes(t("queue.recoveryAvailable")));
    } finally { state.persistenceError = false; state.recoveryAvailable = false; state.isRunning = false; }
  });
}

for (const locale of ["en", "es"] as const) {
  test(`QueuePanel disables Start and says why when the provider has no API key in ${locale}`, () => {
    setLocale(locale);
    const items = state.items;
    const mutable = state as any;
    mutable.items = [...items, { ...item, id: "pending", status: "pending", error: null }];
    mutable.translationParams = { provider_id: "openai", options: { source_lang: "ja", target_lang: "en" }, fallback_provider_ids: [] };
    const seed = (key: string | null) => (client: QueryClient) => {
      client.setQueryData(["providers"], [
        { id: "openai", name: "OpenAI", requires_api_key: true, is_free: false, configured: true },
        { id: "mock", name: "Mock", requires_api_key: false, is_free: true, configured: true },
      ]);
      client.setQueryData(["config"], { providers: key ? { openai: { api_key: key } } : {} });
      client.setQueryData(["formats"], []);
    };
    try {
      const startLabel = t("queue.start", { count: 1 });
      const start = (html: string) => (html.match(/<button\b[^>]*>[\s\S]*?<\/button>/g) ?? []).find(b => b.includes(startLabel));
      const missing = render(seed(null));
      const button = start(missing);
      assert.ok(button, "Start is rendered");
      assert.ok(button.includes('disabled=""'), "Start is disabled without the API key");
      const reason = button.match(/aria-describedby="([^"]+)"/);
      assert.ok(reason, "Start points at the reason it is disabled");
      assert.match(missing, new RegExp(`id="${reason[1]}"[^>]*>${t("queue.needsKey")}`));
      const ready = start(render(seed("***")));
      assert.ok(ready && !ready.includes('disabled=""'), "a saved key enables Start");
      assert.ok(!ready.includes("aria-describedby"));
      mutable.translationParams = { provider_id: "mock", options: { source_lang: "ja", target_lang: "en" }, fallback_provider_ids: [] };
      const keyless = start(render(seed(null)));
      assert.ok(keyless && !keyless.includes('disabled=""'), "a provider without a key requirement stays startable");
    } finally {
      mutable.items = items; mutable.translationParams = null;
    }
  });
}
