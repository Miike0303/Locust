import assert from "node:assert/strict";
import { test } from "node:test";
import { registerHooks } from "node:module";
import { ApiError } from "../lib/apiError";
import { setLocale, t } from "../lib/i18n";
import * as drafts from "../stores/draftStore";
import { useProjectStore } from "../stores/projectStore";
import { useEditorStore } from "../stores/editorStore";
import type { StringEntry } from "../lib/api";

const project = { path: "C:/fixture", name: "Fixture", format_id: "html-game" };
const projectKey = drafts.draftProjectKey(project);
const key = drafts.draftEntryKey(projectKey, "row");
const conflict = "translation changed since it was loaded";
const h = {
  t, drafts, useProjectStore, useEditorStore,
  states: [] as any[], cursor: 0, columns: [] as any[],
  server: "V0" as string | null, calls: [] as any[], failRead: false,
  async patchString(_id: string, data: any) {
    h.calls.push(data);
    if ("expected_translation" in data && data.expected_translation !== h.server) throw new ApiError(`409: ${conflict}`);
    if ("translation" in data) h.server = data.translation;
    return { translation: h.server };
  },
  async getString() {
    if (h.failRead) throw new Error("offline");
    return { translation: h.server };
  },
};
(globalThis as any).__staleSave = h;
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (/\/components\/(DetailPanel|StringTable)\.tsx$/.test(context.parentURL ?? "")) {
      const preamble = "const h=globalThis.__staleSave;";
      const stubs: Record<string, string> = {
        react: `export const useState=initial=>{const i=h.cursor++;if(!(i in h.states))h.states[i]=initial;return [h.states[i],value=>{h.states[i]=value;}];};export const useRef=initial=>{const i=h.cursor++;return h.states[i]??=( {current:initial});};export const useEffect=()=>{};export const useMemo=fn=>fn();`,
        "../lib/i18n": "export const useT=()=>h.t;",
        "../lib/api": "export const patchString=h.patchString;export const getString=h.getString;export const getConfig=async()=>({});export const encodedByteLen=()=>0;",
        "../stores/draftStore": `export const useDraftStore=Object.assign(selector=>selector(h.drafts.useDraftStore.getState()),{getState:h.drafts.useDraftStore.getState});${Object.keys(drafts).filter(k => k !== "useDraftStore").map(k => `export const ${k}=h.drafts.${k};`).join("")}`,
        "../stores/projectStore": "export const useProjectStore=Object.assign(selector=>selector(h.useProjectStore.getState()),{getState:h.useProjectStore.getState});",
        "../stores/editorStore": "export const useEditorStore=selector=>selector?selector(h.useEditorStore.getState()):h.useEditorStore.getState();",
        "@tanstack/react-query": "export const useQuery=()=>({data:undefined});",
        "@tanstack/react-table": "export const useReactTable=options=>{h.columns=options.columns;return {getHeaderGroups:()=>[],getRowModel:()=>({rows:[]})};};export const getCoreRowModel=()=>{};export const getSortedRowModel=()=>{};export const flexRender=()=>null;",
      };
      if (stubs[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(preamble + stubs[specifier])}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: DetailPanel } = await import("./DetailPanel");
const { default: StringTable } = await import("./StringTable");
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
const entry: StringEntry = { id: "row", source: "Source", translation: "V0", status: "translated", tags: [], metadata: {}, created_at: "2026-01-01", file_path: "story.html", context: null, provider_used: null, char_limit: null, translated_at: null, reviewed_at: null };
function render(kind: string, row = entry) {
  h.cursor = 0;
  if (kind === "detail") return DetailPanel({ projectKey, entry: row, onRefetch() {}, onClose() {} });
  const inlineStates = h.states;
  h.states = [];
  StringTable({ data: [row], onRefetch() {} });
  h.states = inlineStates;
  const cell = h.columns.find(c => c.accessorKey === "translation").cell({ row: { original: row } });
  h.cursor = 0;
  return cell.type(cell.props);
}
function reset() {
  h.states = []; h.calls = []; h.server = "V0"; h.failRead = false;
  drafts.useDraftStore.setState({ drafts: {}, records: [], persistenceIssue: null, persistenceIssues: {} });
  useProjectStore.setState({ project });
  setLocale("en");
}
async function editAndSave(kind: string) {
  let tree = render(kind);
  if (kind === "inline") {
    nodes(tree).find(n => n.type === "button" && n.props["aria-label"] === t("table.editTranslation")).props.onClick({ stopPropagation() {} });
    tree = render(kind);
  }
  const textarea = nodes(tree).find(n => n.type === "textarea");
  textarea.props.onFocus?.();
  textarea.props.onChange({ target: { value: "A draft" } });
  h.server = "V1";
  // A refetch while editing must never rebase A's draft onto B's value.
  tree = render(kind, { ...entry, translation: "V1" });
  await nodes(tree).find(n => n.type === "textarea").props.onBlur();
}
for (const kind of ["detail", "inline"]) {
  test(`${kind}: conflict preserves text and offers localized recovery actions`, async () => {
    reset();
    await editAndSave(kind);
    assert.equal(h.server, "V1");
    assert.equal(h.calls[0].expected_translation, "V0");
    assert.equal(drafts.useDraftStore.getState().drafts[key].text, "A draft");
    for (const locale of ["en", "es"] as const) {
      setLocale(locale);
      const tree = nodes(render(kind));
      assert.ok(tree.some(n => n.props.role === "alert" && n.props.children === t("api.error.translationConflict")));
      assert.ok(tree.some(n => n.type === "button" && n.props.children === t("detail.loadLatest")));
      assert.ok(tree.some(n => n.type === "button" && n.props.children === t("detail.overwrite")));
    }
  });
  test(`${kind}: Load latest fetches the current value and rebases the draft`, async () => {
    reset();
    await editAndSave(kind);
    h.server = "V2";
    const button = nodes(render(kind)).find(n => n.type === "button" && n.props.children === t("detail.loadLatest"));
    assert.ok(button, "Load latest is explicit");
    await button.props.onClick();
    assert.equal(drafts.useDraftStore.getState().drafts[key].text, "V2");
    assert.equal(drafts.useDraftStore.getState().drafts[key].baseline, "V2");
    assert.equal(h.calls.length, 1, "loading must not write");
  });
  test(`${kind}: Overwrite resends the retained text without the guard`, async () => {
    reset();
    await editAndSave(kind);
    const button = nodes(render(kind)).find(n => n.type === "button" && n.props.children === t("detail.overwrite"));
    assert.ok(button, "Overwrite is explicit");
    await button.props.onClick();
    assert.equal(h.server, "A draft");
    assert.equal(h.calls.length, 2);
    assert.equal(h.calls[1].expected_translation, undefined);
    assert.equal(drafts.useDraftStore.getState().drafts[key].error, null);
  });
  test(`${kind}: a failed Load latest shows the failure and keeps both recovery actions`, async () => {
    reset();
    await editAndSave(kind);
    h.failRead = true;
    const button = nodes(render(kind)).find(n => n.type === "button" && n.props.children === t("detail.loadLatest"));
    await button.props.onClick();
    const tree = nodes(render(kind));
    assert.equal(drafts.useDraftStore.getState().drafts[key].text, "A draft");
    assert.ok(tree.some(n => n.props.role === "alert" && n.props.children === "offline"));
    assert.ok(tree.some(n => n.type === "button" && n.props.children === t("detail.overwrite")));
  });
}

test("inline: cancelling a further edit does not disable explicit Overwrite", async () => {
  reset();
  await editAndSave("inline");
  nodes(render("inline")).find(n => n.type === "button" && n.props["aria-label"] === t("table.editTranslation")).props.onClick({ stopPropagation() {} });
  nodes(render("inline")).find(n => n.type === "textarea").props.onKeyDown({ key: "Escape", preventDefault() {}, stopPropagation() {} });
  await nodes(render("inline")).find(n => n.type === "button" && n.props.children === t("detail.overwrite")).props.onClick();
  assert.equal(h.server, "A draft");
});
