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
  "Cannot reach Locust backend at http://localhost:7842/api — is locust server running?",
);

assert.equal(
  t("settings.history.totalRuns", { count: 1 }),
  "Total (1 run)",
);
assert.equal(
  t("settings.history.totalRuns", { count: 2 }),
  "Total (2 runs)",
);

assert.equal(t("this.key.does.not.exist"), "this.key.does.not.exist");
assert.doesNotThrow(() => t("also.missing", { count: 3, name: "x" }));

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

console.log("i18n.test.ts: ok");
