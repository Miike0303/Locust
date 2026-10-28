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
  [{ UnencodableTranslation: { encoding: "Shift-JIS" } } as ValidationKind, "UnencodableTranslation", "validate.kind.unencodableLabel"],
  ["EmptyTranslation", "EmptyTranslation", "validate.kind.emptyLabel"],
  ["IdenticalToSource", "IdenticalToSource", "validate.kind.identicalLabel"],
  [{ InconsistentTranslation: { variants: 2, common: "Guardar" } }, "InconsistentTranslation", "validate.kind.inconsistentLabel"],
  ["StaleTranslation", "StaleTranslation", "validate.kind.staleLabel"],
  ["InvalidInjectionProvenance", "InvalidInjectionProvenance", "validate.kind.provenanceLabel"],
  [{ InvalidInjectionProvenance: { reason: "MissingTextAssetOriginal" } }, "InvalidInjectionProvenance", "validate.kind.provenanceLabel"],
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
      return { format: "module", source: `export const useModalA11y=()=>({}); export const MODAL_BACKDROP_CLASS='', MODAL_FOOTER_CLASS='', MODAL_HEADER_CLASS='', MODAL_BODY_CLASS=''; export const modalPanelClass=x=>x;`, shortCircuit: true };
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
  test(`translation consistency has a localized advisory label and hint (${locale})`, async () => {
    const { validationBadgeLabel, validationKindHint } = await import("./validationLabels.ts");
    const t = (key: any) => translate(catalog, locale, key);
    assert.equal(validationBadgeLabel("InconsistentTranslation", t), locale === "en"
      ? "Inconsistent translation" : "Traducción incoherente");
    assert.equal(validationKindHint("InconsistentTranslation", t), locale === "en"
      ? "Same source translated differently — review for consistency"
      : "Mismo texto original con traducciones distintas: revisa la coherencia");
    assert.equal(validationKindHint("FutureValidationKind", t), undefined);
  });

  test(`binary capacity warning reports validation without predicting skipped translations (${locale})`, () => {
    for (const count of [1, 3]) {
      const warning = translate(catalog, locale, "inject.toast.binarySlots", { count });
      assert.doesNotMatch(warning, /will be skipped|se omitirán/);
      assert.equal(warning, locale === "en"
        ? `${count} translation(s) exceed binary inject slots — resolve capacity issues before injection`
        : `${count} traducción(es) superan los huecos binarios de inyección — resuelva los problemas de capacidad antes de inyectar`);
    }
  });

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

const messageCases: [ValidationKind, string, string][] = [
  [{ MissingPlaceholder: { placeholder: "{name}" } }, "missing {name}", "falta {name}"],
  [{ ExtraPlaceholder: { placeholder: "%s" } }, "extra %s", "extra %s"],
  [{ ExceedsCharLimit: { actual: 12, limit: 0 } }, "12 chars > limit 0", "12 caracteres > límite 0"],
  [{ ExceedsBinarySlot: { actual: 16, limit: 8, encoding: "utf16le" } }, "16 > 8 bytes (utf16le)", "16 > 8 bytes (utf16le)"],
  [{ UnencodableTranslation: { encoding: "Shift-JIS" } },
    "The translation contains characters that cannot be encoded as Shift-JIS. Injection is refused; the original game and saved translation remain unchanged.",
    "La traducción contiene caracteres que no se pueden codificar en Shift-JIS. Se rechaza la inserción; el juego original y la traducción guardada permanecen intactos."],
  ["EmptyTranslation", "Translation is empty.", "La traducción está vacía."],
  ["IdenticalToSource", "Translation is identical to the source.", "La traducción es idéntica al original."],
  [{ InconsistentTranslation: { variants: 2, common: "Guardar" } },
    "The same source has 2 translation variants; most common translation: “Guardar”",
    "El mismo texto original tiene 2 traducciones distintas; la más común: «Guardar»"],
  ["StaleTranslation",
    "Translate this entry again or mark it reviewed/approved before injecting or pivoting it.",
    "Traduce esta entrada de nuevo o márcala como revisada/aprobada antes de insertarla o pivotarla."],
  [{ InvalidInjectionProvenance: { reason: { MalformedMetadata: { field: "locust_injection_source" } } } },
    "Injection metadata “locust_injection_source” is invalid. Extract this entry again before injection.",
    "Los metadatos de inserción «locust_injection_source» no son válidos. Extrae esta entrada de nuevo antes de insertarla."],
  [{ InvalidInjectionProvenance: { reason: { MissingMetadata: { field: "textasset_group_capacity" } } } },
    "Injection metadata “textasset_group_capacity” is missing. Extract this entry again before injection.",
    "Faltan los metadatos de inserción «textasset_group_capacity». Extrae esta entrada de nuevo antes de insertarla."],
  [{ InvalidInjectionProvenance: { reason: { InvalidCapacity: { encoding: "utf8", capacity: 0 } } } },
    "The recorded injection capacity (0 bytes, utf8) is invalid. Extract this entry again.",
    "La capacidad de inserción registrada (0 bytes, utf8) no es válida. Extrae esta entrada de nuevo."],
  [{ InvalidInjectionProvenance: { reason: { ConflictingEncodings: { slot_encoding: "utf8", capacity_encoding: "utf16le" } } } },
    "The binary slot uses utf8, but its recorded capacity uses utf16le. Extract this entry again.",
    "El hueco binario usa utf8, pero su capacidad registrada usa utf16le. Extrae esta entrada de nuevo."],
  [{ InvalidInjectionProvenance: { reason: { CapacitySourceMismatch: { encoding: "utf8", capacity: 100 } } } },
    "The recorded capacity (100 bytes, utf8) does not match the physical source. Extract this entry again.",
    "La capacidad registrada (100 bytes, utf8) no coincide con el original físico. Extrae esta entrada de nuevo."],
  [{ InvalidInjectionProvenance: { reason: { UnencodableSource: { encoding: "sjis" } } } },
    "The physical source cannot be encoded as sjis. Check its encoding and extract this entry again.",
    "El original físico no se puede codificar en sjis. Comprueba su codificación y extrae esta entrada de nuevo."],
  [{ InvalidInjectionProvenance: { reason: { UnsupportedTextAssetLayout: { layout: "future_layout" } } } },
    "The TextAsset layout “future_layout” is unsupported. Extract it with a supported layout before injection.",
    "El formato de TextAsset «future_layout» no es compatible. Extráelo con un formato compatible antes de insertarlo."],
  [{ InvalidInjectionProvenance: { reason: "ConflictingTextAssetMetadata" } },
    "This TextAsset has conflicting metadata. Extract the TextAsset again.",
    "Este TextAsset tiene metadatos contradictorios. Extrae el TextAsset de nuevo."],
  [{ InvalidInjectionProvenance: { reason: { TextAssetGroupTooLarge: { max_members: 16384, max_original_bytes: 1048576 } } } },
    "Automatic TextAsset reconstruction supports at most 16384 entries and 1048576 original bytes. Use a supported smaller group; this is a shared limit.",
    "La reconstrucción automática de TextAsset admite como máximo 16384 entradas y 1048576 bytes originales. Usa un grupo más pequeño compatible; este límite es compartido."],
  [{ InvalidInjectionProvenance: { reason: "MissingTextAssetOriginal" } },
    "The original shared TextAsset is missing. Reload its project database or extract it again.",
    "Falta el TextAsset original compartido. Vuelve a cargar la base de datos de su proyecto o extráelo de nuevo."],
  [{ InvalidInjectionProvenance: { reason: "TextAssetOriginalMismatch" } },
    "The original shared TextAsset does not match its recorded digest or length. Reload its project database or extract it again.",
    "El TextAsset original compartido no coincide con su resumen criptográfico o longitud registrados. Vuelve a cargar la base de datos de su proyecto o extráelo de nuevo."],
  [{ InvalidInjectionProvenance: { reason: { TextAssetOriginalExceedsCapacity: { actual: 25, capacity: 20 } } } },
    "The original TextAsset uses 25 bytes, exceeding its recorded capacity of 20 bytes. Extract it again.",
    "El TextAsset original ocupa 25 bytes y supera su capacidad registrada de 20 bytes. Extráelo de nuevo."],
  [{ InvalidInjectionProvenance: { reason: { InvalidTextAssetOverhead: { overhead: 30, original_bytes: 25 } } } },
    "The recorded TextAsset overhead (30 bytes) exceeds the original blob (25 bytes). Extract it again.",
    "La sobrecarga registrada del TextAsset (30 bytes) supera el bloque original (25 bytes). Extráelo de nuevo."],
  [{ InvalidInjectionProvenance: { reason: { UnsupportedTextAssetReconstruction: { layout: "loc_line" } } } },
    "The translation cannot be reconstructed into the TextAsset layout “loc_line”. Remove unsupported separators or line breaks.",
    "La traducción no se puede reconstruir en el formato de TextAsset «loc_line». Elimina los separadores o saltos de línea incompatibles."],
];

for (const [locale, catalog, expectedIndex] of [["en", en, 1], ["es", es, 2]] as const) {
  test(`live and stored validation messages use structured data without mutating issues (${locale})`, async () => {
    const { validationMessage } = await import("./validationLabels.ts");
    const t = (key: any, vars?: any) => translate(catalog, locale, key, vars);
    for (const messageCase of messageCases) {
      const [kind] = messageCase;
      const live = { kind, message: "backend diagnostic kept unchanged" };
      const stored = { ...live, kind: validationKindLabel(kind), kind_data: kind };
      for (const issue of [live, stored]) {
        const before = JSON.stringify(issue);
        assert.equal(validationMessage(issue, t), messageCase[expectedIndex]);
        assert.equal(JSON.stringify(issue), before);
      }
    }
    // Unit kinds need no payload, even with an older list API.
    assert.equal(validationMessage({ kind: "EmptyTranslation", message: "legacy empty" }, t), messageCases[5][expectedIndex]);
  });

  test(`validation result rows share all structured message translations (${locale})`, () => {
    (globalThis as any).__badgeTranslate = (key: string, vars?: any) => translate(catalog, locale, key, vars);
    const tree = ValidationResultsModal({
      open: true, onClose() {}, onSelectEntry() {},
      result: { fonts: [], validation: {
        total_checked: messageCases.length, issues_found: messageCases.length, entries_with_issues: messageCases.length,
        by_kind: {}, issues: messageCases.map(([kind], i) => ({ entry_id: `row-${i}`, kind, message: "raw English diagnostic" })),
      } },
    });
    const text = textOf(tree);
    for (const messageCase of messageCases) assert.ok(text.includes(messageCase[expectedIndex]), text);
    assert.ok(!text.includes("raw English diagnostic"));
  });

  test(`legacy, incomplete and unknown validation payloads retain raw messages (${locale})`, async () => {
    const { validationMessage } = await import("./validationLabels.ts");
    const t = (key: any, vars?: any) => translate(catalog, locale, key, vars);
    const message = "translation is empty";
    const kinds: unknown[] = [
      "InconsistentTranslation", { InconsistentTranslation: { variants: 2 } },
      { InconsistentTranslation: { variants: 2, common: null } },
      { InconsistentTranslation: { common: "Guardar" } },
      "MissingPlaceholder", { MissingPlaceholder: {} }, { ExtraPlaceholder: { placeholder: null } },
      { ExceedsCharLimit: { actual: 12 } }, { ExceedsCharLimit: { actual: -1, limit: 2 } },
      { ExceedsBinarySlot: { actual: 12, limit: 8 } }, { UnencodableTranslation: {} },
      "InvalidInjectionProvenance", "FutureKind", { FutureKind: { n: 2 } }, "toString", "__proto__",
      { InvalidInjectionProvenance: {} }, { InvalidInjectionProvenance: { reason: null } },
      ...[
        "FutureReason", "toString", "__proto__", { FutureReason: { detail: "new" } },
        "MalformedMetadata", { MalformedMetadata: {} }, { MalformedMetadata: { field: 42 } },
        { InvalidCapacity: { encoding: "utf8", capacity: -1 } },
        { InvalidCapacity: { encoding: "utf8", capacity: 1.5 } },
        { InvalidCapacity: { encoding: "utf8", capacity: Number.MAX_SAFE_INTEGER + 1 } },
        { ConflictingEncodings: { slot_encoding: "utf8" } },
        { TextAssetGroupTooLarge: { max_members: 2 } },
        { ConflictingTextAssetMetadata: {} },
        { MalformedMetadata: { field: "a" }, MissingMetadata: { field: "b" } },
        {}, [], 42, true, { UnencodableSource: null }, { UnencodableSource: [] },
      ].map(reason => ({ InvalidInjectionProvenance: { reason } })),
      null, {}, [], { MissingPlaceholder: null },
      { MissingPlaceholder: { placeholder: "%s" }, ExtraPlaceholder: { placeholder: "%s" } },
    ];
    for (const kind of kinds) {
      assert.equal(validationMessage({ kind: kind as ValidationKind, message }, t), message);
      if (kind && typeof kind === "object" && !Array.isArray(kind) && Object.keys(kind).length === 1) {
        assert.equal(validationMessage({ kind: Object.keys(kind)[0], kind_data: kind as ValidationKind, message }, t), message);
      }
    }
    assert.equal(validationMessage({ kind: "FutureKind", kind_data: { MissingPlaceholder: { placeholder: "%s" } }, message }, t), message);
    assert.equal(validationMessage({ kind: "FutureKind", kind_data: { InvalidInjectionProvenance: { reason: "MissingTextAssetOriginal" } }, message }, t), message);
    const common = '«Hola» "viajero"\n<player> {n} \\ ruta';
    assert.equal(validationMessage({ kind: { InconsistentTranslation: { variants: 3, common } }, message }, t),
      locale === "en" ? `The same source has 3 translation variants; most common translation: “${common}”`
        : `El mismo texto original tiene 3 traducciones distintas; la más común: «${common}»`);
    assert.equal(validationMessage({ kind: { InvalidInjectionProvenance: { reason: { MalformedMetadata: { field: common } } } }, message }, t),
      locale === "en" ? `Injection metadata “${common}” is invalid. Extract this entry again before injection.`
        : `Los metadatos de inserción «${common}» no son válidos. Extrae esta entrada de nuevo antes de insertarla.`);
  });
}
