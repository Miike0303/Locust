import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { after, beforeEach, test } from "node:test";
import type { StringEntry } from "../lib/api.ts";
import { setLocale, t } from "../lib/i18n/index.ts";

type Patch = Partial<Pick<StringEntry, "translation" | "status">>;
type Effect = { deps?: unknown[]; cleanup?: () => void };
const row = (id: string): StringEntry => ({
  id, source: `Source ${id}`, translation: `Translation ${id}`, status: "translated",
  file_path: "dialogue.json", context: null, tags: [], provider_used: null,
  char_limit: null, created_at: "", translated_at: null, reviewed_at: null,
});

// Follow the existing component tests' hook seam, but run dependency-aware
// effects as well so navigation exercises the real textarea reset behavior.
const h = {
  values: [] as any[], cursor: 0, dirty: false,
  effects: [] as (() => void)[], pending: [] as Promise<void>[], errors: [] as unknown[],
  rows: {} as Record<string, StringEntry>, writes: [] as { id: string; patch: Patch }[],
  failure: null as unknown, waitForSave: null as Promise<void> | null, invalidations: 0,
  t,
  state(initial: any) {
    const index = this.cursor++;
    if (!(index in this.values)) this.values[index] = typeof initial === "function" ? initial() : initial;
    return [this.values[index], (value: any) => {
      const next = typeof value === "function" ? value(this.values[index]) : value;
      if (!Object.is(next, this.values[index])) this.dirty = true;
      this.values[index] = next;
    }];
  },
  ref(initial: unknown) { return this.state(() => ({ current: initial }))[0]; },
  effect(callback: () => void | (() => void), deps?: unknown[]) {
    const index = this.cursor++;
    const previous = this.values[index] as Effect | undefined;
    if (previous && deps && previous.deps?.length === deps.length &&
      deps.every((dep, i) => Object.is(dep, previous.deps![i]))) return;
    const effect: Effect = { deps };
    this.values[index] = effect;
    this.effects.push(() => { previous?.cleanup?.(); effect.cleanup = callback() || undefined; });
  },
  callback(callback: (...args: any[]) => any, deps: unknown[]) {
    const index = this.cursor++;
    const previous = this.values[index];
    if (previous && deps.every((dep, i) => Object.is(dep, previous.deps[i]))) return previous.callback;
    const wrapped = (...args: any[]) => {
      const result = callback(...args);
      if (result instanceof Promise) {
        this.pending.push(result.then(() => {}, error => { this.errors.push(error); }));
      }
      return result;
    };
    this.values[index] = { deps, callback: wrapped };
    return wrapped;
  },
  async getStrings({ status }: { status: string }) {
    const entries = Object.values(this.rows).filter(entry => entry.status === status);
    return { entries, total: entries.length };
  },
  async patchString(id: string, patch: Patch) {
    this.writes.push({ id, patch });
    if (this.waitForSave) await this.waitForSave;
    if (this.failure !== null) throw this.failure;
    this.rows[id] = { ...this.rows[id], ...patch };
    return this.rows[id];
  },
};
(globalThis as any).__reviewTest = h;
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/pages/Review.tsx")) {
      const stubs: Record<string, string> = {
        react: `const h=globalThis.__reviewTest;export const useState=x=>h.state(x);export const useRef=x=>h.ref(x);export const useEffect=(fn,deps)=>h.effect(fn,deps);export const useCallback=(fn,deps)=>h.callback(fn,deps);`,
        "react-router-dom": `export const useNavigate=()=>()=>{};`,
        "@tanstack/react-query": `export const useQueryClient=()=>({invalidateQueries:async()=>{globalThis.__reviewTest.invalidations++;}});`,
        "../stores/projectStore": `export const useProjectStore=select=>select({project:{path:'/game'}});`,
        "../lib/i18n": `export const useT=()=>globalThis.__reviewTest.t;`,
        "../lib/api": `const h=globalThis.__reviewTest;export const getStrings=filter=>h.getStrings(filter);export const patchString=(id,patch)=>h.patchString(id,patch);`,
      };
      if (stubs[specifier]) return { url: `data:text/javascript,${encodeURIComponent(stubs[specifier])}`, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
});
const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
Object.defineProperty(globalThis, "window", {
  configurable: true, value: { addEventListener() {}, removeEventListener() {} },
});
const { default: Review, reviewApprovalPatch } = await import("./Review.tsx");
after(() => {
  hooks.deregister();
  if (previousWindow) Object.defineProperty(globalThis, "window", previousWindow);
  else Reflect.deleteProperty(globalThis, "window");
  Reflect.deleteProperty(globalThis, "__reviewTest");
  setLocale("en");
});

function render() {
  for (let attempt = 0; attempt < 20; attempt++) {
    h.cursor = 0;
    h.dirty = false;
    const tree = Review();
    for (const effect of h.effects.splice(0)) effect();
    if (!h.dirty) return tree;
  }
  throw new Error("Review did not settle");
}
function nodes(node: any): any[] {
  if (!node || typeof node !== "object") return [];
  if (Array.isArray(node)) return node.flatMap(nodes);
  return [node, ...nodes(node.props?.children)];
}
function text(node: any): string {
  if (typeof node === "string") return node;
  if (Array.isArray(node)) return node.map(text).join("");
  return node && typeof node === "object" ? text(node.props?.children) : "";
}
function button(label: string) {
  const found = nodes(render()).find(node => node.type === "button" && text(node).trim().startsWith(label));
  assert.ok(found, `missing action: ${label}`);
  return found;
}
function textarea() { return nodes(render()).find(node => node.type === "textarea"); }
function edit(value: string) { textarea().props.onChange({ target: { value } }); }
function progress(current: number, approved: number) {
  assert.ok(text(render()).includes(t("review.progress", { current, total: 2, approved })));
}
async function settle() {
  do { await Promise.all(h.pending.splice(0)); render(); } while (h.pending.length);
}
async function approve() {
  button(t("review.approve")).props.onClick();
  await settle();
}
beforeEach(async () => {
  setLocale("en");
  h.values = []; h.effects = []; h.pending = []; h.errors = [];
  h.rows = { A: row("A"), B: row("B") }; h.writes = [];
  h.failure = null; h.waitForSave = null; h.invalidations = 0;
  render();
  await settle();
  assert.equal(textarea().props.value, "Translation A");
});

test("the approval decision preserves approved status and distinguishes cleared text from no change", () => {
  assert.deepEqual(reviewApprovalPatch(row("A"), "Translation A"), { status: "approved" });
  assert.deepEqual(reviewApprovalPatch({ ...row("A"), status: "reviewed" }, "Translation A"), { status: "approved" });
  const approved = { ...row("A"), status: "approved" as const };
  assert.equal(reviewApprovalPatch(approved, "Translation A"), null);
  assert.deepEqual(reviewApprovalPatch(approved, "Revised A"), { translation: "Revised A", status: "approved" });
  assert.deepEqual(reviewApprovalPatch(approved, ""), { translation: "", status: "approved" });
  assert.equal(reviewApprovalPatch({ ...approved, translation: null }, ""), null);
});

test("approve A, previous, edit and approve persists the revision and counts A once", async () => {
  await approve();
  progress(2, 1);
  button(t("review.previous")).props.onClick();
  edit("Revised A");
  await approve();
  assert.equal(h.rows.A.translation, "Revised A");
  assert.equal(h.rows.A.status, "approved");
  progress(2, 1);
  button(t("review.previous")).props.onClick();
  assert.equal(textarea().props.value, "Revised A", "the local cache must retain the saved revision");
});

test("approving an unchanged previously approved row advances without a write", async () => {
  await approve();
  button(t("review.previous")).props.onClick();
  h.writes = [];
  await approve();
  assert.deepEqual(h.writes, []);
  progress(2, 1);
});

for (const revisited of [false, true]) {
  test(`${revisited ? "repeated" : "first"} approval failure shows the error, stays on A and keeps the edit for retry`, async () => {
    if (revisited) {
      await approve();
      button(t("review.previous")).props.onClick();
    }
    edit("Unsaved A");
    h.failure = revisited ? "Cannot save revision" : new Error("Cannot save revision");
    const invalidations = h.invalidations;
    await approve();
    assert.equal(textarea().props.value, "Unsaved A");
    progress(1, revisited ? 1 : 0);
    assert.equal(h.rows.A.translation, "Translation A");
    assert.equal(h.invalidations, invalidations);
    assert.ok(text(render()).includes("Cannot save revision"), "save errors must be visible alongside the edit");
    assert.deepEqual(h.errors, [], "approval must handle save rejections");
    h.failure = null;
    await approve();
    assert.equal(h.rows.A.translation, "Unsaved A");
    assert.equal(h.rows.A.status, "approved");
    progress(2, 1);
    assert.ok(!text(render()).includes("Cannot save revision"), "successful retry clears the error");
  });
}

test("a revised approval waits for persistence before advancing", async () => {
  await approve();
  button(t("review.previous")).props.onClick();
  edit("Delayed A");
  let finish!: () => void;
  h.waitForSave = new Promise<void>(resolve => { finish = resolve; });
  button(t("review.approve")).props.onClick();
  try {
    progress(1, 1);
    assert.equal(textarea().props.value, "Delayed A");
    assert.equal(h.rows.A.translation, "Translation A");
  } finally {
    finish();
    await settle();
  }
  assert.equal(h.rows.A.translation, "Delayed A");
  progress(2, 1);
});
