import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient } from "@tanstack/react-query";
import type { GlossaryEntry, ProjectInfo } from "../../lib/api.ts";
import { ApiError } from "../../lib/apiError.ts";
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
  baselines: [] as (GlossaryEntry | undefined)[], calls: [] as GlossaryEntry[], identities: [] as (string | undefined)[], deletes: [] as unknown[][], toasts: [] as unknown[][], invalidations: [] as any[],
  queryError: null as Error | null, failureError: null as Error | null, fail: false, failure: "write failed", waitForSave: null as Promise<void> | null,
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
    const error = options.queryKey[0] === "glossary" ? h.queryError : null;
    return { data: state?.data, isLoading: false, isError: error !== null, error };
  },
  async save(entry: GlossaryEntry, expectedDatabase?: string, expectedEntry?: GlossaryEntry) {
    h.calls.push(entry); h.identities.push(expectedDatabase); h.baselines.push(expectedEntry);
    if (h.waitForSave) await h.waitForSave;
    if (h.fail) throw h.failureError ?? new Error(h.failure);
  },
  async delete(...args: unknown[]) {
    h.deletes.push(args);
    if (h.fail) throw h.failureError ?? new Error(h.failure);
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
        "../../lib/api": "export const getConfig=()=>{};export const getGlossary=()=>{};export const addGlossaryEntry=h.save;export const deleteGlossaryEntry=h.delete;",
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
  client.clear(); h.project = project; h.slots = []; h.effects = []; h.calls = []; h.identities = []; h.baselines = []; h.deletes = []; h.toasts = []; h.invalidations = [];
  h.queryError = null; h.failureError = null; h.fail = false; h.failure = "write failed"; h.waitForSave = null; setLocale("en");
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
  assert.deepEqual(h.baselines, [undefined]);
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

test("editing sends the full original baseline separately from every draft field", async () => {
  startEdit(); translation("Draft translation"); note("Draft note"); caseSensitive(false);
  // A background refresh must not silently advance the editor's baseline.
  client.setQueryData(["glossary", project.database_path, "en-es"], [{ ...original, translation: "Newer translation", context: null, case_sensitive: false }]);
  await save();
  assert.deepEqual(h.calls, [{ ...original, translation: "Draft translation", context: "Draft note", case_sensitive: false }]);
  assert.deepEqual(h.baselines, [original]);
});

const glossaryConflict = "glossary entry changed or was deleted since editing began; cancel and reopen the entry to review the latest version";
for (const locale of ["en", "es"] as const) {
  for (const deleted of [false, true]) {
    test(`${locale} stale-entry conflict refetches and retains the full draft after ${deleted ? "deletion" : "an update"}`, async () => {
      setLocale(locale);
      startEdit(); translation("Keep my draft"); note("Keep my note"); caseSensitive(false);
      h.fail = true; h.failureError = new ApiError(`${deleted ? "" : "409: "}${glossaryConflict}`);
      await save();
      assert.deepEqual(h.invalidations, [{ queryKey: ["glossary", project.database_path], refetchType: "active" }]);
      const refreshed = deleted ? [] : [{ ...original, translation: "Newer translation" }];
      client.setQueryData(["glossary", project.database_path, "en-es"], refreshed);
      assert.equal(control("settings.glossary.translation", row()).props.value, "Keep my draft");
      assert.equal(control("settings.glossary.note", row()).props.value, "Keep my note");
      assert.equal(control("settings.glossary.caseSensitive", row()).props.checked, false);
      assert.equal(action("settings.glossary.save", row()).props.disabled, false);
      assert.deepEqual(h.toasts, [["error", t("settings.glossary.toast.saveFailed" as TranslateKey, { error: label("api.error.glossaryConflict") })]]);
      // Retrying cannot overwrite the newer version or resurrect a deleted entry.
      await save();
      assert.deepEqual(h.baselines, [original, original]);
      action("common.cancel", row()).props.onClick();
      assert.deepEqual(client.getQueryData(["glossary", project.database_path, "en-es"]), refreshed);
      if (!deleted) {
        startEdit(); translation("Reviewed draft"); h.fail = false; await save();
        assert.deepEqual(h.baselines[2], refreshed[0]);
      }
    });
  }
}

test("a failed conflict refetch keeps the draft visible alongside the load error", async () => {
  startEdit(); translation("Keep this draft");
  h.fail = true; h.failureError = new ApiError(glossaryConflict);
  await save();
  h.queryError = new Error("Refresh failed");
  assert.equal(control("settings.glossary.translation", row()).props.value, "Keep this draft");
  assert.ok(renderToStaticMarkup(tree()).includes(t("settings.glossary.loadFailed", { error: "Refresh failed" })));
});

test("a refreshed row that no longer matches the filter keeps its editor visible", async () => {
  change(find(node => node.props?.placeholder === t("settings.glossary.filterPlaceholder")), "EE.UU.");
  startEdit(); translation("Keep this draft");
  h.fail = true; h.failureError = new ApiError(glossaryConflict);
  await save();
  client.setQueryData(["glossary", project.database_path, "en-es"], [{ ...original, translation: "Newer translation" }]);
  assert.equal(control("settings.glossary.translation", row()).props.value, "Keep this draft");
});

test("a conflict arriving after a project switch never refetches the new project's glossary", async () => {
  startEdit(); translation("Pending A");
  let finish!: () => void;
  h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
  h.fail = true; h.failureError = new ApiError(glossaryConflict);
  const pending = save();
  h.project = { ...project, database_path: "C:/other.db" };
  tree(); finish(); await pending;
  assert.deepEqual(h.invalidations, [{ queryKey: ["glossary", project.database_path], refetchType: "none" }]);
});

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


function fillAdd() {
  change(find(node => node.props?.placeholder === t("settings.glossary.sourceTerm")), "US");
  change(find(node => node.props?.placeholder === t("settings.glossary.preferredTranslation")), "EE.UU.");
}
function confirmDelete() {
  action("settings.glossary.deleteTitle", row()).props.onClick();
  return find(node => node.props?.open === true && typeof node.props?.onConfirm === "function").props.onConfirm();
}

for (const operation of ["add", "edit", "delete"]) {
  test(`${operation} sends the database identity captured by the rendered section`, async () => {
    if (operation === "add") {
      fillAdd(); await action("settings.glossary.addEntry").props.onClick();
    } else if (operation === "edit") {
      startEdit(); translation("Updated"); await save();
    } else {
      await confirmDelete();
    }
    if (operation === "delete") assert.deepEqual(h.deletes, [["US", "en-es", project.database_path]]);
    else assert.deepEqual(h.identities, [project.database_path]);
  });

  test(`${operation} conflict shows an error and does not report a successful mutation`, async () => {
    h.fail = true; h.failure = "409: project changed before glossary mutation; retry";
    if (operation === "add") {
      fillAdd(); await action("settings.glossary.addEntry").props.onClick();
      assert.equal(find(node => node.props?.placeholder === t("settings.glossary.sourceTerm")).props.value, "US");
      assert.equal(find(node => node.props?.placeholder === t("settings.glossary.preferredTranslation")).props.value, "EE.UU.");
    } else if (operation === "edit") {
      startEdit(); translation("Retry me"); await save();
      assert.equal(control("settings.glossary.translation", row()).props.value, "Retry me");
    } else {
      await confirmDelete();
      assert.ok(renderToStaticMarkup(tree()).includes("EE.UU."));
    }
    const key = operation === "edit" ? "saveFailed" : operation === "add" ? "addFailed" : "deleteFailed";
    assert.deepEqual(h.toasts, [["error", t(`settings.glossary.toast.${key}` as TranslateKey, { error: h.failure })]]);
    assert.deepEqual(h.invalidations, []);
    assert.deepEqual(client.getQueryData(["glossary", project.database_path, "en-es"]), [original]);
  });
}

test("an in-flight save keeps A's identity and never invalidates B as active", async () => {
  startEdit(); translation("Pending A");
  let finish!: () => void;
  h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
  const pending = save();
  h.project = { ...project, database_path: "C:/other.db" };
  client.setQueryData(["glossary", h.project.database_path, "en-es"], [original]);
  tree();
  finish(); await pending;
  assert.deepEqual(h.identities, [project.database_path]);
  assert.deepEqual(h.invalidations, [{ queryKey: ["glossary", project.database_path], refetchType: "none" }]);
});


test("an unsaved project uses the existing in-memory database identity", async () => {
  h.project = { ...project, database_path: undefined };
  fillAdd(); await action("settings.glossary.addEntry").props.onClick();
  assert.deepEqual(h.identities, [":memory:"]);
});

// Exercise the real glossary transport functions as well as the section's API seam.
const transport = { calls: [] as any[], setTauri: (_value: boolean) => {} };
(globalThis as any).__glossaryApiTest = transport;
const runtimeHook = registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source:
        "export let IS_TAURI=true;export const isTauri=()=>IS_TAURI;globalThis.__glossaryApiTest.setTauri=value=>{IS_TAURI=value;};" };
    }
    return nextLoad(url, context);
  },
});
const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
const previousFetch = globalThis.fetch;
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
    if (command === "get_server_port") return 7842;
    transport.calls.push([command, args]);
  } },
} });
globalThis.fetch = async (url, init) => {
  transport.calls.push([String(url), init?.method, init?.body ? JSON.parse(String(init.body)) : null]);
  return new Response(null, { status: 204 });
};
const realApi = await import("../../lib/api.ts");
// Initialize the fallback URL using Tauri's existing port seam before testing HTTP.
await realApi.deleteGlossaryEntry("bootstrap", "en-es");
after(() => {
  runtimeHook.deregister(); globalThis.fetch = previousFetch;
  if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__glossaryApiTest");
});
for (const tauri of [true, false]) {
  test(`${tauri ? "Tauri" : "HTTP"} glossary add carries optional expected database without changing the entry`, async () => {
    transport.setTauri(tauri);
    for (const expected of [undefined, "C:/project A & B.locust.db"]) {
      transport.calls = [];
      await realApi.addGlossaryEntry(original, expected);
      assert.deepEqual(transport.calls, tauri
        ? [["add_glossary_entry", expected === undefined ? { entry: original } : { entry: original, expectedDatabase: expected }]]
        : [["http://localhost:7842/api/glossary", "POST", expected === undefined ? original : { ...original, expected_database: expected }]]);
    }
  });

  test(`${tauri ? "Tauri" : "HTTP"} glossary edit carries the full expected entry with or without a project binding`, async () => {
    transport.setTauri(tauri);
    const draft = { ...original, translation: "Updated", context: null, case_sensitive: false };
    for (const expected of [undefined, "C:/project A & B.locust.db"]) {
      transport.calls = [];
      await realApi.addGlossaryEntry(draft, expected, original);
      assert.deepEqual(transport.calls, tauri
        ? [["add_glossary_entry", { entry: draft, expectedEntry: original, ...(expected === undefined ? {} : { expectedDatabase: expected }) }]]
        : [["http://localhost:7842/api/glossary", "POST", { ...draft, expected_entry: original, ...(expected === undefined ? {} : { expected_database: expected }) }]]);
    }
  });

  test(`${tauri ? "desktop fallback" : "HTTP"} glossary delete encodes the optional expected database`, async () => {
    transport.setTauri(tauri);
    for (const expected of [undefined, "C:/project A & B.locust.db"]) {
      transport.calls = [];
      await realApi.deleteGlossaryEntry("HP/MP", "en-es", expected);
      const suffix = expected === undefined ? "" : `&expected_database=${encodeURIComponent(expected)}`;
      assert.deepEqual(transport.calls, [[`http://localhost:7842/api/glossary/HP%2FMP?lang_pair=en-es${suffix}`, "DELETE", null]]);
    }
  });
}
