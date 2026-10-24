import assert from "node:assert/strict";
import { test } from "node:test";
import { registerHooks } from "node:module";
import { en } from "../lib/i18n/en.ts";
import { es } from "../lib/i18n/es.ts";
import { translate } from "../lib/i18n/index.ts";
import type { ValidationKind } from "../lib/api.ts";

(globalThis as any).window = {};
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/ValidationResultsModal.tsx") && specifier === "../lib/i18n") {
      return { url: "test:encoding-modal-i18n", shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url === "test:encoding-modal-i18n") {
      return { format: "module", source: "export const useT=()=>globalThis.__encodingTranslate;", shortCircuit: true };
    }
    if (url.endsWith("/lib/modalA11y.ts")) {
      return { format: "module", source: "export const useModalA11y=()=>({}); export const MODAL_BACKDROP_CLASS='', MODAL_FOOTER_CLASS='', MODAL_HEADER_CLASS='shrink-0', MODAL_BODY_CLASS='flex-1 min-h-0 overflow-y-auto'; export const modalPanelClass=x=>x;", shortCircuit: true };
    }
    return nextLoad(url, context);
  },
});
const { default: ValidationResultsModal } = await import("./ValidationResultsModal.tsx");

function nodes(node: any): any[] {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (node == null || typeof node !== "object") return [];
  return [node, ...nodes(node.props?.children)];
}
function textOf(node: any): string {
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (node == null || typeof node === "boolean") return "";
  return typeof node === "object" ? textOf(node.props?.children) : String(node);
}

for (const [locale, catalog, expected] of [
  ["en", en, "The translation contains characters that cannot be encoded as Shift-JIS. Injection is refused; the original game and saved translation remain unchanged."],
  ["es", es, "La traducción contiene caracteres que no se pueden codificar en Shift-JIS. Se rechaza la inserción; el juego original y la traducción guardada permanecen intactos."],
] as const) {
  test(`encoding refusal renders clear localized details and no opaque excerpt (${locale})`, () => {
    (globalThis as any).__encodingTranslate = (key: string, vars?: any) => translate(catalog, locale, key, vars);
    let selected = "";
    const tree = ValidationResultsModal({
      open: true, onClose() {}, onSelectEntry(id) { selected = id; },
      result: {
        fonts: [], validation: {
          total_checked: 1, issues_found: 1, entries_with_issues: 1,
          by_kind: { UnencodableTranslation: 1 },
          issues: [{
            entry_id: "synthetic-row",
            kind: { UnencodableTranslation: { encoding: "Shift-JIS" } } as ValidationKind,
            message: "opaque backend excerpt",
            source: "opaque source excerpt",
          }],
        },
      },
    });
    const text = textOf(tree);
    assert.ok(text.includes(expected), text);
    assert.ok(!text.includes("opaque"), text);
    assert.ok(!text.includes("UnencodableTranslation"), text);
    const badge = nodes(tree).find(node => node.type === "span" &&
      textOf(node) === (catalog as Record<string, string>)["validate.kind.unencodableLabel"]);
    assert.ok(badge?.props.className.includes("text-danger"));
    const rowButton = nodes(tree).find(node => node.type === "button" && textOf(node).includes("synthetic-row"));
    rowButton.props.onClick();
    assert.equal(selected, "synthetic-row");
  });
}
