import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { gameFileExtensions } from "../lib/gameSource.ts";
import Welcome from "./Welcome.tsx";
import { readFileSync } from "node:fs";
import { useProjectStore } from "../stores/projectStore.ts";
import { setLocale, t, type TranslateKey } from "../lib/i18n/index.ts";
import type { AppConfig, PluginInfo, ProviderInfo } from "../lib/api.ts";

const originalFetch = globalThis.fetch;
before(() => {
  // Seed the query cache; no test may contact a backend.
  globalThis.fetch = async () => { throw new Error("Unexpected Welcome backend request"); };
});
after(() => {
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
function renderWelcome(scenario: Scenario): string {
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
        recent_projects: scenario === "recent" ? recentProjects : [],
      };
      client.setQueryData(["config"], config);
      client.setQueryData(["formats"], formats);
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
      assert.equal(hasText(html, t("welcome.allFormats", { stable: 2, experimental: 2 })), scenario !== "loading");
      assert.ok(hasText(html, t("welcome.formatsAvailable", { count: scenario === "loading" ? 0 : 4 })));
      assert.ok(hasText(html, t("welcome.recentCount", { count: scenario === "recent" ? 2 : 0 })));
      if (scenario === "recent") {
        for (const text of ["Demo Game", "Saved Project", "/projects/Saved.locust.db", t("welcome.recentDbBadge"),
          t("welcome.recentGame", { path: "/games/Saved" }), t("welcome.addToQueue")]) {
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
        assert.ok(/<span class="[^"]*bg-warning-muted text-warning/.test(html), "experimental chips use semantic status colors");
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
      assert.equal(details.length, 2);
      for (const disclosure of details) assert.doesNotMatch(disclosure[1], /\bopen\b/);
      const other = details.find(d => hasText(d[2], t("welcome.otherWays")));
      assert.ok(other, "native disclosure exposes the alternate entry points on expansion");
      for (const key of ["welcome.openDb", "welcome.applyPatch", "recovery.action", "welcome.chooseFormat"] as const) {
        assert.ok(hasText(other[2], t(key)), `reachable entry point ${key}`);
      }
      for (const key of ["welcome.openFolder", "welcome.openFile"] as const) assert.ok(hasText(html, t(key)));
      const grid = details.find(d => hasText(d[2], t("welcome.allFormats", { stable: 2, experimental: 2 })));
      assert.ok(grid);
      const badges = [...grid[2].matchAll(/<span class="[^"]*bg-warning-muted text-warning[^>]*>([^<]+)<\/span>/g)];
      assert.equal(badges.length, formats.filter(f => f.stability === "experimental").length);
      assert.ok(badges.every(b => b[1] === t("welcome.format.experimental")));
      assert.ok(hasText(html, t("welcome.stableFormats", { names: "RPG Maker MV/MZ, Unknown engine" })));
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
