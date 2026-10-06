import assert from "node:assert/strict";
import { after, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { setLocale } from "./i18n/index.ts";
import PatchStatusIndicator from "../components/PatchStatusIndicator.tsx";
import type { PatchStatusResult } from "./api.ts";

function render(data: PatchStatusResult) {
  const client = new QueryClient({ defaultOptions: { queries: { gcTime: Infinity } } });
  client.setQueryData(["patchStatus", "game", 0], data);
  try {
    return renderToStaticMarkup(createElement(QueryClientProvider, { client },
      createElement(PatchStatusIndicator, { gamePath: "game", onOpenPatch() {} })));
  } finally { client.clear(); }
}

for (const [locale, copy] of [
  ["en", { original: "Original game", modified: "Game modified", direct: "Translated (direct)", add: "Language added", patched: "Patch applied", interrupted: "Patch interrupted", unknown: "Game state unknown", pending: "Injection unfinished" }],
  ["es", { original: "Juego original", modified: "Juego modificado", direct: "Traducido (directo)", add: "Idioma añadido", patched: "Parche aplicado", interrupted: "Parche interrumpido", unknown: "Estado del juego desconocido", pending: "Inyección sin terminar" }],
] as const) {
  for (const [status, label] of [["not_patched", copy.original], ["patched", copy.patched], ["interrupted", copy.interrupted], ["unknown", copy.unknown]] as const) {
    test(`badge ${status} in ${locale}`, () => {
      setLocale(locale);
      assert.ok(render({ status }).includes(`>${label}</button>`));
    });
  }
  for (const mode of ["direct", "add"] as const) {
    test(`badge ${mode} and language/time tooltip in ${locale}`, () => {
      setLocale(locale);
      const html = render({ status: "not_patched", injections: [{ transaction_id: "generation", mode, language: "es", applied_at: "2026-10-06T12:00:00Z", changed_files: 1 }] });
      assert.ok(html.includes(`>${copy[mode]}</button>`));
      assert.ok(!html.includes(copy.original));
      assert.match(html, /title="[^"]*es[^\"]*2026-10-06/);
    });
  }
  test(`badge legacy injection with unavailable metadata in ${locale}`, () => {
    setLocale(locale);
    const html = render({ status: "not_patched", injections: [{ transaction_id: "legacy", mode: null, language: null, applied_at: null, changed_files: 1 }] });
    assert.ok(html.includes(`>${copy.modified}</button>`));
    assert.ok(!html.includes(copy.original));
  });
  test(`badge coexistence and warning precedence in ${locale}`, () => {
    setLocale(locale);
    const injections: NonNullable<PatchStatusResult["injections"]> = [
      { transaction_id: "one", mode: "direct", language: "es", applied_at: null, changed_files: 1 },
      { transaction_id: "two", mode: "add", language: "fr", applied_at: null, changed_files: 2 },
    ];
    const both = render({ status: "patched", patch_id: "zip", patch_version: "2", language: "de", injections });
    assert.ok(both.includes(`>${copy.direct} · ${copy.add} · ${copy.patched}</button>`));
    assert.match(both, /title="[^"]*es[^"]*fr[^"]*zip@2[^"]*de/);
    for (const [status, label] of [["interrupted", copy.interrupted], ["unknown", copy.unknown]] as const) {
      assert.ok(render({ status, injections }).includes(`>${label}</button>`));
    }
    assert.ok(render({ status: "not_patched", injections, injection_pending: true }).includes(`>${copy.pending}</button>`));
  });
}
after(() => setLocale("en"));
