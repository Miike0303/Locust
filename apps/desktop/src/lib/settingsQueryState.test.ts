import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, before, test } from "node:test";
import * as React from "react";
import * as ReactQuery from "@tanstack/react-query";
import * as Router from "react-router-dom";
import * as i18n from "./i18n/index.ts";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { setLocale, translate } from "./i18n/index.ts";
import { en } from "./i18n/en.ts";
import { es } from "./i18n/es.ts";
import { SETTINGS_SECTIONS, type SettingsSectionId } from "./settingsNav.ts";
import type { AppConfig, GlossaryEntry, ProjectInfo, ProviderInfo, TranslationRun } from "./api.ts";

// SSR keeps the real hooks and QueryClient. Interactive regressions use the
// same dependency-aware hook seam as Review.test.tsx, with real query storage.
const glossary = {
  active: false, project: null as ProjectInfo | null,
  React, ReactQuery, Router, i18n,
  values: [] as any[], cursor: 0, dirty: false, effects: [] as (() => void)[],
  client: new QueryClient({ defaultOptions: { queries: { gcTime: Infinity, staleTime: 30_000 } } }),
  writes: [] as unknown[][], invalidations: [] as readonly unknown[][],
  toasts: [] as string[], refetches: 0, waitForSave: null as Promise<void> | null,
  state(initial: any) {
    const index = this.cursor++;
    if (!(index in this.values)) this.values[index] = typeof initial === "function" ? initial() : initial;
    return [this.values[index], (value: any) => {
      const next = typeof value === "function" ? value(this.values[index]) : value;
      if (!Object.is(next, this.values[index])) this.dirty = true;
      this.values[index] = next;
    }];
  },
  effect(callback: () => void, deps: unknown[]) {
    const index = this.cursor++;
    const previous = this.values[index];
    if (previous && deps.every((dep, i) => Object.is(dep, previous[i]))) return;
    this.values[index] = deps;
    this.effects.push(callback);
  },
  query(options: any) {
    const query = this.client.getQueryCache().build(this.client, options);
    return { data: query.state.data, isLoading: false, isError: false,
      refetch: async () => { this.refetches++; } };
  },
};
(globalThis as any).__glossaryTest = glossary;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/pages/Settings.tsx")) {
      const stubs: Record<string, string> = {
        react: `const h=globalThis.__glossaryTest;
          export const useState=x=>h.active?h.state(x):h.React.useState(x);
          export const useEffect=(fn,deps)=>h.active?h.effect(fn,deps):h.React.useEffect(fn,deps);
          export const useMemo=(fn,deps)=>h.active?fn():h.React.useMemo(fn,deps);`,
        "@tanstack/react-query": `const h=globalThis.__glossaryTest;
          export const useQuery=o=>h.active?h.query(o):h.ReactQuery.useQuery(o);
          export const useQueryClient=()=>h.active?{invalidateQueries:async o=>{h.invalidations.push(o.queryKey);await h.client.invalidateQueries(o);}}:h.ReactQuery.useQueryClient();`,
        "react-router-dom": `const h=globalThis.__glossaryTest;
          export const useLocation=()=>h.active?{search:'?section=glossary'}:h.Router.useLocation();
          export const useNavigate=()=>h.active?()=>{}:h.Router.useNavigate();`,
        "../lib/i18n": `const h=globalThis.__glossaryTest;
          export const useT=()=>h.active?h.i18n.t:h.i18n.useT();export const useLocale=()=>h.i18n.useLocale();`,
        "../stores/projectStore": `const h=globalThis.__glossaryTest;
          export const useProjectStore=select=>select(h);useProjectStore.getState=()=>h;`,
        "../stores/toastStore": `export const addToast=(_type,message)=>globalThis.__glossaryTest.toasts.push(message);`,
        "../lib/api": `export * from ${JSON.stringify(new URL("./api.ts", import.meta.url).href)};
          const h=globalThis.__glossaryTest;
          export const addGlossaryEntry=async entry=>{h.writes.push(['add',entry]);if(h.waitForSave)await h.waitForSave;};
          export const deleteGlossaryEntry=async (...args)=>{h.writes.push(['delete',...args]);if(h.waitForSave)await h.waitForSave;};`,
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});

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
  hooks.deregister();
  glossary.client.clear();
  Reflect.deleteProperty(globalThis, "__glossaryTest");
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
  glossary.project = glossaryProject("/projects/fixture.locust.db");
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } },
  });
  try {
    client.setQueryData(["config"], { default_source_lang: "ja", default_target_lang: "en" });
    if (section === "glossary") {
      await assert.rejects(client.fetchQuery({
        queryKey: ["glossary", glossary.project.database_path, "ja-en"], queryFn: () => api.getGlossary("ja-en"),
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
    glossary.project = null;
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
      assertSemanticPalette(html);
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
      assertSemanticPalette(html);
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
      assertSemanticPalette(html);
    });
  }
}

function assertSemanticPalette(html: string): void {
  const looseGrays = html.match(/(?:text|border)-gray-[\w/]+/g) ?? [];
  assert.equal(looseGrays.length, 0, `Settings still renders loose grays: ${looseGrays.slice(0, 5).join(", ")}`);
}

function glossaryProject(database: string | undefined, path = "/games/shared"): ProjectInfo {
  return { path, database_path: database, format_id: "html", name: "Fixture" };
}
function glossaryNodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(glossaryNodes);
  return [node, ...glossaryNodes(node.props?.children)];
}
function renderGlossary() {
  for (let attempt = 0; attempt < 20; attempt++) {
    glossary.cursor = 0; glossary.dirty = false;
    const section = glossaryNodes(Settings()).find(node => node.type?.name === "GlossarySection");
    assert.ok(section);
    const tree = section.type(section.props);
    for (const effect of glossary.effects.splice(0)) effect();
    if (!glossary.dirty) return tree;
  }
  throw new Error("Glossary did not settle");
}
function glossaryNode(predicate: (node: any) => boolean) {
  const node = glossaryNodes(renderGlossary()).find(predicate);
  assert.ok(node, "missing glossary control");
  return node;
}
function confirmation() { return glossaryNode(node => node.props?.confirmLabel === i18n.t("common.delete")); }
function seedGlossary(translation: string) {
  renderGlossary();
  const query = glossary.client.getQueryCache().getAll().filter(q => q.queryKey[0] === "glossary").slice(-1)[0];
  const entry: GlossaryEntry = { term: "HP", translation, lang_pair: "ja-en", context: null, case_sensitive: false };
  glossary.client.setQueryData(query.queryKey, [entry]);
  return query.queryKey;
}
function startDelete() {
  glossaryNode(node => node.props?.title === i18n.t("settings.glossary.deleteTitle")).props.onClick();
  return confirmation();
}
function prepareAdd() {
  for (const [key, value] of [["settings.glossary.sourceTerm", "HP"], ["settings.glossary.preferredTranslation", "Health Points"]] as const) {
    glossaryNode(node => node.props?.placeholder === i18n.t(key)).props.onChange({ target: { value } });
  }
  return glossaryNode(node => node.type === "button" && node.props?.onClick?.name === "handleAdd").props.onClick;
}
function glossaryTest(name: string, run: () => void | Promise<void>) {
  test(name, async () => {
    setLocale("en"); glossary.active = true;
    glossary.project = glossaryProject("/projects/A.locust.db");
    glossary.values = []; glossary.effects = []; glossary.writes = [];
    glossary.invalidations = []; glossary.toasts = []; glossary.refetches = 0;
    glossary.waitForSave = null; glossary.client.clear();
    try { await run(); } finally { glossary.active = false; glossary.project = null; }
  });
}

glossaryTest("glossary cache separates databases sharing a game path within the freshness window", () => {
  const firstKey = seedGlossary("Health Points");
  assert.ok(JSON.stringify(renderGlossary()).includes("Health Points"));
  glossary.project = glossaryProject("/projects/B.locust.db");
  assert.ok(!JSON.stringify(renderGlossary()).includes("Health Points"), "B must not show A's cached row");
  const secondKey = seedGlossary("Hit Points");
  assert.ok(firstKey.includes("/projects/A.locust.db"));
  assert.ok(secondKey.includes("/projects/B.locust.db"));
  assert.ok(JSON.stringify(renderGlossary()).includes("Hit Points"));
});

glossaryTest("glossary falls back to the game path when no database path is available", () => {
  glossary.project = glossaryProject(undefined, "/games/A");
  const firstKey = seedGlossary("Health Points");
  glossary.project = glossaryProject(undefined, "/games/B");
  assert.ok(!JSON.stringify(renderGlossary()).includes("Health Points"));
  assert.ok(firstKey.includes("/games/A"));
  assert.ok(seedGlossary("Hit Points").includes("/games/B"));
});

glossaryTest("switching databases resets a pending glossary delete confirmation", () => {
  seedGlossary("Health Points");
  assert.equal(startDelete().props.open, true);
  glossary.project = glossaryProject("/projects/B.locust.db");
  assert.equal(confirmation().props.open, false);
  assert.deepEqual(glossary.writes, []);
});

for (const locale of ["en", "es"] as const) {
  for (const action of ["add", "delete"] as const) {
    glossaryTest(`${locale}: stale glossary ${action} refuses a changed project before rerender with a visible message`, async () => {
      setLocale(locale);
      seedGlossary("Health Points");
      const submit = action === "delete" ? startDelete().props.onConfirm : prepareAdd();
      glossary.project = glossaryProject("/projects/B.locust.db");
      await submit();
      assert.deepEqual(glossary.writes, [], "an A callback must not mutate the current B database");
      assert.ok(glossary.toasts.includes(locale === "en"
        ? "The open project changed. Try again in the current project's glossary."
        : "El proyecto abierto cambió. Intente de nuevo en el glosario del proyecto actual."));
    });
  }
}

for (const action of ["add", "delete"] as const) {
  glossaryTest(`a completed glossary ${action} invalidates only its captured project without refetching after a switch`, async () => {
    seedGlossary("Health Points");
    const submit = action === "delete" ? startDelete().props.onConfirm : prepareAdd();
    let finish!: () => void;
    glossary.waitForSave = new Promise<void>(resolve => { finish = resolve; });
    const saving = submit();
    assert.equal(glossary.writes.length, 1);
    glossary.project = glossaryProject("/projects/B.locust.db");
    const secondKey = seedGlossary("Hit Points");
    finish(); await saving;
    assert.equal(glossary.refetches, 0, "an old query must not read the current project's backend");
    assert.deepEqual(glossary.invalidations, [["glossary", "/projects/A.locust.db"]]);
    assert.equal(glossary.client.getQueryState(secondKey)?.isInvalidated, false);
  });
}

async function renderPopulatedSection(section: SettingsSectionId): Promise<string> {
  glossary.project = glossaryProject("/projects/fixture.locust.db");
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } },
  });
  try {
    const providers: ProviderInfo[] = [
      { id: "mock", name: "Mock", is_free: true, requires_api_key: false, configured: true },
      ...["openai", "claude", "grok"].map(id => ({
        id, name: id, is_free: false, requires_api_key: true, configured: true,
      })),
      ...["argos", "ollama", "grok-sub"].map(id => ({
        id, name: id, is_free: id !== "grok-sub", requires_api_key: false, configured: true,
      })),
    ];
    const config: AppConfig = {
      providers: { openai: { api_key: "***", model: "gpt-4o-mini" } },
      default_provider: "mock", default_source_lang: "ja", default_target_lang: "en",
      default_batch_size: 20, default_cost_limit: null, recent_projects: [],
      ui: { theme: "dark", font_size: 18, show_source_column: true, table_row_height: 36 },
    };
    const run: TranslationRun = {
      id: 1, started_at: "2026-01-01T00:00:00Z", provider: "mock", source_lang: "ja", target_lang: "en",
      strings_translated: 2, tokens_used: 12, input_tokens: 8, output_tokens: 4,
      cost_usd: 0, cost_is_complete: true, duration_secs: 3,
    };
    client.setQueryData(["config"], config);
    client.setQueryData(["providers"], providers);
    client.setQueryData(["glossary", glossary.project.database_path, "ja-en"], [
      { term: "Greeting", translation: "Hello", lang_pair: "ja-en", context: null, case_sensitive: false },
    ]);
    client.setQueryData(["translation-runs"], [run]);
    client.setQueryData(["backups", "report"], { entries: [healthyBackup], unreadable: [] });
    return renderToStaticMarkup(createElement(QueryClientProvider, { client },
      createElement(MemoryRouter, { initialEntries: [`/settings?section=${section}`] },
        createElement(Settings))));
  } finally { client.clear(); glossary.project = null; }
}

for (const [locale, catalog] of [["en", en], ["es", es]] as const) {
  for (const { id: section, labelKey } of SETTINGS_SECTIONS) {
    test(`${locale} ${section} uses semantic colors and retains localized headings and controls`, async () => {
      setLocale(locale);
      const html = await renderPopulatedSection(section);
      assert.ok(html.includes(catalog[`settings.${section}.title`]), "localized section heading");
      for (const { labelKey: navLabel } of SETTINGS_SECTIONS) {
        assert.ok(html.includes(catalog[navLabel]), `localized navigation: ${navLabel}`);
      }
      assert.ok(html.includes(`aria-current="page"`));
      assert.ok(html.includes(`>${catalog[labelKey]}</button>`), "active section label");
      const retainedText = {
        providers: [catalog["settings.providers.testConnection"], catalog["settings.providers.needsApiKey"]],
        defaults: [catalog["settings.defaults.sourceLang"], catalog["settings.defaults.langHint"]],
        appearance: [catalog["settings.appearance.interfaceLanguage"], catalog["settings.appearance.tableRowHeightHint"]],
        glossary: [catalog["settings.glossary.addEntry"], "Greeting", "Hello"],
        history: [catalog["settings.history.col.provider"], catalog["settings.history.col.cost"]],
        data: [catalog["settings.data.backups"], "/games/Demo", "healthy"],
      }[section];
      for (const text of retainedText) assert.ok(html.includes(text), `retained copy: ${text}`);
      assertSemanticPalette(html);
    });
  }
}
