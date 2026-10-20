import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { setLocale, t } from "../lib/i18n/index.ts";

// Keep page components, stores, React and queries real. Only replace native IPC.
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      const source = specifier === "./runtime" ? "export const IS_TAURI=true;"
        : specifier === "@tauri-apps/api/core" ? "export const invoke=async()=>7842;" : null;
      if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    }
    return nextResolve(specifier, context);
  },
});
const pages = await Promise.all(["Dashboard", "Editor", "Review", "Qa", "Speakers", "TranslationMemory"].map(async name => ({
  name, Page: (await import(`./${name}.tsx`)).default,
})));
after(() => { hooks.deregister(); setLocale("en"); });

for (const locale of ["en", "es"] as const) {
  for (const { name, Page } of pages) {
    test(`${locale}: ${name} no-project has one adjacent Home link and plain guidance`, () => {
      setLocale(locale);
      const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
      try {
        const html = renderToStaticMarkup(createElement(QueryClientProvider, { client },
          createElement(MemoryRouter, {}, createElement(Page))));
        const label = locale === "en" ? "Go to Home" : "Ir al inicio";
        // Anchors with href have the implicit link role; match its exact accessible name.
        const links = [...html.matchAll(/<a\b([^>]*)>([^<]*)<\/a>/g)].filter(match => match[2] === label);
        assert.equal(links.length, 1, `exactly one link named ${label}`);
        assert.match(links[0][1], /href="\/"/);
        assert.equal([...html.matchAll(/href="\/"/g)].length, 1, "no duplicate Home action");
        assert.match(html, new RegExp(`<p[^>]*>[^<]+<\\/p>${links[0][0].replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}<\\/div>`), "action belongs to the message group");
        const titleKey = name === "Dashboard" ? "dashboard.noProject.title"
          : name === "Qa" ? "qa.noProject.title"
          : name === "TranslationMemory" ? "memory.noProject.title" : "editor.empty.title";
        assert.ok(html.includes(t(titleKey)));
        assert.doesNotMatch(html, /Welcome|Bienvenida/);
        if (name === "Review") assert.match(html, new RegExp(`<h1[^>]*>${t("nav.review")}<\\/h1>`));
        if (name === "Speakers") {
          assert.ok(!html.includes(t("speakers.contextHint")));
          assert.ok(!html.includes(t("speakers.evidenceHint")));
          assert.doesNotMatch(html, /Project identity required/);
        }
        if (name === "Qa") {
          const run = html.match(/<button\b([^>]*)>Run validation<\/button>/);
          if (locale === "en") {
            assert.ok(run);
            assert.match(run[1], /disabled=""/);
            assert.match(run[1], /disabled:cursor-not-allowed/);
          }
        }
        assert.ok(client.getQueryCache().getAll().every(query => query.isDisabled()), "no project requests");
      } finally { client.clear(); }
    });
  }
}
