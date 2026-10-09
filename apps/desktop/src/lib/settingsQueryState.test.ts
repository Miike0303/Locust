import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { setLocale, translate } from "./i18n/index.ts";
import { en } from "./i18n/en.ts";
import { es } from "./i18n/es.ts";

let Settings: typeof import("../pages/Settings.tsx").default;
let api: typeof import("./api.ts");
let backendError = "no project open";
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
const originalFetch = globalThis.fetch;

before(async () => {
  // Exercise the real API's error wrapping without a backend or network.
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { __TAURI_INTERNALS__: { invoke: async (command: string) => {
      if (command === "get_server_port") return 7842;
      throw backendError;
    } } },
  });
  globalThis.fetch = async () => new Response(backendError, {
    status: backendError === "no project open" ? 404 : 500,
  });
  api = await import("./api.ts");
  Settings = (await import("../pages/Settings.tsx")).default;
});

after(() => {
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  globalThis.fetch = originalFetch;
  setLocale("en");
});

test("query failures retain their stable identity across localization and locale changes", async () => {
  const { ApiError } = await import("./apiError.ts");
  const { settingsQueryState } = await import("./settingsQueryState.ts");
  for (const locale of ["en", "es"] as const) {
    setLocale(locale);
    for (const raw of ["no project open", "404: no project open", " 404: no project open \n"]) {
      const error = new ApiError(raw);
      assert.equal(error.message, (locale === "en" ? en : es)["api.error.noProjectOpen"]);
      setLocale(locale === "en" ? "es" : "en");
      assert.equal(settingsQueryState(error), "no_project");
      setLocale(locale);
    }
    for (const raw of ["404: path not found", "500: database unavailable", "no project open: unexpected detail"]) {
      assert.equal(settingsQueryState(new ApiError(raw)), "failed");
    }
    for (const error of [new Error("offline"), null, undefined, "unknown failure"]) {
      assert.equal(settingsQueryState(error), "failed");
    }
  }
});

async function renderQueryFailure(section: "glossary" | "history", raw: string): Promise<string> {
  backendError = raw;
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } },
  });
  try {
    client.setQueryData(["config"], { default_source_lang: "ja", default_target_lang: "en" });
    if (section === "glossary") {
      await assert.rejects(client.fetchQuery({
        queryKey: ["glossary", "ja-en"], queryFn: () => api.getGlossary("ja-en"),
      }));
    } else {
      await assert.rejects(client.fetchQuery({
        queryKey: ["translation-runs"], queryFn: api.getTranslationRuns,
      }));
    }
    return renderToStaticMarkup(createElement(QueryClientProvider, { client },
      createElement(MemoryRouter, { initialEntries: [`/settings?section=${section}`] },
        createElement(Settings))));
  } finally {
    client.clear();
  }
}

for (const [locale, catalog, hints] of [
  ["en", en, {
    glossary: "Open a project to see its glossary.",
    history: "Open a project to see its translation history.",
  }],
  ["es", es, {
    glossary: "Abra un proyecto para ver su glosario.",
    history: "Abra un proyecto para ver su historial de traducción.",
  }],
] as const) {
  for (const section of ["glossary", "history"] as const) {
    test(`${locale} ${section} shows a project hint instead of an empty list or load error`, async () => {
      setLocale(locale);
      const html = await renderQueryFailure(section, "no project open");
      assert.ok(html.includes(hints[section]), html);
      assert.ok(!html.includes(catalog[`settings.${section}.empty`]));
      assert.ok(!html.includes(catalog["api.error.noProjectOpen"]));
    });

    test(`${locale} ${section} keeps unrelated query failures visible`, async () => {
      setLocale(locale);
      const html = await renderQueryFailure(section, "database unavailable");
      const error = section === "history"
        ? translate(catalog, locale, "api.error.http", { status: 500, body: "database unavailable" })
        : "database unavailable";
      const message = translate(catalog, locale, `settings.${section}.loadFailed`, { error });
      assert.ok(html.includes(message), html);
      assert.ok(!html.includes(hints[section]));
      assert.ok(!html.includes(catalog[`settings.${section}.empty`]));
    });
  }
}

const healthyBackup = {
  id: "healthy", path: "/backups/healthy", source_path: "/games/Demo",
  created_at: "2026-01-01T00:00:00Z", file_count: 2, size_bytes: 16,
};

async function renderBackups(state: "loading" | "failed" | "empty" | "damaged" | "mixed"): Promise<string> {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } },
  });
  try {
    // An old array cache must not satisfy the new report query.
    if (state !== "loading" && state !== "failed") client.setQueryData(["backups"], []);
    if (state === "failed") {
      await assert.rejects(client.fetchQuery({
        queryKey: ["backups", "report"], queryFn: async () => { throw new Error("backup root unavailable"); },
      }));
    } else if (state !== "loading") {
      client.setQueryData(["backups", "report"], {
        entries: state === "mixed" ? [healthyBackup] : [],
        unreadable: state === "empty" ? [] : [
          { id: "damaged<id>", error: `manifest\n  unreadable <unsafe> ${"x".repeat(300)}` },
        ],
      });
    }
    return renderToStaticMarkup(createElement(QueryClientProvider, { client },
      createElement(MemoryRouter, { initialEntries: ["/settings?section=data"] },
        createElement(Settings))));
  } finally { client.clear(); }
}

for (const [locale, catalog, damagedTitle, failedText] of [
  ["en", en, "Damaged backups", "Could not load backups: backup root unavailable"],
  ["es", es, "Copias de seguridad dañadas", "No se pudieron cargar las copias de seguridad: backup root unavailable"],
] as const) {
  for (const state of ["loading", "failed", "empty", "damaged", "mixed"] as const) {
    test(`${locale} Data renders ${state} backup state without misleading empty text or unsafe actions`, async () => {
      setLocale(locale);
      const html = await renderBackups(state);
      assert.equal(html.includes(catalog["settings.data.noBackups"]), state === "empty", html);
      assert.equal(html.includes(catalog["common.loading"]), state === "loading", html);
      assert.equal(html.includes(failedText), state === "failed", html);
      assert.equal(html.includes(damagedTitle), state === "damaged" || state === "mixed", html);
      assert.equal(html.includes('title="' + catalog["settings.data.restore"] + '"'), state === "mixed", html);
      assert.equal(html.includes('title="' + catalog["settings.data.delete"] + '"'), state === "mixed", html);
      if (state === "mixed") assert.ok(html.includes("/games/Demo") && html.includes("healthy"), html);
      if (state === "damaged" || state === "mixed") {
        assert.ok(html.includes("damaged&lt;id&gt;"), html);
        assert.ok(html.includes("manifest unreadable &lt;unsafe&gt;"), html);
        assert.ok(!html.includes("x".repeat(300)), "damaged diagnostics should stay short");
      }
    });
  }
}
