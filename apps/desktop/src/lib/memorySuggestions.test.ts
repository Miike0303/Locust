import assert from "node:assert/strict";
import { test } from "node:test";
import { memoryLanguagePair, memoryOriginLabel, memoryScorePercent } from "./memorySuggestions";
import { setLocale, t } from "./i18n";

test("memory percent badges round and stay within 0–100", () => {
  assert.deepEqual([0, 0.6, 0.876, 1, -1, 2, NaN, Infinity].map(memoryScorePercent),
    ["0%", "60%", "88%", "100%", "0%", "100%", "0%", "0%"]);
});

for (const [locale, project, global] of [["en", "Project", "Global"], ["es", "Proyecto", "Global"]] as const) {
  test(`${locale}: memory origins are localized`, () => {
    setLocale(locale);
    assert.equal(memoryOriginLabel("project", t), project);
    assert.equal(memoryOriginLabel("global", t), global);
    setLocale("en");
  });
}

test("memory pair follows translation defaults, supports auto source keys and requires a valid target", () => {
  assert.equal(memoryLanguagePair(undefined, {}, [], "en"), "auto-es");
  assert.equal(memoryLanguagePair(undefined, { source: "en", target: "auto" }, [], "en"), null);
  assert.equal(memoryLanguagePair({ default_source_lang: "en", default_target_lang: "es" }, {}, [], "en"), "en-es");
  assert.equal(memoryLanguagePair({ default_source_lang: "en", default_target_lang: "es" }, { source: "ja", target: "fr" }, [], "en"), "ja-fr");
  assert.equal(memoryLanguagePair(undefined, { source: "en", target: "en" }, [], "en"), null);
  assert.equal(memoryLanguagePair(undefined, { source: "ja", target: "en" }, ["This English source contains enough letters to detect."], "en"), "en-es");
});
