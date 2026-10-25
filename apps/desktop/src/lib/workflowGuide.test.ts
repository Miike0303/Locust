import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { after, test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { MutationObserver, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { WORKFLOW_STEP_LABELS, resolveWorkflowCheckState, resolveWorkflowGuideStep, resolveWorkflowGuideTarget, type WorkflowCheckState } from "./workflowGuide.ts";
import WorkflowGuideBanner from "../components/WorkflowGuideBanner.tsx";
import { setLocale, t } from "./i18n/index.ts";
import { qaLastRunKey } from "./qaDashboard.ts";
import { draftProjectKey } from "../stores/draftStore.ts";
import { useProjectStore } from "../stores/projectStore.ts";
import { queryClient } from "./queryClient.ts";
import { useEditorStore } from "../stores/editorStore.ts";

const reviewHarness = {
  t, values: [] as any[], cursor: 0, effects: [] as (() => void)[], dirty: false,
  state(initial: any) {
    const index = this.cursor++;
    if (!(index in this.values)) this.values[index] = typeof initial === "function" ? initial() : initial;
    return [this.values[index], (next: any) => {
      const value = typeof next === "function" ? next(this.values[index]) : next;
      if (!Object.is(value, this.values[index])) this.dirty = true;
      this.values[index] = value;
    }];
  },
  effect(fn: () => void, deps: unknown[]) {
    const index = this.cursor++;
    const previous = this.values[index];
    if (previous && deps.every((dep, i) => Object.is(dep, previous[i]))) return;
    this.values[index] = deps;
    this.effects.push(fn);
  },
};
(globalThis as any).__workflowReview = reviewHarness;
const harness = { refresh: undefined as (() => Promise<void>) | undefined, action: undefined as (() => void) | undefined, target: "" };
(globalThis as any).__workflowGuideTest = harness;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/pages/Review.tsx")) {
      const actual = nextResolve(specifier, context).url;
      const sources: Record<string, string> = {
        react: `export * from ${JSON.stringify(actual)};const h=globalThis.__workflowReview;export const useState=x=>h.state(x);export const useRef=x=>h.state(()=>({current:x}))[0];export const useEffect=(fn,deps)=>h.effect(fn,deps);export const useCallback=fn=>fn;`,
        "@tanstack/react-query": `import {queryClient} from ${JSON.stringify(nextResolve("../lib/queryClient", context).url)};export const useQueryClient=()=>queryClient;`,
        "react-router-dom": `export const useNavigate=()=>()=>{};`,
        "../lib/i18n": `export const useT=()=>globalThis.__workflowReview.t;`,
        "../lib/api": `export * from ${JSON.stringify(actual)};export const getStrings=async filter=>({entries:filter.status==='reviewed'?[]:[${JSON.stringify({id: "review-row", source: "Source", translation: "Before", status: "translated", file_path: "dialogue.json", tags: []})}],total:filter.status==='reviewed'?0:1});export const getSpeakers=async()=>({groups:[]});`,
      };
      if (sources[specifier]) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(sources[specifier])}` };
    }
    if (context.parentURL?.endsWith("/pages/Editor.tsx")) {
      if (specifier === "react-router-dom") {
        const actual = nextResolve(specifier, context).url;
        const source = `export * from ${JSON.stringify(actual)};export const useNavigate=()=>target=>{globalThis.__workflowGuideTest.target=target;};`;
        return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
      }
      if (specifier === "../components/StringTable" || specifier === "../components/WorkflowGuideBanner") {
        const actual = nextResolve(specifier, context).url;
        const assignment = specifier.endsWith("StringTable") ? "refresh=props.onRefetch" : "action=props.onPrimaryAction";
        const react = nextResolve("react", context).url;
        const source = `import {createElement} from ${JSON.stringify(react)};import Component from ${JSON.stringify(actual)};export default props=>{globalThis.__workflowGuideTest.${assignment};return createElement(Component,props);};`;
        return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
      }
    }
    if ((context.parentURL?.endsWith("/pages/Editor.tsx") || context.parentURL?.endsWith("/pages/Review.tsx")) && specifier === "../stores/projectStore") {
      // Zustand's server snapshot is its initial state; expose the seeded project to SSR.
      const actual = nextResolve(specifier, context).url;
      const source = `import {useProjectStore as store} from ${JSON.stringify(actual)};export const useProjectStore=Object.assign((select=s=>s)=>select(store.getState()),store);`;
      return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    }
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      const source = specifier === "./runtime" ? "export const IS_TAURI=true;"
        : specifier === "@tauri-apps/api/core" ? `export const invoke=async(cmd,args)=>cmd==='patch_string'?{id:args.id,...args.data}:cmd==='batch_patch_strings'?{requested:1,applied:1,skipped:0,conflicts:[]}:7842;` : null;
      if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: Editor } = await import("../pages/Editor.tsx");
const { default: Review } = await import("../pages/Review.tsx");
const { batchPatchStrings } = await import("./api.ts");
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__workflowGuideTest"); Reflect.deleteProperty(globalThis, "__workflowReview"); setLocale("en"); useProjectStore.setState({ project: null }); });

const stats = (pending = 0, translated = 0, reviewed = 0, approved = 0) => ({ pending, translated, reviewed, approved });
const base = { hasProject: true, stats: stats(0, 0, 0, 5), skipReview: false };
const unchecked = { summary: {}, lastCheckedAt: null, latestEditAt: null, isChecking: false };
const checked = { ...unchecked, lastCheckedAt: 100 };
for (const [name, context, expected] of [
  ["no project has no contextual step", { ...base, hasProject: false }, null],
  ["missing stats has no contextual step", { ...base, stats: null }, null],
  ["empty project has no contextual step", { ...base, stats: stats() }, null],
  ["pending work starts with translate", { ...base, stats: stats(5) }, "translate"],
  ["skip review does not bypass pending translation", { ...base, stats: stats(2, 3), skipReview: true }, "translate"],
  ["translated work continues to review", { ...base, stats: stats(0, 5) }, "review"],
  ["reviewed work stays in review until approved", { ...base, stats: stats(0, 0, 5) }, "review"],
  ["skip review sends translated work to check", { ...base, stats: stats(0, 5), skipReview: true }, "check"],
  ["skip review sends reviewed work to check", { ...base, stats: stats(0, 0, 5), skipReview: true }, "check"],
  ["approval alone never means validation success", base, "check"],
  ["mixed pending work keeps translate primary", { ...base, stats: stats(2, 3) }, "translate"],
] as const) test(name, () => assert.equal(resolveWorkflowGuideStep(context), expected));

for (const [name, context, expected] of [
  ["not checked without a completed run", unchecked, "not-checked"],
  ["zero-count summary is not validation evidence", { ...unchecked, summary: { Tags: { open: 0, ignored: 0 } } }, "not-checked"],
  ["ignored rows alone cannot establish a clean session run", { ...unchecked, summary: { Tags: { open: 0, ignored: 2 } } }, "not-checked"],
  ["pending validation is checking", { ...unchecked, isChecking: true }, "checking"],
  ["pending validation takes priority over old issues", { ...checked, isChecking: true, summary: { Tags: { open: 3, ignored: 0 } } }, "checking"],
  ["open issues require attention", { ...checked, summary: { Tags: { open: 2, ignored: 3 }, Length: { open: 1, ignored: 0 } } }, "issues"],
  ["persisted open issues still lead to QA", { ...unchecked, summary: { Tags: { open: 1, ignored: 0 } } }, "issues"],
  ["completed empty run is current", checked, "current"],
  ["ignored issues do not prevent a current run", { ...checked, summary: { Tags: { open: 0, ignored: 2 } } }, "current"],
  ["edit after validation makes a clean run not checked", { ...checked, latestEditAt: 101 }, "not-checked"],
  ["edit makes previous issues stale too", { ...checked, latestEditAt: 101, summary: { Tags: { open: 1, ignored: 0 } } }, "not-checked"],
  ["edits before a completed rerun remain current", { ...checked, latestEditAt: 99 }, "current"],
  ["missing summary cannot claim success", { ...checked, summary: undefined }, "not-checked"],
] as const) test(name, () => assert.equal(resolveWorkflowCheckState(context).state, expected));

test("Check counts only open issues across validation kinds", () => {
  assert.equal(resolveWorkflowCheckState({ ...checked, summary: { Tags: { open: 2, ignored: 3 }, Length: { open: 1, ignored: 4 } } }).openIssues, 3);
});
for (const state of ["not-checked", "checking", "issues", "current"] as const) {
  test(`Check ${state} selects the appropriate workflow step`, () => {
    assert.equal(resolveWorkflowGuideStep({ ...base, checkState: state }), state === "current" ? "write" : "check");
  });
}
test("workflow labels cover exactly Translate, Review, Check, Write", () => {
  assert.deepEqual(Object.values(WORKFLOW_STEP_LABELS), ["Translate", "Review", "Check", "Write"]);
});
test("CTA targets preserve dialogs and lead Review and Check to their pages", () => {
  assert.equal(resolveWorkflowGuideTarget("translate"), "translate-dialog");
  assert.equal(resolveWorkflowGuideTarget("review"), "/review");
  assert.equal(resolveWorkflowGuideTarget("check"), "/qa");
  assert.equal(resolveWorkflowGuideTarget("write"), "inject-dialog");
});
function banner(state: WorkflowCheckState, openIssues = 3) {
  return renderToStaticMarkup(createElement(WorkflowGuideBanner, {
    step: state === "current" ? "write" : "check", check: { state, openIssues }, onPrimaryAction() {}, onDismiss() {},
  }));
}
for (const [locale, labels, actions] of [
  ["en", ["Translate", "Review", "Check", "Write"], ["Check translations", "Review issues", "Write translations"]],
  ["es", ["Traducir", "Revisar", "Comprobar", "Escribir"], ["Comprobar traducciones", "Revisar problemas", "Escribir traducciones"]],
] as const) {
  test(`${locale}: four steps and not-checked CTA never mark approval as validation`, () => {
    setLocale(locale);
    const html = banner("not-checked");
    for (const label of labels) assert.ok(html.includes(label), label);
    assert.ok(html.includes(actions[0]));
    assert.ok(html.includes(t("workflow.notChecked")));
    assert.match(html, />4<\/span>/);
    assert.equal((html.match(/lucide-check"/g) ?? []).length, 2);
  });
  test(`${locale}: checking disables the progress CTA`, () => {
    setLocale(locale);
    assert.match(banner("checking"), /<button[^>]*disabled=""[^>]*>/);
    assert.ok(banner("checking").includes(t("workflow.checking")));
  });
  test(`${locale}: issues show warning dot, plural counts and Review issues CTA`, () => {
    setLocale(locale);
    const html = banner("issues");
    assert.match(html, /size-1.5 rounded-full bg-warning/);
    assert.ok(html.includes(locale === "en" ? "3 issues" : "3 problemas"));
    assert.ok(html.includes(actions[1]));
    assert.ok(banner("issues", 1).includes(locale === "en" ? "1 issue" : "1 problema"));
  });
  test(`${locale}: current Check is done and Write retains the Inject dialog target`, () => {
    setLocale(locale);
    const html = banner("current");
    assert.ok(html.includes(actions[2]));
    assert.match(html, /aria-current="step"[^>]*>[\s\S]*?>4<\/span>/);
    assert.equal((html.match(/lucide-check"/g) ?? []).length, 3);
    assert.equal(resolveWorkflowGuideTarget("write"), "inject-dialog");
  });
}
const project = { path: "demo", database_path: "demo.db", format_id: "html", name: "Demo" };
const projectKey = draftProjectKey(project);
function editor(client: QueryClient) {
  useProjectStore.setState({ project, confirmedOpenProjectKey: projectKey, projectTransitionInProgress: false });
  return renderToStaticMarkup(createElement(QueryClientProvider, { client }, createElement(MemoryRouter, {}, createElement(Editor))));
}
function editorClient() {
  const client = queryClient;
  client.clear();
  client.setDefaultOptions({ queries: { retry: false, staleTime: Infinity, gcTime: Infinity } });
  client.setQueryData(["stats", projectKey], { ...base.stats, total: 5, error: 0, total_cost_usd: 0 });
  client.setQueryData(["strings", useEditorStore.getState().filter, projectKey], { entries: [], total: 0, offset: 0, limit: 50 });
  client.setQueryData(["validation-summary", projectKey], {});
  return client;
}
test("Editor reuses QA completion and editor edit staleness survives remount", () => {
  setLocale("en");
  const client = editorClient();
  try {
    assert.ok(editor(client).includes("Check translations"));
    client.setQueryData(qaLastRunKey(projectKey), 100);
    assert.ok(editor(client).includes("Write translations"));
    client.setQueryData(["workflow-last-edit", projectKey], 101);
    assert.ok(editor(client).includes("Check translations"));
    client.setQueryData(qaLastRunKey(projectKey), 102);
    assert.ok(editor(client).includes("Write translations"));
    client.setQueryData(qaLastRunKey("another-database"), 200);
    client.removeQueries({ queryKey: qaLastRunKey(projectKey) });
    assert.ok(editor(client).includes("Check translations"));
  } finally { client.clear(); }
});
function renderReview() {
  for (let attempt = 0; attempt < 20; attempt++) {
    reviewHarness.cursor = 0;
    reviewHarness.dirty = false;
    const tree = Review();
    for (const effect of reviewHarness.effects.splice(0)) effect();
    if (!reviewHarness.dirty) return tree;
  }
  throw new Error("Review did not settle");
}
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
test("completed clean check -> save in Review -> remounted Editor requires Check translations", async () => {
  setLocale("en");
  const client = editorClient();
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", { configurable: true, value: { addEventListener() {}, removeEventListener() {} } });
  try {
    client.setQueryData(qaLastRunKey(projectKey), Date.now());
    assert.ok(editor(client).includes("Write translations"));
    reviewHarness.values = []; reviewHarness.effects = [];
    renderReview();
    await new Promise(resolve => setImmediate(resolve));
    const textarea = nodes(renderReview()).find(node => node.type === "textarea");
    assert.ok(textarea);
    textarea.props.onChange({ target: { value: "Saved in Review" } });
    const buttons = nodes(renderReview()).filter(node => node.type === "button");
    const approveButton = buttons.find(node => node.props.className?.includes("bg-accent"));
    assert.ok(approveButton, "Review approval action is present");
    approveButton.props.onClick();
    await new Promise(resolve => setImmediate(resolve));
    assert.ok((client.getQueryData<number>(["workflow-last-edit", projectKey]) ?? 0) > 0);
    const html = editor(client);
    assert.ok(html.includes("Check translations"));
    assert.ok(html.includes(t("workflow.notChecked")));
    assert.ok(!html.includes("Write translations"));
  } finally {
    client.clear();
    if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow);
    else Reflect.deleteProperty(globalThis, "window");
  }
});
test("Search/Replace shared bulk save stales a completed clean check", async () => {
  setLocale("en");
  const client = editorClient();
  try {
    client.setQueryData(qaLastRunKey(projectKey), Date.now());
    assert.ok(editor(client).includes("Write translations"));
    await batchPatchStrings([{ id: "row", translation: "Replacement" }]);
    assert.ok(editor(client).includes("Check translations"));
  } finally { client.clear(); }
});
for (const locale of ["en", "es"] as const) {
  test(`${locale}: Editor Check and issues CTA navigate to QA and Review navigates to Review`, () => {
    setLocale(locale);
    const client = editorClient();
    try {
      editor(client);
      harness.action!();
      assert.equal(harness.target, "/qa");
      client.setQueryData(["validation-summary", projectKey], { Tags: { open: 2, ignored: 0 } });
      editor(client);
      harness.action!();
      assert.equal(harness.target, "/qa");
      client.setQueryData(["stats", projectKey], { ...stats(0, 5), total: 5, error: 0, total_cost_usd: 0 });
      editor(client);
      harness.action!();
      assert.equal(harness.target, "/review");
    } finally { client.clear(); }
  });
}
test("Editor observes QA pending mutation and open issue summary", async () => {
  setLocale("en");
  const client = editorClient();
  let finish!: () => void;
  const run = new MutationObserver(client, {
    mutationKey: ["validation-run", projectKey],
    mutationFn: () => new Promise<void>(resolve => { finish = resolve; }),
  });
  try {
    const pending = run.mutate();
    await new Promise(resolve => setImmediate(resolve));
    assert.ok(editor(client).includes(t("workflow.checking")));
    finish();
    await pending;
    client.setQueryData(qaLastRunKey(projectKey), 100);
    client.setQueryData(["validation-summary", projectKey], { Tags: { open: 2, ignored: 1 } });
    assert.ok(editor(client).includes("Review issues"));
    assert.ok(editor(client).includes("2 issues"));
  } finally { finish?.(); client.clear(); }
});
test("Editor CTA dispatch and save callback retain existing modal wiring", () => {
  const source = readFileSync(new URL("../pages/Editor.tsx", import.meta.url), "utf8");
  assert.match(source, /workflowStep === "translate"\) setShowTranslateModal\(true\)/);
  assert.match(source, /workflowStep === "write"\) setShowInjectModal\(true\)/);
  assert.match(source, /navigate\(resolveWorkflowGuideTarget\(workflowStep\)/);
  assert.match(source, /<InjectModal\s+open=\{showInjectModal\}/);
});
