import assert from "node:assert/strict";
import { test } from "node:test";
import { registerHooks } from "node:module";
import { en } from "./i18n/en.ts";
import { es } from "./i18n/es.ts";
import { translate } from "./i18n/index.ts";
import type { ValidationKind } from "./api.ts";

const cases = [
  [{ MissingPlaceholder: { placeholder: "{name}" } }, "MissingPlaceholder", "validate.kind.missingLabel"],
  [{ ExtraPlaceholder: { placeholder: "{name}" } }, "ExtraPlaceholder", "validate.kind.extraLabel"],
  [{ ExceedsCharLimit: { actual: 3, limit: 2 } }, "ExceedsCharLimit", "validate.kind.charLimitLabel"],
  [{ ExceedsBinarySlot: { actual: 3, limit: 2, encoding: "utf8" } }, "ExceedsBinarySlot", "validate.kind.binarySlotLabel"],
  ["EmptyTranslation", "EmptyTranslation", "validate.kind.emptyLabel"],
  ["IdenticalToSource", "IdenticalToSource", "validate.kind.identicalLabel"],
  ["StaleTranslation", "StaleTranslation", "validate.kind.staleLabel"],
  ["InvalidInjectionProvenance", "InvalidInjectionProvenance", "validate.kind.provenanceLabel"],
] satisfies [ValidationKind, string, string][];

(globalThis as any).window = {};
(globalThis as any).__badgeTranslate = (key: string, vars?: any) => translate(en, "en", key, vars);
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/ValidationResultsModal.tsx") && specifier === "../lib/i18n") {
      return { url: "test:badge-i18n", shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url === "test:badge-i18n") {
      return { format: "module", source: "export const useT=()=>globalThis.__badgeTranslate;", shortCircuit: true };
    }
    if (url.endsWith("/lib/modalA11y.ts")) {
      return { format: "module", source: `export const useModalA11y=()=>({}); export const MODAL_BACKDROP_CLASS='', MODAL_FOOTER_CLASS=''; export const modalPanelClass=x=>x;`, shortCircuit: true };
    }
    return nextLoad(url, context);
  },
});
const { validationKindLabel } = await import("./api.ts");
const { default: ValidationResultsModal } = await import("../components/ValidationResultsModal.tsx");

function textOf(node: any): string {
  if (node == null || typeof node === "boolean") return "";
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (typeof node !== "object") return String(node);
  return textOf(node.props?.children);
}
function spanTexts(node: any): string[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(spanTexts);
  return [...(node.type === "span" ? [textOf(node)] : []), ...spanTexts(node.props?.children)];
}

for (const [locale, catalog] of [["en", en], ["es", es]] as const) {
  test(`all known validation badge kinds use catalog labels (${locale})`, async () => {
    const { validationBadgeLabel } = await import("./validationLabels.ts");
    const t = (key: any) => translate(catalog, locale, key);
    for (const [kind, raw, key] of cases) {
      assert.equal(validationKindLabel(kind), raw);
      const expected = (catalog as Record<string, string>)[key];
      assert.equal(typeof expected, "string", `${locale}: missing ${key}`);
      assert.equal(validationBadgeLabel(raw, t), expected);
      assert.notEqual(validationBadgeLabel(raw, t), raw);
    }
    for (const future of ["FutureValidationKind", "Unknown", "toString", "__proto__"]) {
      assert.equal(validationBadgeLabel(future, t), future);
    }
  });

  test(`both issue and summary badges render localized labels (${locale})`, () => {
    (globalThis as any).__badgeTranslate = (key: string, vars?: any) => translate(catalog, locale, key, vars);
    const tree = ValidationResultsModal({
      open: true, onClose() {}, onSelectEntry() {},
      result: {
        validation: {
          total_checked: cases.length, issues_found: cases.length, entries_with_issues: cases.length,
          by_kind: Object.fromEntries([...cases.map(([, raw]) => [raw, 1]), ["FutureValidationKind", 1]]),
          issues: [...cases.map(([kind], i) => ({ entry_id: `entry-${i}`, kind, message: "detail" })),
            { entry_id: "future", kind: "FutureValidationKind" as ValidationKind, message: "future detail" }],
        }, fonts: [],
      },
    });
    const badges = spanTexts(tree);
    for (const [, raw, key] of cases) {
      const expected = (catalog as Record<string, string>)[key];
      assert.ok(!badges.includes(raw), `${locale}: issue badge exposes ${raw}`);
      assert.ok(!badges.includes(`${raw}: 1`), `${locale}: summary badge exposes ${raw}`);
      assert.ok(badges.includes(expected), `${locale}: missing issue badge ${key}`);
      assert.ok(badges.includes(`${expected}: 1`), `${locale}: missing summary badge ${key}`);
    }
    assert.ok(badges.includes("FutureValidationKind"));
    assert.ok(badges.includes("FutureValidationKind: 1"));
  });
}
