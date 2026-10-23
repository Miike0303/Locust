import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import AppearanceSection from "../pages/settings/AppearanceSection.tsx";
import ProvidersSection from "../pages/settings/ProvidersSection.tsx";
import { setLocale } from "./i18n/index.ts";
import { en } from "./i18n/en.ts";
import { es } from "./i18n/es.ts";

const source = (name: string) => readFileSync(new URL(`../pages/settings/${name}.tsx`, import.meta.url), "utf8");
const providers = [
  { id: "google", name: "Google", is_free: true, requires_api_key: false, configured: true },
  { id: "openai", name: "OpenAI", is_free: false, requires_api_key: true, configured: false },
  { id: "grok-sub", name: "Grok subscription", is_free: false, requires_api_key: false, configured: false },
];
function render(section: typeof AppearanceSection, default_provider: string | null | undefined, loaded = true) {
  const client = new QueryClient({ defaultOptions: { queries: { gcTime: Infinity, staleTime: Infinity } } });
  client.setQueryData(["providers"], providers);
  if (loaded) client.setQueryData(["config"], { providers: {}, default_provider,
    ui: { theme: "system", font_size: 14, show_source_column: true, table_row_height: 36 } });
  try { return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(section))); }
  finally { client.clear(); }
}

for (const [locale, catalog, label] of [["en", en, "Default"], ["es", es, "Predeterminado"]] as const) {
  test(`${locale}: interface language is immediately after Appearance and before Theme`, () => {
    setLocale(locale);
    const html = render(AppearanceSection, null);
    assert.match(html, /<h2[^>]*>[^<]+<\/h2><div><label[^>]*for="ui-language"/);
    assert.ok(html.indexOf('id="ui-language"') < html.indexOf(catalog["settings.appearance.theme"]));
    assert.ok(html.includes(catalog["settings.appearance.showSourceColumnHint"]));
  });
  for (const id of ["google", "openai", "grok-sub", "unknown", "", null, undefined]) {
    test(`${locale}: configured default ${String(id)} is labeled without implying readiness`, () => {
      setLocale(locale);
      const html = render(ProvidersSection, id);
      const known = providers.some(p => p.id === id);
      assert.equal(html.split(label).length - 1, known ? 1 : 0);
      if (id === "grok-sub") assert.ok(html.includes(`${label}: Grok subscription`));
      if (id === "google" || id === "openai") {
        const name = providers.find(p => p.id === id)!.name;
        assert.match(html, new RegExp(`<h3[^>]*>${name}</h3><span[^>]*>${label}</span>`));
      }
      assert.ok(html.includes(catalog["settings.providers.needsApiKey"]));
    });
  }
  test(`${locale}: unloaded config does not invent a default`, () => {
    setLocale(locale);
    assert.ok(!render(ProvidersSection, undefined, false).includes(label));
  });
}

test("language selector retains its id, locale binding, options and persistence hook", () => {
  const appearance = source("AppearanceSection");
  assert.equal((appearance.match(/id="ui-language"/g) ?? []).length, 1);
  assert.match(appearance, /const \{ locale, setLocale \} = useLocale\(\)/);
  assert.match(appearance, /id="ui-language"\s+value=\{locale\}\s+onChange=\{\(e\) => setLocale\(e.target.value as Locale\)\}/);
  for (const locale of ["en", "es"]) assert.ok(appearance.includes(`<option value="${locale}">{t("settings.appearance.locale.${locale}")}</option>`));
});

test("existing appearance saves and provider health/save payloads remain identical across checkout line endings", () => {
  for (const [name, start, end, hash] of [
    ["AppearanceSection", "  const setTheme", "  if (configQuery.isError)", "6a4cd5e5119c4786be679d7eaada69f843bcd21e5addc6e6ae129fdf99b0cbd6"],
    ["ProvidersSection", "  const handleTest", "  const grokSub", "a7eae738ad55bf1671f121f29323ac65bfc802bfa5ef5f79ef2b361668159812"],
  ]) {
    for (const newline of ["\n", "\r\n"]) {
      const text = source(name).replace(/\r\n/g, "\n").replace(/\n/g, newline);
      const handler = text.slice(text.indexOf(start), text.indexOf(end)).replace(/\r\n/g, "\n");
      assert.equal(createHash("sha256").update(handler).digest("hex"), hash);
    }
  }
});
