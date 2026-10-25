import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { gameFileExtensions } from "../lib/gameSource.ts";
import { registerHooks } from "node:module";
import { draftProjectKey } from "../lib/projectScope.ts";
import { APP_VERSION } from "../lib/appVersion.ts";
import { readFileSync } from "node:fs";
import { useProjectStore } from "../stores/projectStore.ts";
import { setLocale, t, type TranslateKey } from "../lib/i18n/index.ts";
import type { AppConfig, PluginInfo, ProviderInfo } from "../lib/api.ts";

// Capture rendered native controls, then exercise their real Welcome handlers.
const actions = { buttons: [] as any[], opens: [] as any[], databases: [] as any[] };
(globalThis as any).__welcomeActions = actions;
const hooks = registerHooks({ resolve(specifier, context, nextResolve) {
  if (context.parentURL?.endsWith("/pages/Welcome.tsx")) {
    const actual = nextResolve(specifier, context).url;
    const source = specifier === "react/jsx-runtime"
      ? `import * as actual from ${JSON.stringify(actual)};export const Fragment=actual.Fragment;
        const wrap=fn=>(type,props,...rest)=>{if(type==='button')globalThis.__welcomeActions.buttons.push(props);return fn(type,props,...rest);};
        export const jsx=wrap(actual.jsx);export const jsxs=wrap(actual.jsxs);`
      : specifier === "../lib/openProjectFlow"
        ? `export * from ${JSON.stringify(actual)};
          export const completeOpenProject=async(...args)=>{globalThis.__welcomeActions.opens.push(args);return null;};
          export const completeOpenProjectDb=async(...args)=>{globalThis.__welcomeActions.databases.push(args);return {project_name:'Saved Project',total_strings:1};};` : null;
    if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
  }
  return nextResolve(specifier, context);
} });
const { default: Welcome } = await import("./Welcome.tsx");

const originalFetch = globalThis.fetch;
before(() => {
  // Seed the query cache; no test may contact a backend.
  globalThis.fetch = async () => { throw new Error("Unexpected Welcome backend request"); };
});
after(() => {
  hooks.deregister();
  Reflect.deleteProperty(globalThis, "__welcomeActions");
  globalThis.fetch = originalFetch;
  setLocale("en");
});

const formats: PluginInfo[] = [
  { id: "godot-dialogic", name: "Godot / Dialogic", description: "Backend-only Godot description", extensions: ["pck"], supported_modes: ["replace"], stability: "experimental" },
  { id: "rpgmaker-mv", name: "RPG Maker MV/MZ", description: "Backend-only description",
    extensions: [".json"], supported_modes: ["add", "replace"], stability: "stable" },
  { id: "unity", name: "Unity", description: "Backend-only Unity description",
    extensions: [".assets", ".bundle"], supported_modes: ["replace"], stability: "experimental" },
  { id: "unknown-format", name: "Unknown engine", description: "Unknown format fallback",
    extensions: [".custom"], supported_modes: ["replace"] },
  { id: "hidden-format", name: "Hidden engine", description: "Hidden description",
    extensions: [".hidden"], supported_modes: [], stability: "comingsoon" },
];
const recentProjects: AppConfig["recent_projects"] = [
  { name: "Demo Game", path: "/games/Demo", format_id: "rpgmaker-mv", last_opened: "2026-01-01T00:00:00Z" },
  { name: "Saved Project", path: "/games/Saved", format_id: "unknown-format",
    database_path: "/projects/Saved.locust.db", last_opened: "" },
];
const readyProviders: ProviderInfo[] = [
  { id: "mock", name: "Mock", is_free: true, requires_api_key: false, configured: true },
];

type Scenario = "first-run" | "recent" | "current" | "ready" | "loading";
function renderWelcome(scenario: Scenario, recents = recentProjects, engines = formats, cachedProgress = false): string {
  actions.buttons = [];
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } },
  });
  const initialState = useProjectStore.getInitialState();
  const previousProject = initialState.project;
  initialState.project = scenario === "current"
    ? { name: "Current Game", path: "/games/Current", format_id: "renpy" } : null;
  try {
    if (scenario !== "loading") {
      const config: AppConfig = {
        providers: {}, default_provider: null, default_source_lang: "ja", default_target_lang: "en",
        default_batch_size: 20, default_cost_limit: null,
        ui: { theme: "dark", font_size: 18, show_source_column: true, table_row_height: 36 },
        recent_projects: scenario === "recent" ? recents : [],
      };
      client.setQueryData(["config"], config);
      client.setQueryData(["formats"], engines);
      if (cachedProgress) client.setQueryData(["stats", draftProjectKey({ ...recents[0], database_path: recents[0].database_path ?? undefined })],
        { total: 10, pending: 4, translated: 3, reviewed: 2, approved: 1, error: 0, total_cost_usd: 0 });
      client.setQueryData(["providers"], scenario === "recent" || scenario === "ready" ? readyProviders : []);
    }
    return renderToStaticMarkup(createElement(QueryClientProvider, { client },
      createElement(MemoryRouter, { initialEntries: ["/"] }, createElement(Welcome))));
  } finally {
    initialState.project = previousProject;
    client.clear();
  }
}

function hasText(html: string, text: string): boolean {
  // Use React's escaping for catalog entries containing punctuation or markup.
  const escaped = renderToStaticMarkup(createElement("span", null, text)).slice(6, -7);
  return html.includes(escaped);
}

for (const locale of ["en", "es"] as const) {
  for (const scenario of ["first-run", "recent", "ready", "loading"] as const) {
    test(`${locale} Welcome ${scenario} retains localized content and uses semantic surfaces, copy and chips`, () => {
      setLocale(locale);
      const html = renderWelcome(scenario);
      const controls: TranslateKey[] = [
        "welcome.headline", "welcome.tagline", "welcome.openFolder", "welcome.openFile",
        "welcome.openDb", "welcome.applyPatch", "recovery.action", "welcome.chooseFormat",
      ];
      for (const key of controls) assert.ok(hasText(html, t(key)), `localized content: ${key}`);
      const showHint = scenario === "first-run";
      assert.equal(hasText(html, t("welcome.providerHintLink")), showHint);
      assert.equal(html.includes(`aria-label="${t("welcome.providerHintDismiss")}"`), showHint);
      assert.equal(hasText(html, t("welcome.recent")), scenario === "recent");
      assert.equal(hasText(html, t("welcome.supportedEngines")), scenario !== "loading");
      assert.ok(!hasText(html, t("welcome.recentCount", { count: scenario === "recent" ? 2 : 0 })));
      assert.ok(!hasText(html, t("welcome.formatsAvailable", { count: scenario === "loading" ? 0 : 4 })));
      assert.ok(html.includes(`v${APP_VERSION}`));
      if (scenario === "recent") {
        for (const text of ["Demo Game", "Saved Project", "/projects/Saved.locust.db", t("welcome.recentDbBadge"),
          "/games/Saved", t("welcome.addToQueue")]) {
          assert.ok(hasText(html, text), `retained recent project: ${text}`);
        }
      }
      if (scenario !== "loading") {
        for (const text of ["Godot / Dialogic", t("format.desc.godot-dialogic"), "RPG Maker MV/MZ", "Unity", "Unknown engine", "Unknown format fallback",
          t("format.desc.rpgmaker-mv"), t("welcome.format.experimental"), ".json", ".assets", ".custom"]) {
          assert.ok(hasText(html, text), `retained format content: ${text}`);
        }
        assert.ok(!html.includes("Hidden engine") && !html.includes("Backend-only"), "format visibility and localization stay unchanged");
      }
      const looseGrays = html.match(/[\w:/-]*gray-[\w/.-]+/g) ?? [];
      assert.equal(looseGrays.length, 0, `Welcome renders loose grays: ${looseGrays.slice(0, 5).join(", ")}`);
      assert.ok(!/text-\[\d+(?:\.\d+)?px\]/.test(html), "rendered type must scale with the font-size preference");
      // The bounded column must not paint its own background: outside it the body shows through.
      assert.ok(html.includes("mx-auto text-text text-body"), "page uses semantic base text on the body background");
      assert.ok(html.includes("bg-accent hover:bg-accent-hover"), "primary action uses the shared accent");
      assert.ok(html.includes("text-text-muted"), "muted copy uses the shared foreground");
      if (scenario !== "loading") {
        assert.ok(html.includes("border-border bg-surface"), "format cards use raised surfaces");
        assert.ok(/<span class="[^"]*text-text-muted/.test(html), "experimental chips use muted semantic colors");
      }
      if (showHint) assert.ok(html.includes("bg-warning-muted"), "provider guidance uses semantic warning colors");
    });
  }
}


for (const locale of ["en", "es"] as const) {
  for (const scenario of ["first-run", "recent", "current"] as const) {
    test(`${locale} Home ${scenario}: one primary, useful headline and progressive disclosure`, () => {
      setLocale(locale);
      const html = renderWelcome(scenario);
      const primaries = [...html.matchAll(/<button\b[^>]*class="[^"]*\bbg-accent [^"]*"[^>]*>([\s\S]*?)<\/button>/g)];
      assert.equal(primaries.length, 1);
      const name = scenario === "current" ? "Current Game" : "Demo Game";
      const label = scenario === "first-run" ? t("welcome.openFolder") : t("welcome.continue", { name });
      assert.ok(hasText(primaries[0][0], label), `primary is ${label}`);
      assert.ok(hasText(html, locale === "en" ? "Translate a game's dialogue" : "Traduzca los diálogos de un juego"));
      assert.ok(hasText(html, locale === "en" ? "Open a single file" : "Abrir un solo archivo"));
      assert.equal((html.match(/<h1\b/g) ?? []).length, 1);
      const rail = readFileSync(new URL("../components/Layout.tsx", import.meta.url), "utf8");
      assert.doesNotMatch(rail, /<h1\b/);
      const details = [...html.matchAll(/<details([^>]*)>([\s\S]*?)<\/details>/g)];
      assert.equal(details.length, 1);
      for (const disclosure of details) assert.doesNotMatch(disclosure[1], /\bopen\b/);
      const other = details.find(d => hasText(d[2], t("welcome.otherWays")));
      assert.ok(other, "native disclosure exposes the alternate entry points on expansion");
      for (const key of ["welcome.openDb", "welcome.applyPatch", "recovery.action", "welcome.chooseFormat"] as const) {
        assert.ok(hasText(other[2], t(key)), `reachable entry point ${key}`);
      }
      for (const key of ["welcome.openFolder", "welcome.openFile"] as const) assert.ok(hasText(html, t(key)));
      const strip = html.slice(html.indexOf('aria-labelledby="supported-engines-title"'));
      assert.ok(strip.indexOf("RPG Maker MV/MZ") < strip.indexOf("Godot / Dialogic"));
      assert.ok(strip.indexOf("Unknown engine") < strip.indexOf("Godot / Dialogic"));
      assert.equal((strip.match(new RegExp(`>\\s*${t("welcome.format.experimental")}<`, "g")) ?? []).length, 2);
      assert.match(html, /<a[^>]*href="https:\/\/github.com\/Miike0303\/Locust"[^>]*target="_blank"[^>]*rel="noopener noreferrer"/);
      assert.doesNotMatch(html, /uppercase|bg-(?:pink|blue|indigo|purple|orange|cyan|violet|rose|teal|lime)-/);
    });
  }
}

test("Home retains existing Continue paths and accessible manual-picker safety", () => {
  const source = readFileSync(new URL("./Welcome.tsx", import.meta.url), "utf8");
  assert.match(source, /onClick=\{\(\) => project \? navigate\("\/editor"\) : openRecent\(recentProjects\[0\]\)\}/);
  assert.match(source, /aria-label=\{t\("common.close"\)\}/);
  assert.equal((source.match(/t\("welcome.format.experimental"\)/g) ?? []).length, 2, "picker and grid keep badges");
});

test("Welcome and Queue's shared native file filter includes registered PCKs and existing shortcuts", () => {
  const extensions = gameFileExtensions(formats);
  for (const extension of ["pck", "exe", "rpgproject", "rvproj2", "json", "assets", "bundle"]) assert.ok(extensions.includes(extension));
  assert.ok(!extensions.includes("hidden") && !extensions.includes(".json"));
  assert.equal(new Set(extensions).size, extensions.length);
});

for (const locale of ["en", "es"] as const) {
  test(`${locale}: recent card shows at most six accessible rows with engine badges, relative dates and folder tooltips`, () => {
    setLocale(locale);
    const recents = Array.from({ length: 8 }, (_, i) => ({ ...recentProjects[0], name: `Game ${i}`, path: `/games/${i}`,
      last_opened: new Date(Date.now() - 2 * 86400000).toISOString() }));
    const html = renderWelcome("recent", recents);
    assert.match(html, /<section[^>]*aria-labelledby="recent-projects-title"/);
    assert.equal((html.match(/data-recent-project=/g) ?? []).length, 6);
    for (let i = 0; i < 6; i++) {
      assert.match(html, new RegExp(`<button[^>]*data-recent-project="/games/${i}"[^>]*class="[^"]*focus-visible:ring-2`));
      assert.ok(html.includes(`title="/games/${i}"`));
    }
    assert.ok(!html.includes('data-recent-project="/games/6"'));
    assert.ok(hasText(html, new Intl.RelativeTimeFormat(locale, { numeric: "auto" }).format(-2, "day")));
    assert.match(html, /rounded bg-surface-muted px-1.5 py-0.5 font-mono text-caption text-text-muted/);
    assert.match(html, /font-bold[^>]*>Game 0/);
    assert.doesNotMatch(renderWelcome("ready"), /recent-projects-title|data-recent-project=|role="progressbar"/);
  });

  test(`${locale}: recent progress uses only the matching project cache and FE-6 palette`, () => {
    setLocale(locale);
    assert.doesNotMatch(renderWelcome("recent"), /role="progressbar"/);
    const html = renderWelcome("recent", recentProjects, formats, true);
    assert.equal((html.match(/role="progressbar"/g) ?? []).length, 1);
    assert.match(html, /aria-valuenow="60"/);
    assert.ok(hasText(html, t("welcome.translatedPercent", { percent: 60 })));
    assert.match(html, /bg-accent\/60/);
    const different = [{ ...recentProjects[0], database_path: "/other.locust.db" }, recentProjects[0]];
    const isolated = renderWelcome("recent", different, formats, true);
    assert.equal((isolated.match(/role="progressbar"/g) ?? []).length, 1);
  });

  test(`${locale}: supported engine chips are stable-first and collapse after eight`, () => {
    setLocale(locale);
    const engines = [...formats, ...Array.from({ length: 7 }, (_, i) => ({ ...formats[0], id: `extra-${i}`, name: `Extra ${i}` }))];
    const html = renderWelcome("ready", [], engines);
    const strip = html.slice(html.indexOf('aria-labelledby="supported-engines-title"'));
    assert.match(strip, /border-accent/);
    assert.ok(strip.indexOf("RPG Maker MV/MZ") < strip.indexOf("Godot / Dialogic"));
    const details = strip.match(/<details[^>]*>([\s\S]*?)<\/details>/);
    assert.ok(details);
    assert.ok(hasText(details[0], t("welcome.moreEngines", { count: 3 })));
    assert.equal((strip.slice(0, strip.indexOf("<details")).match(/data-engine=/g) ?? []).length, 8);
    assert.equal((details[0].match(/data-engine=/g) ?? []).length, 3);
    assert.ok(!strip.includes("Hidden engine"));
    assert.doesNotMatch(renderWelcome("ready"), /welcome.moreEngines/);
  });
}

test("recent row activation reuses saved-folder and database open flows", async () => {
  renderWelcome("recent");
  actions.opens = []; actions.databases = [];
  const rows = actions.buttons.filter(p => p["data-recent-project"]);
  assert.equal(rows.length, 2);
  await rows[0].onClick();
  assert.equal(actions.opens.length, 1);
  assert.equal(actions.opens[0][0], "/games/Demo");
  assert.equal(actions.opens[0][1], "rpgmaker-mv");
  assert.equal(actions.opens[0][3], true);
  await rows[1].onClick();
  assert.equal(actions.databases.length, 1);
  assert.deepEqual(actions.databases[0].slice(0, 3), ["/projects/Saved.locust.db", "/games/Saved", "unknown-format"]);
});
