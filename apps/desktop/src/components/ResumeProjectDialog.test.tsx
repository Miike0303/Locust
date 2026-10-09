import assert from "node:assert/strict";
import { test } from "node:test";
import { registerHooks } from "node:module";
import { renderToStaticMarkup } from "react-dom/server";
import { setLocale, t } from "../lib/i18n/index.ts";
import * as a11y from "../lib/modalA11y.ts";
import type { ProjectOpenChoice, ProjectOpenChoiceRequest } from "../lib/openProjectFlow.ts";

// Capture the dialog's a11y configuration and element callbacks without a DOM.
// The focus trap/Escape implementation itself is covered by modalA11y.test.ts.
const h = { t, a11y, modal: null as any, setTauri: (_value: boolean) => {} };
(globalThis as any).__resumeDialog = h;
registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source: `
        export let IS_TAURI=true;
        export const isTauri=()=>IS_TAURI;
        globalThis.__resumeDialog.setTauri=value=>{IS_TAURI=value;};
      ` };
    }
    return nextLoad(url, context);
  },
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/ResumeProjectDialog.tsx")) {
      const stubs: Record<string, string> = {
        react: `export const useRef=()=>({current:null});export const useId=()=> 'description';export const useEffect=()=>{};export const useSyncExternalStore=()=>null;`,
        "../lib/i18n": `export const useT=()=>globalThis.__resumeDialog.t;`,
        "../lib/modalA11y": `const h=globalThis.__resumeDialog;
          export const MODAL_BACKDROP_CLASS=h.a11y.MODAL_BACKDROP_CLASS;
          export const MODAL_FOOTER_CLASS=h.a11y.MODAL_FOOTER_CLASS;
          export const modalPanelClass=h.a11y.modalPanelClass;
          export function useModalA11y(options){h.modal=options;return {dialogRef:{current:null},dialogProps:h.a11y.buildModalDialogProps({titleId:'title'}),titleProps:h.a11y.buildModalTitleProps('title')};}`,
      };
      if (stubs[specifier]) return { url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
});
const { default: Dialog } = await import("./ResumeProjectDialog.tsx");
function elements(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(elements);
  return [node, ...elements(node.props?.children)];
}

for (const locale of ["en", "es"] as const) {
  for (const verified of [true, false]) {
    test(`${locale} ${verified ? "verified" : "unverified"} dialog exposes explicit choices and safe initial focus`, () => {
      setLocale(locale);
      const chosen: ProjectOpenChoice[] = [];
      const request: ProjectOpenChoiceRequest = {
        gamePath: "/games/title",
        preflight: verified
          ? { kind: "resume_available", database_path: "/games/title.locust.db", project_path: "/games/title", format_id: "renpy" }
          : { kind: "needs_attention", reason: "Physical member drift <unsafe>" },
      };
      const tree = Dialog({ request, onChoose: choice => chosen.push(choice) });
      const nodes = elements(tree);
      const buttons = nodes.filter(node => node.type === "button");
      const cancel = buttons.find(node => node.props.children === t("common.cancel"));
      const refresh = buttons.find(node => node.props.children === t("resume.refresh"));
      const resume = buttons.find(node => node.props.children === t("resume.resume"));
      assert.ok(cancel && refresh);
      assert.equal(!!resume, verified);
      assert.equal(h.modal.open, true);
      assert.equal(h.modal.ownEscape, true);
      assert.equal(h.modal.initialFocusRef, (verified ? resume : cancel).props.ref);
      const panel = nodes.find(node => node.props.role === "dialog");
      assert.equal(panel.props["aria-modal"], true);
      assert.equal(panel.props["aria-describedby"], "description");
      const markup = renderToStaticMarkup(tree);
      assert.ok(markup.includes(t("resume.refreshWarning")));
      assert.ok(markup.includes("/games/title"));
      if (!verified) assert.ok(markup.includes("Physical member drift &lt;unsafe&gt;"));
      cancel.props.onClick();
      h.modal.onClose(); // Escape uses the same cancellation callback.
      tree!.props.onClick(); // Backdrop also cancels.
      refresh.props.onClick();
      if (verified) resume.props.onClick();
      assert.deepEqual(chosen, verified
        ? ["cancel", "cancel", "cancel", "refresh", "resume"]
        : ["cancel", "cancel", "cancel", "refresh"]);
    });
  }
}

test("closed dialog renders nothing and does not claim Escape", () => {
  assert.equal(Dialog({ request: null, onChoose() {} }), null);
  assert.equal(h.modal.open, false);
});

test("real frontend clients serialize the same preflight/resume contract on Tauri and HTTP", async () => {
  const calls: unknown[][] = [];
  const preflight = { kind: "resume_available", database_path: "/game.locust.db", project_path: "/game", format_id: "renpy" };
  const opened = { project_path: "/game", project_name: "game", format_id: "renpy", total_strings: 1, added: 0 };
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const previousFetch = globalThis.fetch;
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    __TAURI_INTERNALS__: { invoke: async (command: string, args: unknown) => {
      calls.push([command, args]);
      if (command === "get_server_port") return 7842;
      return command === "preflight_project_open" ? preflight : opened;
    } },
  } });
  globalThis.fetch = async (url, init) => {
    calls.push([String(url), init?.method, init?.body ? JSON.parse(String(init.body)) : null]);
    return new Response(JSON.stringify(String(url).endsWith("/preflight") ? preflight : opened));
  };
  try {
    const api = await import("../lib/api.ts");
    // Resolve the test server URL through the desktop port seam; subsequent
    // browser transport calls exercise real fetch serialization without Vite.
    h.setTauri(true);
    await api.getCurrentProject();
    calls.length = 0;
    assert.deepEqual(await api.preflightProjectOpen("/selected", "renpy"), preflight);
    assert.deepEqual(await api.resumeProject("/game.locust.db", "/game", "renpy"), opened);
    assert.deepEqual(calls, [
      ["preflight_project_open", { gamePath: "/selected", format: "renpy" }],
      ["resume_project", { databasePath: "/game.locust.db", gamePath: "/game", formatId: "renpy" }],
    ]);
    calls.length = 0;
    h.setTauri(false);
    assert.deepEqual(await api.preflightProjectOpen("/selected", "renpy"), preflight);
    assert.deepEqual(await api.resumeProject("/game.locust.db", "/game", "renpy"), opened);
    assert.deepEqual(calls, [
      ["http://localhost:7842/api/project/preflight", "POST", { game_path: "/selected", format: "renpy" }],
      ["http://localhost:7842/api/project/resume", "POST", { database_path: "/game.locust.db", game_path: "/game", format_id: "renpy" }],
    ]);
  } finally {
    globalThis.fetch = previousFetch;
    if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow);
    else Reflect.deleteProperty(globalThis, "window");
    h.setTauri(true);
    setLocale("en");
  }
});
