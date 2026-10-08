/**
 * Lightweight asserts (run: npx --yes tsx src/lib/i18n/i18n.test.ts).
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync, readdirSync } from "node:fs";
import ts from "typescript";
import { en } from "./en.ts";
import { es } from "./es.ts";
import { setLocale, t, translate } from "./index.ts";
import { formatDescriptionKey, KNOWN_FORMAT_IDS } from "../formatDescriptions.ts";
import { projectOpenMergeNotice } from "../projectOpenMerge.ts";

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

for (const value of [String.raw`C:\$&\game`, "a$$b", "$`", "$'"]) {
  test(`interpolation preserves replacement characters in ${JSON.stringify(value)}`, () => {
    assert.equal(
      translate({ message: "Before {path} after {path}." }, "en", "message", { path: value }),
      `Before ${value} after ${value}.`,
    );
  });
}

test("interpolation only substitutes placeholders in the original template", () => {
  for (const vars of [
    { path: String.raw`C:\{name}\game`, name: "Ada" },
    { name: "Ada", path: String.raw`C:\{name}\game` },
  ]) {
    assert.equal(
      translate({ message: "{name}: {path}; {missing}" }, "en", "message", vars),
      String.raw`Ada: C:\{name}\game; {missing}`,
    );
  }
});

test("interpolation only uses own, literal variable names", () => {
  assert.equal(
    translate({ message: "{a.b} {axb} {toString} {count}" }, "en", "message", { "a.b": "literal", count: 0 }),
    "literal {axb} {toString} 0",
  );
});
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

// Format cards: translated user-facing descriptions, no developer notes.
for (const id of KNOWN_FORMAT_IDS) {
  const key = formatDescriptionKey(id);
  assert.ok(key, `missing format.desc.${id}`);
  for (const locale of ["en", "es"] as const) {
    const text = translate(locale === "en" ? en : es, locale, key);
    assert.ok(text.length > 0 && text !== key, `${locale} ${id}`);
    assert.doesNotMatch(text, /synthetic|XOR|0x|FE FE/i, `${locale} ${id}: ${text}`);
  }
  assert.notEqual(es[key], en[key], `es ${id} is untranslated`);
}
assert.equal(formatDescriptionKey("some-future-format"), null);

// FilterBar inputs are w-32 (file) and w-24 (tag) at 11px; longer placeholders
// were clipped ("Cualquier archivc"). The "File"/"Tag" label sits beside them.
for (const catalog of [en, es]) {
  assert.ok(catalog["filter.anyFile"].length <= 14, catalog["filter.anyFile"]);
  assert.ok(catalog["filter.anyTag"].length <= 10, catalog["filter.anyTag"]);
}

setLocale("es");
assert.equal(t("welcome.recentCount", { count: 1 }), "1 proyecto reciente");
assert.equal(t("welcome.recentCount", { count: 3 }), "3 proyectos recientes");
setLocale("en");
assert.equal(t("welcome.recentCount", { count: 1 }), "1 recent project");
assert.equal(t("welcome.recentCount", { count: 3 }), "3 recent projects");

// Activity-log copy must be translated at the call site, including conditional
// branches. Parse TypeScript so multiline calls and commas in templates cannot
// hide an English message from the regression check. Diagnostics are excluded.
const untranslatedLogs: string[] = [];
const activityKeysUsed = new Set<string>();
let logCallCount = 0;
function hasLiteralMessage(node: ts.Expression): boolean {
  if (ts.isParenthesizedExpression(node)) return hasLiteralMessage(node.expression);
  if (ts.isConditionalExpression(node)) {
    return hasLiteralMessage(node.whenTrue) || hasLiteralMessage(node.whenFalse);
  }
  if (ts.isBinaryExpression(node)) {
    return hasLiteralMessage(node.left) || hasLiteralMessage(node.right);
  }
  return ts.isStringLiteralLike(node) || ts.isTemplateExpression(node);
}
function scanActivityLogs(directory: URL): void {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const url = new URL(entry.name + (entry.isDirectory() ? "/" : ""), directory);
    if (entry.isDirectory()) {
      scanActivityLogs(url);
      continue;
    }
    if (!/\.tsx?$/.test(entry.name) || /\.test\.tsx?$/.test(entry.name)) continue;
    const source = ts.createSourceFile(url.pathname, readFileSync(url, "utf8"), ts.ScriptTarget.Latest, true);
    function visit(node: ts.Node): void {
      if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)) {
        if (node.expression.text === "addLog") {
          logCallCount++;
          const message = node.arguments[1];
          if (message && hasLiteralMessage(message)) {
            const line = source.getLineAndCharacterOfPosition(message.getStart(source)).line + 1;
            untranslatedLogs.push(`${source.fileName.split("/src/")[1]}:${line}`);
          }
        }
        if (node.expression.text === "t") {
          function collectKeys(key: ts.Node): void {
            if (ts.isStringLiteralLike(key) && key.text.startsWith("activity.")) activityKeysUsed.add(key.text);
            ts.forEachChild(key, collectKeys);
          }
          if (node.arguments[0]) collectKeys(node.arguments[0]);
        }
      }
      ts.forEachChild(node, visit);
    }
    visit(source);
  }
}
scanActivityLogs(new URL("../../", import.meta.url));
assert.ok(logCallCount >= 67, "scan must cover all existing activity-log calls");
assert.deepEqual(untranslatedLogs, [], "addLog message arguments must use i18n (detail/source are diagnostics)");
for (const key of activityKeysUsed) {
  for (const catalog of [en, es]) {
    const keys = Object.keys(catalog);
    assert.ok(keys.includes(key) || (keys.includes(`${key}.one`) && keys.includes(`${key}.other`)), `missing activity key: ${key}`);
  }
}
const activityKeys = enKeys.filter(key => key.startsWith("activity."));
assert.ok(activityKeys.length > 0, "activity-log catalog entries must exist");
for (const key of activityKeys) {
  const messageKey = key as keyof typeof en;
  const placeholders = (value: string) => [...value.matchAll(/\{(\w+)\}/g)].map(match => match[1]).sort();
  assert.deepEqual(placeholders(es[messageKey]), placeholders(en[messageKey]), `placeholder parity: ${key}`);
  const stem = key.replace(/\.(one|other)$/, "");
  assert.ok(activityKeysUsed.has(stem) || activityKeysUsed.has(key), `unused activity key: ${key}`);
  if (en[messageKey].includes("{count}")) {
    assert.match(key, /\.(one|other)$/, `counted activity messages must use plural forms: ${key}`);
    assert.ok(enKeys.includes(`${stem}.one`) && enKeys.includes(`${stem}.other`), `incomplete plural pair: ${stem}`);
  }
  // Every new entry renders all its placeholders, including plural selection.
  for (const locale of ["en", "es"] as const) {
    const catalog = locale === "en" ? en : es;
    for (const count of [0, 1, 2]) {
      const vars = Object.fromEntries(placeholders(catalog[messageKey]).map(name => [name, name === "count" ? count : `sample-${name}`]));
      const rendered = translate(catalog, locale, stem, vars);
      assert.notEqual(rendered, stem, `${locale}: ${stem}`);
      assert.doesNotMatch(rendered, /\{\w+\}/, `${locale}: unresolved placeholder in ${stem}`);
    }
  }
}

setLocale("es");
assert.equal(t("activity.project.openFailed"), "No se pudo abrir el proyecto");
assert.equal(t("activity.queue.started", { count: 0 }), "Cola iniciada: 0 proyectos");
assert.equal(t("activity.queue.started", { count: 1 }), "Cola iniciada: 1 proyecto");
assert.equal(t("activity.queue.started", { count: 3 }), "Cola iniciada: 3 proyectos");
assert.equal(t("activity.queue.completedWithIssues", {
  name: "Juego", count: 1, issues: t("activity.count.validationIssues", { count: 2 }),
}), "Completado: Juego (1 cadena, 2 problemas de validación)");
assert.equal(t("activity.queue.completedWithIssues", {
  name: "Juego", count: 2, issues: t("activity.count.validationIssues", { count: 1 }),
}), "Completado: Juego (2 cadenas, 1 problema de validación)");
assert.equal(t("activity.import.completed", {
  format: "po",
  applied: t("activity.import.applied", { count: 1 }),
  skipped: t("activity.import.skipped", { count: 0 }),
  outdated: t("activity.import.outdated", { count: 2 }),
}), "Importación po: 1 traducción aplicada, 0 traducciones omitidas, 2 textos originales desactualizados");
assert.equal(t("activity.project.openedDb", { name: "Juego", count: 1, db: "C:\\juego\\saved.locust.db" }),
  "Se abrió la base de datos del proyecto Juego (1 cadena) desde C:\\juego\\saved.locust.db");
assert.equal(projectOpenMergeNotice({
  project_name: "Juego", format_name: "Ren'Py", total_strings: 1, added: 1,
  updated: 0, stale_source_reset: 1, removed: 2, preserved_translations: 1,
}, t).logMessage, "Se abrió Juego (Ren'Py, 1 cadena): 1 cadena nueva, 0 cadenas actualizadas, 1 traducción devuelta a pendiente, 2 cadenas eliminadas, 1 traducción conservada");
setLocale("en");
assert.equal(t("activity.project.openFailed"), "Failed to open project");
assert.equal(t("activity.queue.started", { count: 1 }), "Queue started: 1 project");
assert.equal(t("activity.queue.started", { count: 3 }), "Queue started: 3 projects");
console.log(`activity log i18n: ${logCallCount} calls checked; ${activityKeys.length} bilingual keys checked`);
console.log("i18n.test.ts: ok");

for (const [locale, catalog] of [["en", en], ["es", es]] as const) {
  test(`Rule95 publishing guidance identifies PowerShell in ${locale}`, () => {
    assert.match(catalog["patch.publish.commandHint"], /\bPowerShell\b/);
  });
}

test("Rule95 publishing strings have catalog and placeholder parity", () => {
  const keys = ["patch.publish.title", "patch.publish.rjCode", "patch.publish.noDetect", "patch.publish.gameVersion", "patch.publish.createEntry", "patch.publish.entryPath", "patch.publish.saveEntry", "patch.publish.entryFilter", "patch.publish.chooseEntry", "patch.publish.identity", "patch.publish.noCode", "patch.publish.commandHint", "patch.publish.copyFailed"] as const;
  for (const key of keys) {
    assert.ok(en[key], key);
    assert.ok(es[key], key);
    assert.deepEqual(es[key].match(/\{\w+\}/g)?.sort(), en[key].match(/\{\w+\}/g)?.sort(), key);
  }
});
