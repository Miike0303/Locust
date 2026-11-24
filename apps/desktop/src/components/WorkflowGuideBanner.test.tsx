import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, afterEach, test } from "node:test";
import { setLocale, t } from "../lib/i18n";
import type { WorkflowCheckState, WorkflowGuideStep } from "../lib/workflowGuide";

// Call the banner as a function and walk its element tree; only useT is stubbed.
(globalThis as any).__workflowBanner = { t };
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/components/WorkflowGuideBanner.tsx") && specifier === "../lib/i18n") {
      return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent("export const useT=()=>globalThis.__workflowBanner.t;")}` };
    }
    return nextResolve(specifier, context);
  },
});
const { default: WorkflowGuideBanner } = await import("./WorkflowGuideBanner");
afterEach(() => setLocale("en"));
after(() => { hooks.deregister(); Reflect.deleteProperty(globalThis, "__workflowBanner"); });

function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function text(node: any): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node !== "object") return String(node);
  return Array.isArray(node) ? node.map(text).join("") : text(node.props?.children);
}
function banner(step: WorkflowGuideStep, state: WorkflowCheckState, openIssues = 0) {
  return nodes(WorkflowGuideBanner({ step, check: { state, openIssues }, onPrimaryAction() {}, onDismiss() {} }));
}
const classes = (node: any) => String(node.props.className).split(/\s+/);

for (const locale of ["en", "es"] as const) {
  for (const [state, issues] of [["not-checked", 0], ["checking", 0], ["issues", 2]] as const) {
    test(`${locale}: Check step status (${state}) is a separate muted pill, not run into the label`, () => {
      setLocale(locale);
      const tree = banner("check", state, issues);
      const status = tree.find(node => node.props?.role === "status");
      assert.ok(status, "the check status is announced");
      const expected = state === "issues" ? t("workflow.issues", { count: issues })
        : t(state === "checking" ? "workflow.checking" : "workflow.notChecked");
      assert.equal(text(status), expected);
      const pill = classes(status);
      for (const name of ["rounded-full", "px-1.5", "font-normal"]) assert.ok(pill.includes(name), name);
      if (state === "issues") assert.ok(pill.includes("bg-warning-muted") && pill.includes("text-warning"), pill.join(" "));
      else assert.ok(pill.includes("bg-surface-muted") && pill.includes("text-text-muted"), pill.join(" "));
      // The label and the pill are distinct siblings of the current step.
      const step = tree.find(node => node.props?.["aria-current"] === "step");
      const children = [step.props.children].flat();
      assert.ok(children.includes(t("workflow.check")));
      assert.ok(children.includes(status));
    });
  }

  test(`${locale}: the description wraps instead of being the only truncated item`, () => {
    setLocale(locale);
    for (const step of ["translate", "review", "check", "write"] as const) {
      const tree = banner(step, "not-checked");
      const description = tree.find(node => node.type === "p");
      assert.equal(text(description), t(step === "check" ? "workflow.checkDesc" : `workflow.${step}Desc`));
      const names = classes(description);
      assert.ok(!names.includes("truncate") && !names.includes("whitespace-nowrap"), names.join(" "));
      assert.ok(names.includes("flex-1") && names.includes("min-w-64"), "moves to its own row before it gets narrower than 16rem");
    }
  });
}
