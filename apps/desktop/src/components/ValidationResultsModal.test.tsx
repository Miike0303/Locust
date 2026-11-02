import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import { en } from "../lib/i18n/en.ts";
import { es } from "../lib/i18n/es.ts";
import { translate } from "../lib/i18n/index.ts";
import type { ValidationKind, ValidationResponse } from "../lib/api.ts";
import { QA_PAGE_SIZE } from "../lib/qaDashboard.ts";

const h = {
  states: [] as any[], cursor: 0, effects: [] as (() => void)[], changed: false,
  result: null as ValidationResponse | null, open: true,
  selected: [] as string[], reviewed: [] as string[][],
  state(initial: any) {
    const index = this.cursor++;
    if (!(index in this.states)) this.states[index] = typeof initial === "function" ? initial() : initial;
    return [this.states[index], (value: any) => {
      const next = typeof value === "function" ? value(this.states[index]) : value;
      if (!Object.is(next, this.states[index])) this.changed = true;
      this.states[index] = next;
    }];
  },
  effect(fn: () => void, deps: unknown[]) {
    const index = this.cursor++;
    const previous = this.states[index] as unknown[] | undefined;
    if (!previous || deps.some((value, i) => !Object.is(value, previous[i]))) {
      this.states[index] = deps;
      this.effects.push(fn);
    }
  },
  memo(fn: () => unknown, deps: unknown[]) {
    const index = this.cursor++;
    const previous = this.states[index];
    if (!previous || deps.some((value, i) => !Object.is(value, previous.deps[i]))) {
      this.states[index] = { deps, value: fn() };
    }
    return this.states[index].value;
  },
};
(globalThis as any).__validationModal = h;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
(globalThis as any).window = {};
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/ValidationResultsModal.tsx")) {
      if (specifier === "../lib/i18n") return { url: "test:encoding-modal-i18n", shortCircuit: true };
      if (specifier === "react") return {
        url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__validationModal;export const useState=initial=>h.state(initial);export const useEffect=(fn,deps)=>h.effect(fn,deps);export const useMemo=(fn,deps)=>h.memo(fn,deps);")}`,
        shortCircuit: true,
      };
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
beforeEach(() => {
  h.states = []; h.cursor = 0; h.effects = []; h.changed = false;
  h.result = null; h.open = true; h.selected = []; h.reviewed = [];
  (globalThis as any).__encodingTranslate = (key: string, vars?: any) => translate(en, "en", key, vars);
});
after(() => {
  hooks.deregister();
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__validationModal");
  Reflect.deleteProperty(globalThis, "__encodingTranslate");
});

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

for (const [locale, catalog] of [["en", en], ["es", es]] as const) {
  test(`issue rows localize empty and identical messages while retaining provenance (${locale})`, () => {
    (globalThis as any).__encodingTranslate = (key: string, vars?: any) => translate(catalog, locale, key, vars);
    const messages = ["translation is empty", "translation is identical to source", "invalid provenance: game/script.rpy", "unknown future diagnostic"];
    const kinds: ValidationKind[] = ["EmptyTranslation", "IdenticalToSource", "InvalidInjectionProvenance", "InvalidInjectionProvenance"];
    const tree = ValidationResultsModal({
      open: true, onClose() {}, onSelectEntry() {},
      result: { fonts: [], validation: {
        total_checked: 4, issues_found: 4, entries_with_issues: 4, by_kind: {},
        issues: messages.map((message, i) => ({ entry_id: `row-${i}`, kind: kinds[i], message })),
      } },
    });
    const text = textOf(tree);
    for (const expected of locale === "en" ? ["Translation is empty.", "Translation is identical to the source."] : ["La traducción está vacía.", "La traducción es idéntica al original."]) assert.ok(text.includes(expected), text);
    for (const message of messages.slice(2)) assert.ok(text.includes(message), text);
  });
}

function makeResult(count: number): ValidationResponse {
  return { fonts: [], validation: {
    total_checked: count, issues_found: count, entries_with_issues: count,
    by_kind: { EmptyTranslation: count },
    issues: Array.from({ length: count }, (_, i) => ({
      entry_id: `entry-${i}`, kind: "EmptyTranslation", message: "translation is empty",
    })),
  } };
}
function render(): any {
  for (let i = 0; i < 4; i++) {
    h.cursor = 0; h.changed = false;
    const tree = ValidationResultsModal({
      open: h.open, result: h.result, onClose() {},
      onSelectEntry(id) { h.selected.push(id); },
      onReviewInEditor(ids) { h.reviewed.push(ids); },
    });
    for (const effect of h.effects.splice(0)) effect();
    if (!h.changed) return tree;
  }
  throw new Error("Hook harness did not settle");
}
function button(tree: any, key: "common.previous" | "common.next" | "validate.reviewInEditor") {
  const label = (globalThis as any).__encodingTranslate(key);
  const found = nodes(tree).find(node => node.type === "button" && textOf(node) === label);
  assert.ok(found, `missing ${key} control`);
  return found;
}
function issueRows(tree: any): any[] {
  return nodes(tree).filter(node => node.type === "li");
}
function rowId(row: any): string {
  const id = nodes(row).find(node => node.props?.className === "font-mono text-caption text-text-muted truncate");
  assert.ok(id, "issue row must identify its entry");
  return textOf(id);
}

test("50,000 issues render at most QA_PAGE_SIZE issue rows", context => {
  h.result = makeResult(50_000);
  const timings: number[] = [];
  let tree: any;
  for (let i = 0; i < 3; i++) {
    h.states = []; h.effects = [];
    const start = performance.now();
    tree = render();
    timings.push(performance.now() - start);
  }
  timings.sort((a, b) => a - b);
  const rows = issueRows(tree);
  context.diagnostic(`50,000-issue initial element render median (3 runs): ${timings[1].toFixed(2)} ms; issue rows: ${rows.length}`);
  assert.equal(rows.length, QA_PAGE_SIZE);
  assert.equal(rowId(rows[0]), "entry-0");
  assert.equal(rowId(rows[rows.length - 1]), "entry-49");
  button(tree, "validate.reviewInEditor").props.onClick();
  assert.deepEqual(h.reviewed, [h.result.validation.issues.map(issue => issue.entry_id)]);
});

test("every one of 50,000 issues is reachable through bounded pages in both directions", () => {
  h.result = makeResult(50_000);
  let tree = render();
  assert.equal(button(tree, "common.previous").props.disabled, true);
  for (let page = 0; page < 1000; page++) {
    const rows = issueRows(tree);
    assert.equal(rows.length, QA_PAGE_SIZE);
    assert.deepEqual(rows.map(rowId), h.result.validation.issues.slice(page * QA_PAGE_SIZE, (page + 1) * QA_PAGE_SIZE).map(issue => issue.entry_id));
    assert.ok(textOf(tree).includes(`${page + 1} / 1000`));
    const next = button(tree, "common.next");
    assert.equal(next.props.disabled, page === 999);
    if (page < 999) { next.props.onClick(); tree = render(); }
  }
  for (let page = 998; page >= 0; page--) {
    const previous = button(tree, "common.previous");
    assert.equal(previous.props.disabled, false);
    previous.props.onClick(); tree = render();
    assert.equal(rowId(issueRows(tree)[0]), `entry-${page * QA_PAGE_SIZE}`);
  }
  assert.equal(button(tree, "common.previous").props.disabled, true);
});

test("partial last page stays reachable and clicking page 2 selects the right entry", () => {
  h.result = makeResult(53);
  button(render(), "common.next").props.onClick();
  const tree = render();
  const rows = issueRows(tree);
  assert.deepEqual(rows.map(rowId), ["entry-50", "entry-51", "entry-52"]);
  assert.equal(button(tree, "common.next").props.disabled, true);
  const rowButton = nodes(rows[1]).find(node => node.type === "button");
  rowButton.props.onClick();
  assert.deepEqual(h.selected, ["entry-51"]);
});

test("paging retains full summary, font problems, and all unique editor worklist IDs", () => {
  h.result = makeResult(103);
  h.result.validation.issues[51].entry_id = "entry-0";
  h.result.validation.entries_with_issues = 102;
  h.result.fonts = [{ font_path: "font.ttf", font_name: "Test font", total_unique_chars: 1,
    missing_chars: ["ñ"], missing_count: 1, coverage_percent: 0, has_full_coverage: false }];
  h.result.font_issues = [{ font_path: "broken.ttf", message: "Unreadable font" }];
  button(render(), "common.next").props.onClick();
  const tree = render();
  const text = textOf(tree);
  assert.ok(text.includes(`${translate(en, "en", "validate.kind.emptyLabel")}: 103`), text);
  assert.ok(text.includes(translate(en, "en", "validate.fontProblems", { count: 2 })), text);
  const fontSection = nodes(tree).find(node => typeof node.type === "function" && node.type.name === "FontSection");
  assert.ok(fontSection, "full font details stay available on later pages");
  assert.equal(fontSection.props.fonts, h.result.fonts);
  assert.equal(fontSection.props.issues, h.result.font_issues);
  button(tree, "validate.reviewInEditor").props.onClick();
  const expected = h.result.validation.issues.map(issue => issue.entry_id).filter((id, index, ids) => ids.indexOf(id) === index);
  assert.deepEqual(h.reviewed, [expected]);
  assert.equal(h.reviewed[0].length, 102);
  assert.equal(h.reviewed[0][101], "entry-102");
});

for (const change of ["new result", "reopen"]) {
  test(`pagination resets to page 1 on ${change}`, () => {
    h.result = makeResult(123);
    button(render(), "common.next").props.onClick();
    assert.equal(rowId(issueRows(render())[0]), "entry-50");
    if (change === "new result") h.result = makeResult(101);
    else {
      h.open = false;
      assert.equal(render(), null);
      h.open = true;
    }
    const tree = render();
    assert.equal(rowId(issueRows(tree)[0]), "entry-0");
    assert.equal(button(tree, "common.previous").props.disabled, true);
  });
}

for (const [locale, catalog] of [["en", en], ["es", es]] as const) {
  test(`pagination uses existing localized controls (${locale})`, () => {
    (globalThis as any).__encodingTranslate = (key: string, vars?: any) => translate(catalog, locale, key, vars);
    h.result = makeResult(51);
    const tree = render();
    assert.equal(textOf(button(tree, "common.previous")), catalog["common.previous"]);
    assert.equal(textOf(button(tree, "common.next")), catalog["common.next"]);
    assert.ok(textOf(tree).includes("1 / 2"));
  });
}

test("empty and single-page results keep review availability and need no pagination", () => {
  for (const count of [0, 1, QA_PAGE_SIZE]) {
    h.result = makeResult(count);
    const tree = render();
    assert.equal(issueRows(tree).length, count);
    assert.equal(button(tree, "validate.reviewInEditor").props.disabled, count === 0);
    assert.ok(!nodes(tree).some(node => node.type === "button" &&
      (textOf(node) === en["common.previous"] || textOf(node) === en["common.next"])));
  }
});
