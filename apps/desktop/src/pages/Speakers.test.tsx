import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { cloneElement, createElement } from "react";
import { QueryClient } from "@tanstack/react-query";
import { setLocale, t } from "../lib/i18n/index.ts";
import { draftProjectKey } from "../stores/draftStore.ts";
import type { ProjectInfo } from "../lib/api.ts";

const project: ProjectInfo = { database_path: "C:/demo.db", path: "C:/game", format_id: "renpy", name: "Demo" };
const groups = { groups: [
  { id: "opaque:a", name: "Alice", count: 2, entry_ids: ["a", "b"], note: "Formal" },
  { id: "opaque:b", name: "Alice", count: 1, entry_ids: ["c"], note: "Informal" },
], unassigned_entry_ids: ["compiled#1"] };
const client = new QueryClient();
const h = {
  t,
  project: project as ProjectInfo | null, states: new Map<string, any[]>(), visited: new Set<string>(), slots: [] as any[], cursor: 0,
  queries: [] as any[], calls: [] as any[], ids: [] as string[], destination: "", fail: false,
  finish: null as null | (() => void), defer: false,
  async save(payload: any) {
    h.calls.push(payload);
    if (h.defer) await new Promise<void>(resolve => { h.finish = resolve; });
    if (h.fail) throw Error("write failed");
    return { speaker_id: payload.speaker_id, note: payload.note };
  },
  query(options: any) {
    h.queries.push(options);
    const state = client.getQueryState(options.queryKey);
    return { data: state?.data, isPending: !state || state.status === "pending", isError: state?.status === "error", error: state?.error, refetch() {} };
  },
};
(globalThis as any).__speakersTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/pages/Speakers.tsx")) {
      const stubs: Record<string, string> = {
        react: `export const useState=initial=>{const slots=h.slots,i=h.cursor++;if(!(i in slots))slots[i]=typeof initial==='function'?initial():initial;return [slots[i],value=>{slots[i]=typeof value==='function'?value(slots[i]):value;}];};
          export const useRef=initial=>{const slots=h.slots,i=h.cursor++;return slots[i]??={current:initial};};
          export const useEffect=effect=>{const slots=h.slots,i=h.cursor++;if(!(i in slots))slots[i]={cleanup:effect()};};`,
        "../lib/api": "export const getSpeakers=()=>{};export const saveSpeakerNote=h.save;",
        "../lib/i18n": "export const useT=()=>h.t;",
        "../stores/projectStore": "export const useProjectStore=Object.assign(select=>select(h),{getState:()=>h});",
        "../stores/editorStore": "export const useEditorStore=select=>select({startValidationWorklist:ids=>{h.ids=ids;}});",
        "@tanstack/react-query": "export const useQuery=h.query;export const useQueryClient=()=>globalThis.__speakersClient;",
        "react-router-dom": "export const useNavigate=()=>path=>{h.destination=path;};export const Link='a';",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__speakersTest;" + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
(globalThis as any).__speakersClient = client;
const { default: Speakers } = await import("./Speakers.tsx");
function expand(node: any, scope = "root"): any {
  if (!node || typeof node !== "object") return node;
  if (Array.isArray(node)) return node.map(n => expand(n, scope));
  if (typeof node.type === "function" && ["ProjectSpeakers", "SpeakerNoteEditor"].includes(node.type.name)) {
    scope += `/${node.type.name}/${node.key}`;
    h.visited.add(scope);
    h.slots = h.states.get(scope) ?? []; h.states.set(scope, h.slots); h.cursor = 0;
    return cloneElement(expand(node.type(node.props), scope), { key: node.key });
  }
  const children = expand(node.props?.children, scope);
  return cloneElement(node, {}, ...(Array.isArray(children) ? children : [children]));
}
function tree() {
  h.visited.clear();
  const result = expand(Speakers());
  // An absent component really unmounts: remounting its key gets fresh hooks,
  // and late setters still target only the disposed instance's slots.
  for (const [scope, slots] of h.states) {
    if (!h.visited.has(scope)) {
      for (const slot of slots) slot?.cleanup?.();
      h.states.delete(scope);
    }
  }
  return result;
}
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function html() { return renderToStaticMarkup(createElement(MemoryRouter, {}, tree())); }
function button(key: Parameters<typeof t>[0]) { return nodes(tree()).find(n => n.type === "button" && n.props.children === t(key)); }
function select(id = "opaque:a") {
  nodes(tree()).find(n => n.type === "button" && n.key === id).props.onClick();
  tree();
}
function edit(value: string) { nodes(tree()).find(n => n.type === "textarea").props.onChange({ target: { value } }); }
function seed(data = groups) { client.setQueryData(["speakers", draftProjectKey(h.project!)], data); }
beforeEach(() => { client.clear(); h.states.clear(); h.project = project; h.calls = []; h.queries = []; h.ids = []; h.destination = ""; h.fail = false; h.defer = false; h.finish = null; setLocale("en"); });
after(() => { hooks.deregister(); client.clear(); Reflect.deleteProperty(globalThis, "__speakersTest"); Reflect.deleteProperty(globalThis, "__speakersClient"); setLocale("en"); });

for (const locale of ["en", "es"] as const) {
  test(`${locale}: speaker groups, unassigned rows, localized explanation and selected notes`, () => {
    setLocale(locale); seed(); select();
    const markup = html();
    for (const key of ["speakers.title", "speakers.unassigned", "speakers.contextHint", "speakers.evidenceHint", "speakers.note", "speakers.save", "speakers.clear"] as const) assert.ok(markup.includes(t(key)), key);
    assert.match(markup, /Alice \(2\)/);
    assert.match(markup, /Alice \(1\)/);
    assert.ok(markup.includes(`${t("speakers.unassigned")} (1)`));
    assert.match(markup, />Formal<\/textarea>/);
    select("opaque:b");
    assert.match(html(), />Informal<\/textarea>/, "same display labels must not merge identities");
  });
  test(`${locale}: no project, missing ownership, empty and loading states`, () => {
    setLocale(locale); h.project = null;
    assert.ok(html().includes(t("speakers.noProject")));
    assert.equal(h.queries[h.queries.length - 1].enabled, false);
    h.project = { ...project, database_path: undefined };
    assert.ok(html().includes(t("speakers.reopen")));
    assert.equal(h.queries[h.queries.length - 1].enabled, false);
    h.project = project;
    assert.match(html(), /role="status"/);
    seed({ groups: [], unassigned_entry_ids: [] });
    assert.ok(html().includes(t("speakers.empty")));
    seed({ groups: [], unassigned_entry_ids: ["unknown"] });
    assert.ok(html().includes(`${t("speakers.unassigned")} (1)`));
  });
}

test("note save, clear, pending and failure retain exact opaque ownership and draft", async () => {
  seed(); select(); edit("New note"); h.defer = true;
  const saving = button("speakers.save").props.onClick();
  assert.equal(button("speakers.save").props.disabled, true);
  assert.ok(html().includes(t("speakers.saving")));
  h.finish!(); await saving;
  assert.deepEqual(h.calls[0], { database_path: project.database_path, project_path: project.path, format_id: "renpy", speaker_id: "opaque:a", note: "New note" });
  assert.ok(html().includes(t("speakers.saved")));
  h.defer = false;
  await button("speakers.clear").props.onClick();
  assert.equal(h.calls[h.calls.length - 1].note, "");
  assert.match(html(), /<textarea[^>]*><\/textarea>/);
  edit("Retain on failure"); h.fail = true;
  await button("speakers.save").props.onClick();
  assert.match(html(), /role="alert"/);
  assert.match(html(), /write failed/);
  assert.match(html(), />Retain on failure<\/textarea>/);
});

for (const changed of [{ database_path: "C:/other.db" }, { path: "C:/other" }, { format_id: "html" }]) {
  test(`project switch isolates drafts, query data, late save and stale handlers: ${JSON.stringify(changed)}`, async () => {
    seed(); select(); edit("Old draft");
    const staleSave = button("speakers.save").props.onClick;
    const staleOpen = button("speakers.openRows").props.onClick;
    h.defer = true;
    const saving = staleSave();
    h.project = { ...project, ...changed };
    assert.ok(!html().includes("Old draft"));
    assert.equal(h.queries[h.queries.length - 1].queryKey[1], draftProjectKey(h.project));
    seed({ ...groups, groups: [{ ...groups.groups[0], note: "Other project note" }] }); select();
    h.finish!(); await saving;
    assert.match(html(), />Other project note<\/textarea>/);
    assert.ok(!html().includes(t("speakers.saved")));
    await staleSave(); staleOpen();
    assert.equal(h.calls.length, 1);
    assert.equal(h.destination, "");
  });
}

test("pending note survives actual selected-editor unmount/remount and completion updates the visible note", async () => {
  seed(); select(); edit("Saved Alice note"); h.defer = true;
  const saving = button("speakers.save").props.onClick();
  const editorScope = [...h.states.keys()].find(scope => scope.includes("SpeakerNoteEditor"))!;
  const originalSlots = h.states.get(editorScope);
  select("opaque:b");
  assert.equal(h.states.has(editorScope), false, "Alice's editor hooks must be disposed");
  select();
  assert.notEqual(h.states.get(editorScope), originalSlots, "Alice really remounts");
  const textarea = nodes(tree()).find(n => n.type === "textarea");
  assert.equal(textarea.props.value, "Saved Alice note");
  assert.equal(button("speakers.save").props.disabled, true);
  assert.ok(html().includes(t("speakers.saving")));
  h.finish!(); await saving;
  assert.equal(nodes(tree()).find(n => n.type === "textarea").props.value, "Saved Alice note");
  assert.equal(button("speakers.save").props.disabled, false);
  assert.ok(html().includes(t("speakers.saved")));
});

test("save acknowledgements preserve newer edits and stale save handlers cannot submit older text", async () => {
  seed(); select(); edit("First request"); h.defer = true;
  const staleSave = button("speakers.save").props.onClick;
  const saving = staleSave();
  assert.notEqual(nodes(tree()).find(n => n.type === "textarea").props.disabled, true, "typing remains possible while saving");
  await staleSave();
  assert.equal(h.calls.length, 1, "pending ownership synchronously rejects duplicate saves");
  edit("Newer draft"); select("opaque:b"); select();
  h.finish!(); await saving;
  assert.equal(nodes(tree()).find(n => n.type === "textarea").props.value, "Newer draft");
  assert.ok(!html().includes(t("speakers.saved")));
  h.defer = false;
  await button("speakers.save").props.onClick();
  assert.equal(h.calls[1].note, "Newer draft");
  await staleSave();
  assert.equal(h.calls.length, 2, "an older render cannot resend its captured text");
  assert.equal(nodes(tree()).find(n => n.type === "textarea").props.value, "Newer draft");
});

test("a disposed project view's completion cannot regress a newer mounted view's save or cache", async () => {
  seed(); select(); edit("Old request"); h.defer = true;
  const oldSave = button("speakers.save").props.onClick();
  const finishOld = h.finish!;
  h.project = null; tree(); // Dispose the entire owning view, not just its editor.
  h.project = project; select(); edit("New mounted save"); h.defer = false;
  await button("speakers.save").props.onClick();
  finishOld(); await oldSave;
  assert.equal(h.calls.length, 2);
  assert.equal(nodes(tree()).find(n => n.type === "textarea").props.value, "New mounted save");
  assert.equal(client.getQueryData<typeof groups>(["speakers", draftProjectKey(project)])!.groups[0].note, "New mounted save");
});

test("failed save survives selected-editor remount without losing its draft or pending/error ownership", async () => {
  seed(); select(); edit("Retain Alice draft"); h.defer = true; h.fail = true;
  const saving = button("speakers.save").props.onClick();
  select("opaque:b"); select();
  assert.equal(button("speakers.save").props.disabled, true);
  h.finish!(); await saving;
  assert.equal(nodes(tree()).find(n => n.type === "textarea").props.value, "Retain Alice draft");
  assert.equal(button("speakers.save").props.disabled, false);
  assert.match(html(), /write failed/);
  select("opaque:b");
  assert.ok(!html().includes("write failed"));
});

test("query refresh reconciles a clean note but never discards a genuinely dirty draft", async () => {
  seed(); select();
  const saveBeforeRefresh = button("speakers.save").props.onClick;
  const refresh = (note: string) => seed({ ...groups, groups: [{ ...groups.groups[0], note }, groups.groups[1]] });
  refresh("Refreshed note");
  assert.equal(nodes(tree()).find(n => n.type === "textarea").props.value, "Refreshed note");
  await saveBeforeRefresh();
  assert.equal(h.calls[0].note, "Refreshed note", "a pre-refresh handler must not send stale clean text");
  edit("My draft"); refresh("New server note");
  assert.equal(nodes(tree()).find(n => n.type === "textarea").props.value, "My draft");
  select("opaque:b"); select();
  assert.equal(nodes(tree()).find(n => n.type === "textarea").props.value, "My draft");
  edit("Refreshed note"); // Returning to the edit baseline is no longer dirty.
  assert.equal(nodes(tree()).find(n => n.type === "textarea").props.value, "New server note");
});

test("group and unassigned links reuse existing editor worklists; load errors offer retry", () => {
  seed(); select(); button("speakers.openRows").props.onClick();
  assert.deepEqual(h.ids, ["a", "b"]); assert.equal(h.destination, "/editor");
  button("speakers.openUnassigned").props.onClick(); assert.deepEqual(h.ids, ["compiled#1"]);
  client.getQueryCache().find({ queryKey: ["speakers", draftProjectKey(project)] })!.setState({ status: "error", error: Error("read failed") });
  assert.match(html(), /role="alert"/);
  assert.ok(html().includes(t("common.retry")));
});

test("speaker route/navigation are additive and preserve previous destinations and labels", () => {
  const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
  const layout = readFileSync(new URL("../components/Layout.tsx", import.meta.url), "utf8");
  assert.ok(app.includes('<Route path="/speakers" element={<Speakers />} />'));
  for (const [path, label] of [["/", "home"], ["/dashboard", "dashboard"], ["/editor", "editor"], ["/review", "review"], ["/qa", "qa"], ["/memory", "memory"], ["/settings", "settings"], ["/speakers", "speakers"]]) {
    assert.match(layout, new RegExp(`to: "${path}"[^\\n]*labelKey: "nav.${label}"`));
  }
});
