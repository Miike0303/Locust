import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { setLocale } from "../lib/i18n/index.ts";

const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");

test("unknown routes keep the application layout and show recovery", () => {
  assert.match(app, /<Route path="\*" element=\{<NotFound \/>\} \/>/);
  const shell = app.slice(app.indexOf('<Route element={<Layout />}>'), app.indexOf('</Routes>'));
  assert.match(shell, /path="\*"/);
});

test("the five existing route destinations remain available", () => {
  for (const [path, component] of [["/", "Welcome"], ["/editor", "Editor"], ["/review", "Review"], ["/memory", "TranslationMemory"], ["/settings", "Settings"]]) {
    assert.ok(app.includes(`<Route path="${path}" element={<${component} />} />`));
  }
});

test("recovery has localized guidance and an ordinary Home link", async () => {
  const { default: NotFound } = await import("./NotFound.tsx");
  for (const [locale, title, action] of [["en", "Page not found", "Go to Home"], ["es", "Página no encontrada", "Ir al inicio"]] as const) {
    setLocale(locale);
    const html = renderToStaticMarkup(createElement(MemoryRouter, {}, createElement(NotFound)));
    assert.ok(html.includes(title));
    assert.ok(html.includes(action));
    assert.match(html, /href="\/"/);
    assert.match(html, /<h1/);
  }
  setLocale("en");
});
