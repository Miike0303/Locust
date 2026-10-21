import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { PassThrough } from "node:stream";
import { createElement } from "react";
import { renderToPipeableStream } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "../App.tsx";
import { setLocale, t, type TranslateKey } from "./i18n/index.ts";

// Real App/Routes/Outlet/NavLink + lazy modules, not mocked router hooks.
// SSR does not run effects or browser history: the retained production browser
// differential covers replace/state clearing, popstate and error recovery.
const originalFetch = globalThis.fetch;
before(() => {
  setLocale("en");
  globalThis.fetch = async () => { throw new Error("Unexpected characterization backend request"); };
});
after(() => { globalThis.fetch = originalFetch; });

async function render(path: string): Promise<string> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: Infinity } },
  });
  client.setQueryData(["config"], {
    providers: {}, default_provider: null, default_source_lang: "ja", default_target_lang: "en",
    default_batch_size: 20, default_cost_limit: null, recent_projects: [],
    ui: { theme: "dark", font_size: 16, show_source_column: true, table_row_height: 36 },
  });
  client.setQueryData(["providers"], []);
  client.setQueryData(["formats"], []);
  try {
    return await new Promise<string>((resolve, reject) => {
      const output = new PassThrough();
      let html = "";
      output.on("data", chunk => { html += chunk.toString(); });
      output.on("end", () => resolve(html));
      output.on("error", reject);
      const stream = renderToPipeableStream(
        createElement(QueryClientProvider, { client },
          createElement(MemoryRouter, { initialEntries: [path] }, createElement(App))),
        { onAllReady() { stream.pipe(output); }, onError: reject },
      );
    });
  } finally {
    client.clear();
  }
}

const destinations = ["/", "/dashboard", "/editor", "/review", "/qa", "/speakers", "/memory", "/settings"];
const routes: [string, TranslateKey][] = [
  ["/", "welcome.headline"],
  ["/dashboard", "dashboard.title"],
  ["/editor", "editor.empty.title"],
  ["/review", "nav.review"],
  ["/qa", "qa.title"],
  ["/speakers", "speakers.title"],
  ["/memory", "memory.title"],
  ["/settings?section=providers", "settings.providers.title"],
  ["/missing/deep/link", "route.notFound.title"],
];

for (const [path, heading] of routes) {
  test(`real declarative router SSR: ${path} resolves its page and sidebar`, async () => {
    const html = await render(path);
    const escaped = t(heading).replaceAll("'", "&#x27;");
    assert.ok(html.includes(escaped), `page content: ${heading}`);
    assert.doesNotMatch(html, /This screen stopped working/);
    const sidebar = html.match(/<aside\b[\s\S]*?<\/aside>/)?.[0];
    assert.ok(sidebar);
    const links = [...sidebar.matchAll(/<a\b([^>]*)href="([^"]*)"([^>]*)>/g)];
    assert.deepEqual(links.filter(link => link[2].startsWith("/")).map(link => link[2]), destinations);
    const active = links.filter(link => /aria-current="page"/.test(link[1] + link[3])).map(link => link[2]);
    assert.deepEqual(active, path.startsWith("/missing") ? [] : [path.split("?")[0]]);
    // In particular Home must not be active for any non-root destination.
  });
}

for (const [section, key] of [
  ["defaults", "settings.defaults.title"], ["appearance", "settings.appearance.title"],
  ["glossary", "settings.glossary.title"], ["history", "settings.history.title"],
  ["data", "settings.data.title"], ["invalid", "settings.providers.title"],
] as const) {
  test(`real router SSR settings query selects ${section}`, async () => {
    const html = await render(`/settings?section=${section}`);
    assert.ok(html.includes(t(key)));
    assert.equal((html.match(/<button[^>]*aria-current="page"/g) ?? []).length, 1);
  });
}
