import assert from "node:assert/strict";
import { after, beforeEach, test } from "node:test";
import { registerHooks } from "node:module";
import { setLocale, t } from "../lib/i18n";
import * as a11y from "../lib/modalA11y";

const h = { t, a11y };
(globalThis as any).__hotkeyHelpTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    let source: string | undefined;
    if (context.parentURL?.endsWith("/components/HotkeyHelp.tsx")) {
      const stubs: Record<string, string> = {
        "lucide-react": "export const X=()=>null;",
        "../lib/i18n": "export const useT=()=>h.t;",
        "../lib/modalA11y": "export const useModalA11y=()=>({dialogRef:{current:null},dialogProps:h.a11y.buildModalDialogProps({titleId:'title'}),titleProps:h.a11y.buildModalTitleProps('title')});export const MODAL_BACKDROP_CLASS=h.a11y.MODAL_BACKDROP_CLASS;export const MODAL_HEADER_CLASS=h.a11y.MODAL_HEADER_CLASS;export const MODAL_BODY_CLASS=h.a11y.MODAL_BODY_CLASS;export const MODAL_FOOTER_CLASS=h.a11y.MODAL_FOOTER_CLASS;export const modalPanelClass=h.a11y.modalPanelClass;",
      };
      source = stubs[specifier];
    }
    if (context.parentURL?.endsWith("/lib/hotkeys.ts")) {
      const stubs: Record<string, string> = {
        react: "export const useRef=value=>({current:value});export const useEffect=()=>{};",
        "react-router-dom": "export const useNavigate=()=>()=>{};",
        "@tanstack/react-query": "export const useQueryClient=()=>({});",
        "./openProjectFlow": "export const completeOpenProject=async()=>null;export const isDetectionFailure=()=>false;export const pickGameSource=async()=>null;",
        "./projectOpenMerge": "export const PENDING_AFTER_STALE_FILTER={};export const projectOpenMergeNotice=()=>({});export const shouldFocusPendingAfterOpen=()=>false;",
        "../stores/projectStore": "export const useProjectStore=select=>select({setProject(){}});",
        "../stores/editorStore": "export const useEditorStore=select=>select({setFilter(){}});",
        "../stores/logStore": "export const addLog=()=>{};",
        "../stores/toastStore": "export const addToast=()=>{};",
      };
      source = stubs[specifier];
    }
    if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("const h=globalThis.__hotkeyHelpTest;" + source)}` };
    return nextResolve(specifier, context);
  },
});
const { default: HotkeyHelp } = await import("./HotkeyHelp");
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__hotkeyHelpTest"); setLocale("en"); });
beforeEach(() => setLocale("en"));

function nodes(tree: any): any[] {
  if (tree == null || typeof tree !== "object") return [];
  if (Array.isArray(tree)) return tree.flatMap(nodes);
  return [tree, ...nodes(tree.props?.children)];
}
function text(tree: any): string {
  if (tree == null || typeof tree === "boolean") return "";
  if (typeof tree === "string" || typeof tree === "number") return String(tree);
  if (Array.isArray(tree)) return tree.map(text).join("");
  return text(tree.props?.children);
}
function render() { return HotkeyHelp({ open: true, onClose: () => {} }); }
const children = (node: any): any[] => [node?.props?.children].flat();
// Each group is <div><h3>label</h3><div>{rows: <div><span>action</span><kbd>key</kbd></div>}</div></div>.
function group(label: string): [string, string][] {
  const section = nodes(render()).find(node => node.type === "div"
    && children(node).some(child => child?.type === "h3" && text(child) === label));
  assert.ok(section, `group exists: ${label}`);
  const list = children(section).find(child => child?.type === "div");
  return children(list).map(row => {
    const [action, key] = children(row);
    return [text(action), text(key)] as [string, string];
  });
}

for (const locale of ["en", "es"] as const) {
  test(`${locale}: the icon-only close button has an accessible name`, () => {
    setLocale(locale);
    const close = nodes(render()).find(node => node.type === "button");
    assert.ok(close);
    assert.equal(close.props["aria-label"], t("common.close"));
    assert.equal(text(close), "");
  });

  test(`${locale}: Navigation lists Alt+1..Alt+8 in sidebar order, including QA and Speakers`, () => {
    setLocale(locale);
    const alt = group(t("hotkeys.group.Navigation")).filter(([, key]) => key.startsWith("Alt+"));
    assert.deepEqual(alt, [
      [t("hotkeys.navHome"), "Alt+1"],
      [t("hotkeys.navDashboard"), "Alt+2"],
      [t("hotkeys.navEditor"), "Alt+3"],
      [t("hotkeys.navReview"), "Alt+4"],
      [t("hotkeys.navQa" as any), "Alt+5"],
      [t("hotkeys.navSpeakers" as any), "Alt+6"],
      [t("hotkeys.navMemory"), "Alt+7"],
      [t("hotkeys.navSettings"), "Alt+8"],
    ]);
  });

  test(`${locale}: the Review group lists the Review keys`, () => {
    setLocale(locale);
    assert.deepEqual(group(t("hotkeys.group.Review")), [
      [t("review.approve"), "A"],
      [t("review.approve"), "Ctrl+Enter"],
      [t("review.skip"), "S"],
      [t("review.previous"), "P"],
      [t("hotkeys.reviewEdit" as any), "E"],
      [t("review.returnToEditor"), "Esc"],
    ]);
  });
}

test("Spanish labels for the new navigation and Review entries", () => {
  setLocale("es");
  assert.equal(t("hotkeys.navQa" as any), `Ir a ${t("nav.qa" as any)}`);
  assert.equal(t("hotkeys.navSpeakers" as any), "Ir a Personajes");
  assert.equal(t("hotkeys.reviewEdit" as any), "Editar traducción");
});
