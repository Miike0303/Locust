/**
 * Lightweight asserts (run: npx --yes tsx src/lib/i18n/i18n.test.ts).
 */
import assert from "node:assert/strict";
import { en } from "./en.ts";
import { es } from "./es.ts";
import { setLocale, t, translate } from "./index.ts";

const enKeys = Object.keys(en).sort();
const esKeys = Object.keys(es).sort();
assert.deepEqual(esKeys, enKeys, "en and es must have identical key sets");
assert.ok(enKeys.length > 0, "catalog is not empty");

setLocale("en");
assert.equal(
  t("api.unreachable", { base: "http://localhost:7842/api" }),
  "Cannot reach Locust at http://localhost:7842/api. Make sure the app is running.",
);

assert.equal(
  t("settings.history.totalRuns", { count: 1 }),
  "Total (1 run)",
);
assert.equal(
  t("settings.history.totalRuns", { count: 2 }),
  "Total (2 runs)",
);

// Unknown keys still fall through at runtime via translate(); t() is MessageKey-typed.
assert.equal(translate(en, "en", "this.key.does.not.exist"), "this.key.does.not.exist");
assert.doesNotThrow(() => translate(en, "en", "also.missing", { count: 3, name: "x" }));

// Pure translator: interpolation + plural without mutating module locale
assert.equal(
  translate(
    { "hello.name": "Hello, {name}" },
    "en",
    "hello.name",
    { name: "Ada" },
  ),
  "Hello, Ada",
);
assert.equal(
  translate(
    {
      "item.count.one": "{count} item",
      "item.count.other": "{count} items",
    },
    "en",
    "item.count",
    { count: 1 },
  ),
  "1 item",
);
assert.equal(
  translate(
    {
      "item.count.one": "{count} item",
      "item.count.other": "{count} items",
    },
    "en",
    "item.count",
    { count: 5 },
  ),
  "5 items",
);

setLocale("es");
assert.equal(t("welcome.recentCount", { count: 1 }), "1 proyecto reciente");
assert.equal(t("welcome.recentCount", { count: 3 }), "3 proyectos recientes");
setLocale("en");
assert.equal(t("welcome.recentCount", { count: 1 }), "1 recent project");
assert.equal(t("welcome.recentCount", { count: 3 }), "3 recent projects");

console.log("i18n.test.ts: ok");
