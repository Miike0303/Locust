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
function render() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
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
