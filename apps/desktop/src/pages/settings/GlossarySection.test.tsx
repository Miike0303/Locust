import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient } from "@tanstack/react-query";
import type { GlossaryEntry, ProjectInfo } from "../../lib/api.ts";
import { setLocale, t, type TranslateKey } from "../../lib/i18n/index.ts";

const project: ProjectInfo = { database_path: "C:/demo.db", path: "C:/game", format_id: "renpy", name: "Demo" };
const original: GlossaryEntry = {
  term: "US", translation: "EE.UU.", lang_pair: "en-es", case_sensitive: true, context: "Country abbreviation",
};
const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
const label = (key: string) => t(key as TranslateKey); // The parent adds the new glossary catalog keys.
const h = {
  t, project: project as ProjectInfo | null,
  slots: [] as any[], cursor: 0, dirty: false, effects: [] as (() => void)[],
  calls: [] as GlossaryEntry[], toasts: [] as unknown[][], invalidations: [] as any[],
  fail: false, waitForSave: null as Promise<void> | null,
  state(initial: any) {
    const slots = h.slots, index = h.cursor++;
    if (!(index in slots)) slots[index] = typeof initial === "function" ? initial() : initial;
    return [slots[index], (value: any) => {
      const next = typeof value === "function" ? value(slots[index]) : value;
      if (!Object.is(next, slots[index])) h.dirty = true;
      slots[index] = next;
    }];
  },
  effect(callback: () => void, deps: unknown[]) {
    const index = h.cursor++, previous = h.slots[index];
    if (previous && deps.length === previous.length && deps.every((dep, i) => Object.is(dep, previous[i]))) return;
    h.slots[index] = deps;
    h.effects.push(callback);
  },
  query(options: any) {
    const state = client.getQueryState(options.queryKey);
    return { data: state?.data, isLoading: false, isError: false, error: null };
  },
  async save(entry: GlossaryEntry) {
    h.calls.push(entry);
    if (h.waitForSave) await h.waitForSave;
    if (h.fail) throw new Error("write failed");
  },
};
(globalThis as any).__glossaryEditTest = h;
// Follow the Settings/Speakers dependency-aware hook seam, retaining real query storage.
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/settings/GlossarySection.tsx")) {
      const stubs: Record<string, string> = {
        react: "export const useState=h.state;export const useEffect=h.effect;export const useMemo=fn=>fn();export const useRef=x=>h.state(()=>({current:x}))[0];",
        "@tanstack/react-query": "export const useQuery=h.query;export const useQueryClient=()=>({invalidateQueries:async options=>{h.invalidations.push(options);}});",
        "../../lib/api": "export const getConfig=()=>{};export const getGlossary=()=>{};export const addGlossaryEntry=h.save;export const deleteGlossaryEntry=()=>{throw Error('unexpected delete');};",
        "../../lib/i18n": "export const useT=()=>h.t;",
        "../../stores/projectStore": "export const useProjectStore=Object.assign(select=>select(h),{getState:()=>h});",
        "../../stores/toastStore": "export const addToast=(...args)=>h.toasts.push(args);",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__glossaryEditTest;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: GlossarySection } = await import("./GlossarySection.tsx");

function tree() {
  for (let attempt = 0; attempt < 20; attempt++) {
    h.cursor = 0; h.dirty = false;
    const result = GlossarySection();
    for (const effect of h.effects.splice(0)) effect();
    if (!h.dirty) return result;
  }
  throw Error("Glossary did not settle");
}
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function find(predicate: (node: any) => boolean, root = tree()) {
  const result = nodes(root).find(predicate);
  assert.ok(result, "missing glossary control");
  return result;
}
function row() { return find(node => node.type === "tr" && nodes(node).some(n => n.type === "td" && n.props.children === "US")); }
function control(key: string, root = tree()) {
  return find(node => ["input", "textarea"].includes(node.type) && node.props["aria-label"] === label(key), root);
}
function change(node: any, value: string | boolean) {
  node.props.onChange({ target: typeof value === "boolean" ? { checked: value } : { value } });
}
function action(key: string, root = tree()) {
  return find(node => node.type === "button" && (node.props["aria-label"] === label(key)
    || node.props.title === label(key) || node.props.children === label(key)), root);
}
function startEdit() { action("settings.glossary.editTitle", row()).props.onClick(); }
function translation(value: string) { change(control("settings.glossary.translation", row()), value); }
function note(value: string) { change(control("settings.glossary.note", row()), value); }
function caseSensitive(value: boolean) { change(control("settings.glossary.caseSensitive", row()), value); }
function save() { return action("settings.glossary.save", row()).props.onClick(); }

beforeEach(() => {
  client.clear(); h.project = project; h.slots = []; h.effects = []; h.calls = []; h.toasts = []; h.invalidations = [];
  h.fail = false; h.waitForSave = null; setLocale("en");
  client.setQueryData(["config"], { default_source_lang: "en", default_target_lang: "es" });
  client.setQueryData(["glossary", project.database_path, "en-es"], [{ ...original }]);
});
after(() => { hooks.deregister(); client.clear(); Reflect.deleteProperty(globalThis, "__glossaryEditTest"); setLocale("en"); });

test("creating US → EE.UU. sends case sensitivity and the optional note", async () => {
  change(find(node => node.props?.placeholder === t("settings.glossary.sourceTerm")), " US ");
  change(find(node => node.props?.placeholder === t("settings.glossary.preferredTranslation")), " EE.UU. ");
  change(control("settings.glossary.caseSensitive"), true);
  change(control("settings.glossary.note"), "Country abbreviation");
  await action("settings.glossary.addEntry").props.onClick();
  assert.deepEqual(h.calls, [original]);
  assert.equal(control("settings.glossary.caseSensitive").props.checked, false);
  assert.equal(control("settings.glossary.note").props.value, "");
});

test("creating an entry with an empty note sends null and defaults to case insensitive", async () => {
  change(find(node => node.props?.placeholder === t("settings.glossary.sourceTerm")), "US");
  change(find(node => node.props?.placeholder === t("settings.glossary.preferredTranslation")), "EE.UU.");
  change(control("settings.glossary.note"), "   ");
  await action("settings.glossary.addEntry").props.onClick();
  assert.deepEqual(h.calls, [{ ...original, context: null, case_sensitive: false }]);
});

test("the table shows case sensitivity and a truncated note with its full title", () => {
  const longNote = "Country abbreviation ".repeat(30) + "<literal>";
  client.setQueryData(["glossary", project.database_path, "en-es"], [{ ...original, context: longNote }]);
  const markup = renderToStaticMarkup(tree());
  assert.ok(markup.includes(label("settings.glossary.caseSensitive")));
  const indicator = nodes(row()).find(node => node.props.children === label("settings.glossary.caseSensitive"));
  assert.ok(indicator, "the case-sensitive entry needs an indicator in its row");
  const displayedNote = find(node => node.props.title === longNote, row());
  assert.equal(displayedNote.props.children, longNote);
  assert.match(displayedNote.props.className, /truncate/);
  assert.ok(markup.includes("&lt;literal&gt;"));
});

test("editing only the translation preserves case sensitivity, note and upsert identity", async () => {
  startEdit(); translation("Estados Unidos");
  await save();
  assert.deepEqual(h.calls, [{ ...original, translation: "Estados Unidos" }]);
  assert.deepEqual(h.invalidations, [{ queryKey: ["glossary", project.database_path], refetchType: "active" }]);
  assert.equal(nodes(row()).filter(node => ["input", "textarea"].includes(node.type)).length, 0);
});

for (const initial of [true, false]) {
  test(`editing toggles case sensitivity from ${initial} to ${!initial}`, async () => {
    client.setQueryData(["glossary", project.database_path, "en-es"], [{ ...original, case_sensitive: initial }]);
    startEdit();
    assert.equal(control("settings.glossary.caseSensitive", row()).props.checked, initial);
    caseSensitive(!initial); await save();
    assert.deepEqual(h.calls, [{ ...original, case_sensitive: !initial }]);
  });
}

for (const value of ["Use for the country", "   "]) {
  test(`editing ${value.trim() ? "updates" : "clears"} the note through context`, async () => {
    startEdit(); note(value); await save();
    assert.deepEqual(h.calls, [{ ...original, context: value.trim() || null }]);
  });
}

test("cancel sends nothing, restores the row and discards all draft changes", () => {
  startEdit(); translation("Draft"); note("Draft note"); caseSensitive(false);
  action("common.cancel", row()).props.onClick();
  assert.deepEqual(h.calls, []);
  assert.deepEqual(h.invalidations, []);
  assert.ok(renderToStaticMarkup(tree()).includes("EE.UU."));
  startEdit();
  assert.equal(control("settings.glossary.translation", row()).props.value, original.translation);
  assert.equal(control("settings.glossary.note", row()).props.value, original.context);
  assert.equal(control("settings.glossary.caseSensitive", row()).props.checked, true);
});

test("term and language pair stay visible and are not editable in a row", async () => {
  startEdit();
  const controls = nodes(row()).filter(node => ["input", "textarea", "select"].includes(node.type));
  assert.equal(controls.length, 3);
  assert.deepEqual(controls.map(node => node.props["aria-label"]).sort(), [
    label("settings.glossary.translation"), label("settings.glossary.caseSensitive"), label("settings.glossary.note"),
  ].sort());
  const markup = renderToStaticMarkup(tree());
  assert.ok(markup.includes("US")); assert.ok(markup.includes("en-es"));
  translation("Updated"); await save();
  assert.equal(h.calls[0].term, "US"); assert.equal(h.calls[0].lang_pair, "en-es");
});

test("a save failure toasts the error and keeps all edits open for retry", async () => {
  startEdit(); translation("Retry me"); note("Keep this note"); caseSensitive(false); h.fail = true;
  await save();
  assert.equal(control("settings.glossary.translation", row()).props.value, "Retry me");
  assert.equal(control("settings.glossary.note", row()).props.value, "Keep this note");
  assert.equal(control("settings.glossary.caseSensitive", row()).props.checked, false);
  assert.deepEqual(h.invalidations, []);
  assert.deepEqual(h.toasts, [["error", t("settings.glossary.toast.saveFailed" as TranslateKey, { error: "write failed" })]]);
  h.fail = false; await save();
  assert.equal(h.calls.length, 2); assert.deepEqual(h.calls[1], h.calls[0]);
});

test("an empty edited translation is rejected without closing the edit", async () => {
  startEdit(); translation("   "); await save();
  assert.deepEqual(h.calls, []);
  assert.equal(control("settings.glossary.translation", row()).props.value, "   ");
  assert.deepEqual(h.toasts, [["error", t("settings.glossary.toast.termRequired")]]);
});

test("an in-flight edit disables its controls and actions until the save finishes", async () => {
  startEdit(); translation("Pending");
  let finish!: () => void;
  h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
  const pending = save();
  for (const node of nodes(row()).filter(node => ["input", "textarea", "button"].includes(node.type))) {
    assert.equal(node.props.disabled, true);
  }
  finish(); await pending;
  assert.equal(h.calls.length, 1);
});

test("a stale edit save refuses a changed project before rerender", async () => {
  startEdit(); translation("Old project draft");
  const staleSave = action("settings.glossary.save", row()).props.onClick;
  h.project = { ...project, database_path: "C:/other.db" };
  await staleSave();
  assert.deepEqual(h.calls, []);
  assert.deepEqual(h.toasts, [["error", t("settings.glossary.toast.projectChanged")]]);
});

test("switching language pairs discards a row draft instead of moving it to the new pair", () => {
  startEdit(); translation("Wrong pair draft");
  change(find(node => node.type === "input" && node.props.value === "en-es"), "en-fr");
  client.setQueryData(["glossary", project.database_path, "en-fr"], [{ ...original, lang_pair: "en-fr", translation: "États-Unis" }]);
  assert.equal(nodes(row()).filter(node => ["input", "textarea"].includes(node.type)).length, 0);
  change(find(node => node.type === "input" && node.props.value === "en-fr"), "en-es");
  startEdit();
  assert.equal(control("settings.glossary.translation", row()).props.value, "EE.UU.");
  assert.deepEqual(h.calls, []);
});
