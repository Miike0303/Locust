import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { setLocale, t } from "./i18n";
import type { ProjectInfo, StringEntry, TextFitResponse } from "./api";
import * as drafts from "../stores/draftStore";

// Render the actual component and hook with persistent hook slots. Effects
// flush separately so a stale response can arrive before effect cleanup.
const h = {
  slots: [] as any[], cursor: 0, effects: [] as (() => void)[], cleanups: [] as (() => void)[],
  calls: [] as any[], requests: [] as { resolve: (value: TextFitResponse) => void; reject: (error: Error) => void }[],
  project: { key: "project-a" } as ({ key: string } & Partial<ProjectInfo>) | null, t, drafts,
  writes: [] as any[], server: "WWW" as string | null,
  confirmed: "project-a" as string | null, transition: false,
  setTauri: (_value: boolean) => {}, transport: [] as any[], pickerCalls: [] as unknown[], pickerValue: null as string | null,
};
(globalThis as any).__textFit = h;
const mocks: Record<string, string> = {
  react: `const h=globalThis.__textFit;
    export const useState=initial=>{const i=h.cursor++;if(!(i in h.slots))h.slots[i]=typeof initial==='function'?initial():initial;return [h.slots[i],v=>{h.slots[i]=typeof v==='function'?v(h.slots[i]):v;}];};
    export const useRef=initial=>{const i=h.cursor++;return h.slots[i]??={current:initial};};
    export const useEffect=(fn,deps)=>{const i=h.cursor++;const old=h.slots[i];if(!old||deps.some((d,j)=>!Object.is(d,old[j]))) {h.slots[i]=deps;h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=fn();});}};`,
  "../lib/api": `export const estimateTextFit=(body,signal)=>{const h=globalThis.__textFit;h.calls.push({body,signal});return new Promise((resolve,reject)=>h.requests.push({resolve,reject}));};`,
  "../lib/i18n": "export const useT=()=>globalThis.__textFit.t;",
  "../stores/projectStore": "const state=()=>{const h=globalThis.__textFit;return {project:h.project,confirmedOpenProjectKey:h.confirmed,projectTransitionInProgress:h.transition};};export const useProjectStore=Object.assign(selector=>selector(state()),{getState:state});",
  "../lib/projectScope": "export const draftProjectKey=project=>project.key;export const isProjectScopedQueryEnabled=key=>{const h=globalThis.__textFit;return !h.transition&&h.confirmed===key;};",
  "@tauri-apps/plugin-dialog": "export const open=async options=>{const h=globalThis.__textFit;h.pickerCalls.push(options);return h.pickerValue;};",
};
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    const parent = context.parentURL ?? "";
    if (parent.endsWith("/components/TextFitPreview.tsx") && mocks[specifier]) {
      return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(mocks[specifier])}` };
    }
    if (parent.endsWith("/components/DetailPanel.tsx")) {
      const source: Record<string, string> = {
        react: mocks.react, "../lib/i18n": mocks["../lib/i18n"], "../stores/projectStore": mocks["../stores/projectStore"],
        "../lib/modalA11y": "export const isTopmostOverlayDialog=()=>false;export const useModalA11y=()=>({dialogRef:{current:null},dialogProps:{}});",
        "../lib/layoutPrefs": "export const isCompactLayout=()=>false;export const subscribeCompactLayout=()=>()=>{};",
        "../lib/api": "const h=globalThis.__textFit;export const encodedByteLen=()=>0;export const getString=async()=>({translation:h.server});export const patchString=async(id,data)=>{h.writes.push(data);h.server=data.translation;return {id,translation:h.server};};",
        "../stores/draftStore": `const h=globalThis.__textFit;export const useDraftStore=Object.assign(selector=>selector(h.drafts.useDraftStore.getState()),{getState:h.drafts.useDraftStore.getState});${Object.keys(drafts).filter(key => key !== "useDraftStore").map(key => `export const ${key}=h.drafts.${key};`).join("")}`,
      };
      if (source[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source[specifier])}` };
    }
    if (parent.endsWith("/lib/api.ts") && specifier === "@tauri-apps/api/core") {
      return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("export const invoke=async (cmd,args)=>{const h=globalThis.__textFit;if(cmd==='get_server_port')return 7842;h.transport.push([cmd,args]);return {source:[],draft:[]};};")}` };
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) return { format: "module", shortCircuit: true,
      source: "export let IS_TAURI=true;export const isTauri=()=>IS_TAURI;globalThis.__textFit.setTauri=value=>{IS_TAURI=value;};" };
    return nextLoad(url, context);
  },
});
const ui = await import("../components/TextFitPreview.tsx");
const { default: DetailPanel } = await import("../components/DetailPanel.tsx");
const api = await import("./api");
const row: StringEntry = { id: "Map001.json#event_1#page_0#cmd_1#msg", source: "iii", translation: "WWW",
  metadata: { format_id: "rpgmaker-mv" }, tags: ["dialogue"], file_path: "Map001.json", context: null,
  status: "pending", provider_used: null, char_limit: null, created_at: "2026-01-01", translated_at: null, reviewed_at: null };
const validProfile = { fontPath: "fixture.ttf", fontSize: "20", usableWidth: "30" };
const response: TextFitResponse = {
  source: [{ status: "within_width", advance_px: 12, usable_width_px: 30, reason: null }],
  draft: [{ status: "exceeds_width", advance_px: 54, usable_width_px: 30, reason: null }],
};
const storage = new Map<string, string>();
const oldStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
const originalFetch = globalThis.fetch;
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), removeItem: (key: string) => storage.delete(key),
} });
let options: Parameters<typeof ui.useTextFitEstimate>[0];
beforeEach(context => {
  assert.ok("mock" in context, "text-fit fixtures run in a test context");
  h.cleanups.forEach(fn => fn?.()); h.slots = []; h.effects = []; h.cleanups = []; h.cursor = 0;
  h.calls = []; h.requests = []; h.transport = []; h.project = { key: "project-a" }; storage.clear(); h.pickerCalls = []; h.pickerValue = null;
  h.confirmed = "project-a"; h.transition = false;
  h.writes = []; h.server = "WWW"; drafts.useDraftStore.setState({ drafts: {}, records: [], persistenceIssue: null, persistenceIssues: {} });
  options = { projectKey: "project-a", entry: row, draft: "WWW", profile: validProfile, enabled: true, active: true };
  h.setTauri(true); setLocale("en");
  context.mock.timers.enable({ apis: ["setTimeout"] });
});
after(() => {
  h.cleanups.forEach(fn => fn?.()); hooks.deregister(); globalThis.fetch = originalFetch; setLocale("en");
  if (oldStorage) Object.defineProperty(globalThis, "localStorage", oldStorage); else Reflect.deleteProperty(globalThis, "localStorage");
  Reflect.deleteProperty(globalThis, "__textFit");
});
function flush() { h.effects.splice(0).forEach(fn => fn()); }
function estimate(effects = true) { h.cursor = 0; const value = ui.useTextFitEstimate(options); if (effects) flush(); return value; }
async function finish(index = 0, value = response) { h.requests[index].resolve(value); await Promise.resolve(); await Promise.resolve(); }
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  return Array.isArray(node) ? node.flatMap(nodes) : [node, ...nodes(node.props?.children)];
}
function component(projectKey = "project-a", entry = row, draft = "WWW") {
  h.cursor = 0; const view = ui.default({ projectKey, entry, draft }); flush(); return view;
}
function control(suffix: string) { const found = nodes(component()).find(node => node.props?.id?.endsWith(suffix)); assert.ok(found, suffix); return found; }
function expand() { nodes(component()).find(node => node.type === "button" && node.props["aria-expanded"] === false).props.onClick(); return component(); }
function markup() { return renderToStaticMarkup(component()); }
const detailProject = { path: "C:/text-fit-fixture", format_id: "rpgmaker-mv", name: "Fixture" };
const detailProjectKey = drafts.draftProjectKey(detailProject);
const detailDraftKey = drafts.draftEntryKey(detailProjectKey, row.id);
function detail() {
  h.cursor = 0;
  const view = DetailPanel({ projectKey: detailProjectKey, entry: row, onRefetch() {}, onClose() {} });
  flush(); return view;
}
function unsavedDetail() {
  h.project = { ...detailProject, key: detailProjectKey }; h.confirmed = detailProjectKey;
  nodes(detail()).find(node => node.type === "textarea").props.onChange({ target: { value: "Unsaved Latin draft" } });
  return detail();
}

test("eligibility requires extractor provenance, dialogue kind and message locator, with no filename inference", () => {
  assert.equal(ui.isTextFitEligible(row), true);
  for (const invalid of [
    { ...row, metadata: {} }, { ...row, metadata: { format_id: "rpgmaker-mz" } }, { ...row, metadata: { format_id: "rpgmaker-vxa" } },
    { ...row, tags: ["scroll_text"] }, { ...row, tags: ["dialogue", "scroll_text"] }, { ...row, tags: ["choice"] }, { ...row, tags: ["speaker"] },
    { ...row, id: "Actors.json#1#description" }, { ...row, id: "System.json#terms#messages#1" },
    { ...row, id: "pack.json#cmd_1#plugin" }, { ...row, id: "Map001.json#cmd_x#msg" },
  ]) assert.equal(ui.isTextFitEligible(invalid), false, invalid.id);
  assert.equal(ui.isTextFitEligible({ ...row, file_path: "unusual.bin" }), true);
});

test("profiles start blank, persist per project, and tolerate corrupt or unavailable storage", () => {
  assert.deepEqual(ui.loadTextFitProfile("project-a"), { fontPath: "", fontSize: "", usableWidth: "" });
  ui.persistTextFitProfile("project-a", validProfile);
  assert.deepEqual(ui.loadTextFitProfile("project-a"), validProfile);
  assert.deepEqual(ui.loadTextFitProfile("project-b"), { fontPath: "", fontSize: "", usableWidth: "" });
  const broken = { getItem() { throw Error("restricted"); }, setItem() { throw Error("quota"); }, removeItem() {} };
  assert.deepEqual(ui.loadTextFitProfile("a", broken), { fontPath: "", fontSize: "", usableWidth: "" });
  assert.doesNotThrow(() => ui.persistTextFitProfile("a", validProfile, broken));
  const key = [...storage.keys()].find(value => value.startsWith("locust.textFit."))!; storage.set(key, "{bad");
  assert.deepEqual(ui.loadTextFitProfile("project-a"), { fontPath: "", fontSize: "", usableWidth: "" });
});

test("request requires explicit positive finite geometry and preserves draft whitespace and empty text", () => {
  for (const field of ["fontSize", "usableWidth"] as const) for (const value of ["", " ", "0", "-1", "Infinity", "NaN"]) {
    assert.equal(ui.textFitRequest(row.id, "", { ...validProfile, [field]: value }), null);
  }
  assert.equal(ui.textFitRequest(row.id, "", { ...validProfile, fontPath: " " }), null);
  assert.deepEqual(ui.textFitRequest(row.id, " a \r\n\n", validProfile), { entry_id: row.id, draft_text: " a \r\n\n", font_path: "fixture.ttf", font_size_px: 20, usable_width_px: 30 });
  assert.equal(ui.textFitRequest(row.id, "", validProfile)?.draft_text, "");
});

test("requests debounce for 250 ms, send independent current draft, and leave row/profile intact", async context => {
  const snapshot = JSON.stringify({ row, validProfile });
  assert.equal(estimate().loading, true); context.mock.timers.tick(249); assert.equal(h.calls.length, 0);
  options = { ...options, draft: "Hola" }; estimate(); context.mock.timers.tick(249); assert.equal(h.calls.length, 0);
  context.mock.timers.tick(1); assert.equal(h.calls.length, 1);
  assert.deepEqual(h.calls[0].body, { entry_id: row.id, font_path: "fixture.ttf", font_size_px: 20, usable_width_px: 30, draft_text: "Hola" });
  await finish(); assert.deepEqual(estimate().data, response);
  assert.equal(JSON.stringify({ row, validProfile }), snapshot);
});

const changes: Record<string, () => void> = {
  row: () => { options = { ...options, entry: { ...row, id: "Map002.json#cmd_1#msg" } }; },
  project: () => { options = { ...options, projectKey: "project-b" }; h.confirmed = "project-b"; },
  source: () => { options = { ...options, entry: { ...row, source: "other source" } }; },
  draft: () => { options = { ...options, draft: "new draft" }; },
  font: () => { options = { ...options, profile: { ...validProfile, fontPath: "other.otf" } }; },
  size: () => { options = { ...options, profile: { ...validProfile, fontSize: "21" } }; },
  width: () => { options = { ...options, profile: { ...validProfile, usableWidth: "31" } }; },
};
for (const [name, change] of Object.entries(changes)) test(`${name}: invalidation is synchronous and latest request wins`, async context => {
  estimate(); context.mock.timers.tick(250); const oldSignal = h.calls[0].signal;
  change(); assert.equal(estimate(false).data, undefined); assert.equal(oldSignal.aborted, true);
  await finish(); assert.equal(estimate(false).data, undefined, "obsolete success ignored before effect cleanup");
  flush(); context.mock.timers.tick(250); assert.equal(h.calls.length, 2);
  await finish(1); assert.deepEqual(estimate().data, response);
  options = { ...options, draft: options.draft + "!" }; assert.equal(estimate(false).data, undefined, "no old result frame");
});

test("collapsed, invalid, ineligible and inactive profiles do not request; unmount cancels owned work", async context => {
  for (const change of [ { enabled: false }, { active: false }, { entry: { ...row, tags: ["scroll_text"] } }, { profile: { ...validProfile, fontSize: "0" } } ]) {
    options = { projectKey: "project-a", entry: row, draft: "", profile: validProfile, enabled: true, active: true, ...change };
    estimate(); context.mock.timers.tick(250);
  }
  assert.equal(h.calls.length, 0);
  options = { ...options, profile: validProfile }; estimate(); context.mock.timers.tick(250);
  h.cleanups.forEach(fn => fn?.()); assert.equal(h.calls[0].signal.aborted, true);
  await finish(); assert.equal(estimate(false).data, undefined);
});

test("late failures cannot replace current results; returning to a previous identity does not revive old data", async context => {
  estimate(); context.mock.timers.tick(250); options = { ...options, draft: "B" }; estimate(); context.mock.timers.tick(250);
  await finish(1); h.requests[0].reject(Error("obsolete error")); await Promise.resolve(); await Promise.resolve();
  assert.deepEqual(estimate().data, response); assert.equal(estimate().error, undefined);
  options = { ...options, draft: "WWW" }; assert.equal(estimate(false).data, undefined);
  options = { ...options, draft: "B" }; assert.equal(estimate(false).data, undefined);
});

test("live project transition blocks debounced dispatch before React rerenders", context => {
  estimate(); h.transition = true; context.mock.timers.tick(250); assert.equal(h.calls.length, 0);
  assert.equal(estimate().active, false);
  h.transition = false; h.confirmed = "project-b"; estimate(); context.mock.timers.tick(250); assert.equal(h.calls.length, 0);
});

test("live project transition rejects completion before rerender and hides a rendered result immediately", async context => {
  estimate(); context.mock.timers.tick(250); h.transition = true;
  await finish(); assert.equal(estimate(false).data, undefined);
  h.transition = false; estimate(); context.mock.timers.tick(250); await finish(1); assert.deepEqual(estimate().data, response);
  h.transition = true; assert.equal(estimate(false).data, undefined); assert.equal(h.calls[1].signal.aborted, true);
});

for (const locale of ["en", "es"] as const) test(`${locale}: collapsed, configuration, loading, metrics, warnings, unavailable and no-text states`, async context => {
  setLocale(locale);
  assert.ok(markup().includes(t("textFit.title"))); assert.ok(!markup().includes(t("textFit.font"))); assert.equal(h.calls.length, 0);
  expand(); assert.ok(markup().includes(t("textFit.configure"))); assert.equal(control("-size").props.value, "");
  for (const [suffix, value] of [["-font", "fixture.ttf"], ["-size", "20"], ["-width", "30"]]) control(suffix).props.onChange({ target: { value } });
  assert.deepEqual(ui.loadTextFitProfile("project-a"), validProfile);
  assert.ok(markup().includes(t("textFit.loading"))); context.mock.timers.tick(250); await finish();
  const html = markup();
  for (const value of ["12 px / 30 px", "54 px / 30 px", t("textFit.within"), t("textFit.exceeds"), t("textFit.linesHint"), t("textFit.limits")]) assert.ok(html.includes(value), value);
  assert.match(html, /bg-text-muted/); assert.match(html, /bg-warning/); assert.ok(!html.includes("text-success"));
  component("project-a", row, ""); context.mock.timers.tick(250);
  await finish(1, { source: [{ status: "unavailable", advance_px: null, usable_width_px: 30, reason: "U+3042 is unsupported" }], draft: [{ status: "within_width", advance_px: 0, usable_width_px: 30, reason: null }] });
  const independent = renderToStaticMarkup(component("project-a", row, ""));
  assert.ok(independent.includes("U+3042 is unsupported")); assert.ok(independent.includes(t("textFit.noText"))); assert.ok(independent.includes("0 px / 30 px"));
});

for (const locale of ["en", "es"] as const) test(`${locale}: reasons are localized without losing codepoint details`, () => {
  setLocale(locale);
  assert.equal(ui.textFitReason("missing_glyph: U+00E9", t), `${t("textFit.reason.glyph")}: U+00E9`);
  assert.equal(ui.textFitReason("backslash_control", t), t("textFit.reason.backslash"));
  assert.equal(ui.textFitReason("backslash_control: any backslash syntax is unsupported", t), t("textFit.reason.backslash"));
  assert.equal(ui.textFitReason("no_text", t), t("textFit.noText"));
  assert.equal(ui.textFitReason(null, t), t("textFit.unknownReason"));
});

test("es: over-limit request errors localize HTTP and native reasons while unknown diagnostics remain intact", async context => {
  setLocale("es"); ui.persistTextFitProfile("project-a", validProfile); expand();
  for (const [index, message] of ["input_limit: source and draft exceed 16384 scalars", "400: input_limit: source and draft exceed 16384 scalars"].entries()) {
    const draft = "a".repeat(16_385 + index); component("project-a", row, draft); context.mock.timers.tick(250);
    h.requests[index].reject(Error(message)); await Promise.resolve(); await Promise.resolve();
    const html = renderToStaticMarkup(component("project-a", row, draft));
    assert.ok(html.includes(t("textFit.reason.inputLimit"))); assert.ok(html.includes(t("textFit.unavailable")));
    assert.ok(!html.includes("input_limit")); assert.ok(!html.includes("source and draft exceed")); assert.ok(!html.includes("400:"));
  }
  for (const diagnostic of ["Network unavailable", "400: unrelated backend diagnostic"]) assert.equal(ui.textFitRequestError(diagnostic, t), diagnostic);
});

test("invalid font errors remain unavailable even for empty text; project change uses its own blank profile", async context => {
  ui.persistTextFitProfile("project-a", validProfile); expand(); component("project-a", { ...row, source: "" }, ""); context.mock.timers.tick(250);
  await finish(0, { source: [{ status: "unavailable", advance_px: null, usable_width_px: 30, reason: "invalid font" }], draft: [{ status: "unavailable", advance_px: null, usable_width_px: 30, reason: "invalid font" }] });
  const html = renderToStaticMarkup(component("project-a", { ...row, source: "" }, ""));
  assert.ok(html.includes("invalid font")); assert.ok(!html.includes(t("textFit.noText"))); assert.ok(!html.includes("0 px"));
  h.project = { key: "project-b" };
  h.confirmed = "project-b";
  const changed = component("project-b"); assert.ok(renderToStaticMarkup(changed).includes(t("textFit.configure")));
  assert.equal(nodes(changed).find(node => node.props?.id?.endsWith("-font")).props.value, "");
  h.project = null; assert.equal(component("project-b"), null);
});

test("desktop picker selects only TTF/OTF; browser retains editable path without a picker", async () => {
  expand(); h.pickerValue = "chosen.otf";
  const browse = nodes(component()).find(node => node.type === "button" && node.props.children === t("textFit.browse"));
  await browse.props.onClick(); assert.equal(control("-font").props.value, "chosen.otf");
  assert.deepEqual(h.pickerCalls, [{ multiple: false, filters: [{ name: "TTF / OTF", extensions: ["ttf", "otf"] }] }]);
  h.setTauri(false); const html = markup(); assert.ok(!html.includes(t("textFit.browse"))); assert.match(html, /value="chosen.otf"/);
});

test("entering width estimate controls leaves the actual editor draft unsaved and unchanged", async () => {
  const toggle = nodes(component()).find(node => node.type === "button" && node.props["aria-expanded"] === false);
  expand(); const section = component();
  assert.ok(Object.prototype.hasOwnProperty.call(section!.props, "data-text-fit-preview"));
  const controls = [toggle, ...["-font", "-size", "-width"].map(suffix => nodes(section).find(node => node.props?.id?.endsWith(suffix)))];
  h.cleanups.forEach(fn => fn?.()); h.slots = []; h.effects = []; h.cleanups = [];
  const tree = unsavedDetail(), textarea = nodes(tree).find(node => node.type === "textarea");
  assert.equal(nodes(tree).find(node => node.type === ui.default).props.draft, "Unsaved Latin draft");
  const before = JSON.stringify(drafts.useDraftStore.getState().drafts[detailDraftKey]);
  for (const control of controls) {
    assert.ok(control);
    await textarea.props.onBlur({ relatedTarget: { closest(selector: string) {
      assert.equal(selector, "[data-text-fit-preview]"); return section;
    } } });
  }
  assert.equal(h.writes.length, 0); assert.equal(h.server, row.translation);
  assert.equal(JSON.stringify(drafts.useDraftStore.getState().drafts[detailDraftKey]), before);
});

for (const destination of ["outside", "null", "no event"] as const) test(`ordinary editor blur (${destination}) still saves`, async () => {
  const textarea = nodes(unsavedDetail()).find(node => node.type === "textarea");
  const event = destination === "no event" ? undefined : { relatedTarget: destination === "null" ? null : { closest: () => null } };
  await textarea.props.onBlur(event);
  assert.deepEqual(h.writes, [{ translation: "Unsaved Latin draft", expected_translation: row.translation }]);
  assert.equal(h.server, "Unsaved Latin draft");
});

test("explicit Ctrl+Enter in the editor still saves", async () => {
  const textarea = nodes(unsavedDetail()).find(node => node.type === "textarea");
  textarea.props.onKeyDown({ key: "Enter", ctrlKey: true }); await Promise.resolve(); await Promise.resolve();
  assert.deepEqual(h.writes, [{ translation: "Unsaved Latin draft", expected_translation: row.translation }]);
  assert.equal(h.server, "Unsaved Latin draft");
});

for (const tauri of [true, false]) test(`${tauri ? "Tauri" : "HTTP"} API preserves the read-only request/response contract and HTTP abort signal`, async () => {
  h.setTauri(true); globalThis.fetch = async () => new Response("{}"); await api.getCurrentProject(); h.setTauri(tauri);
  globalThis.fetch = async (url, init) => {
    h.transport.push([String(url), init?.method, JSON.parse(String(init?.body)), init?.signal]);
    return new Response(JSON.stringify({ source: [], draft: [] }));
  };
  const controller = new AbortController(); const request = ui.textFitRequest(row.id, "typed\n", validProfile)!;
  assert.deepEqual(await api.estimateTextFit(request, controller.signal), { source: [], draft: [] });
  assert.deepEqual(h.transport, [tauri ? ["text_fit", { request }] : ["http://localhost:7842/api/text-fit", "POST", request, controller.signal]]);
});
