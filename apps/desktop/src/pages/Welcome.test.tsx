import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import Welcome from "./Welcome.tsx";
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

type Scenario = "first-run" | "recent" | "ready" | "loading";
function renderWelcome(scenario: Scenario): string {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } },
  });
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
        "nav.appName", "welcome.tagline", "welcome.openFolder", "welcome.openFile",
        "welcome.openDb", "welcome.applyPatch", "recovery.action", "welcome.chooseFormat",
      ];
      for (const key of controls) assert.ok(hasText(html, t(key)), `localized content: ${key}`);
      const showGuide = scenario !== "recent";
      const showHint = scenario === "first-run";
      assert.equal(hasText(html, t("welcome.guide.title")), showGuide);
      assert.equal(html.includes(`aria-label="${t("welcome.guide.aria")}"`), showGuide);
      if (showGuide) {
        for (const key of ["welcome.guide.open.label", "welcome.guide.translate.label", "welcome.guide.inject.label",
          "welcome.guide.open.description", "welcome.guide.translate.description", "welcome.guide.inject.description"] as const) {
          assert.ok(hasText(html, t(key)), `localized guide: ${key}`);
        }
      }
      assert.equal(hasText(html, t("welcome.providerHintLink")), showHint);
      assert.equal(html.includes(`aria-label="${t("welcome.providerHintDismiss")}"`), showHint);
      assert.equal(hasText(html, t("welcome.recent")), scenario === "recent");
      assert.equal(hasText(html, t("welcome.availableFormats")), scenario !== "loading");
      assert.ok(hasText(html, t("welcome.formatsAvailable", { count: scenario === "loading" ? 0 : 3 })));
      assert.ok(hasText(html, t("welcome.recentCount", { count: scenario === "recent" ? 2 : 0 })));
      if (scenario === "recent") {
        for (const text of ["Demo Game", "Saved Project", "/projects/Saved.locust.db", t("welcome.recentDbBadge"),
          t("welcome.recentGame", { path: "/games/Saved" }), t("welcome.addToQueue")]) {
          assert.ok(hasText(html, text), `retained recent project: ${text}`);
        }
      }
      if (scenario !== "loading") {
        for (const text of ["RPG Maker MV/MZ", "Unity", "Unknown engine", "Unknown format fallback",
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
