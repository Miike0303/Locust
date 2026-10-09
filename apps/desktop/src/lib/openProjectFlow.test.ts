/**
 * Deterministic orchestration and compatibility tests (npm run test:unit).
 */
import nodeAssert from "node:assert/strict";
import { test } from "node:test";
import { registerHooks } from "node:module";
import type { QueryClient } from "@tanstack/react-query";
import {
  completeOpenProject,
  completeOpenProjectDb,
  getProjectOpenChoice,
  requestProjectOpenChoice,
  resolveProjectOpenChoice,
  subscribeProjectOpenChoice,
  type ProjectOpenChoice,
  formatPickerPathFromState,
  isDetectionFailure,
  openDbCanConfirm,
  projectFromOpenResponse,
  projectOpenHttpBody,
  projectOpenTauriArgs,
  runOpenAction,
  shouldOpenProjectDb,
} from "./openProjectFlow.ts";

const assert = {
  equal(actual: unknown, expected: unknown, message?: string) {
    if (actual !== expected) throw new Error(message ?? `${actual} !== ${expected}`);
  },
  ok(cond: unknown, message?: string) {
    if (!cond) throw new Error(message ?? "expected truthy");
  },
};

assert.ok(isDetectionFailure("Could not detect game format"));
assert.ok(isDetectionFailure("format not detected"));
assert.equal(isDetectionFailure("path not found"), false);
assert.equal(isDetectionFailure("detect without the other word"), false);

assert.equal(formatPickerPathFromState(null), null);
assert.equal(formatPickerPathFromState({}), null);
assert.equal(formatPickerPathFromState({ formatPickerPath: "  " }), null);
assert.equal(
  formatPickerPathFromState({ formatPickerPath: "C:\\Games\\Title" }),
  "C:\\Games\\Title",
);

const info = projectFromOpenResponse({
  format_id: "renpy",
  format_name: "Ren'Py",
  total_strings: 3,
  project_path: "/games/title",
  project_name: "title",
  supported_modes: ["replace"],
  database_path: "/games/title.locust.db",
  added: 3,
  updated: 0,
  stale_source_reset: 0,
  removed: 0,
  preserved_translations: 0,
});
assert.equal(info.path, "/games/title");
assert.equal(info.format_id, "renpy");
assert.equal(info.name, "title");
assert.equal(info.database_path, "/games/title.locust.db");

assert.equal(shouldOpenProjectDb(undefined), false);
assert.equal(shouldOpenProjectDb(""), false);
assert.equal(shouldOpenProjectDb("/games/title"), false);
assert.equal(shouldOpenProjectDb("/games/title-pivot.locust.db"), true);
assert.equal(shouldOpenProjectDb("C:\\x\\a.locust.db"), true);
// Negative: treating a bare game path as open-db would re-extract pivots.
assert.equal(shouldOpenProjectDb("/games/title"), false);

assert.ok(openDbCanConfirm("/x/a.locust.db", "/games/title", "renpy"));
assert.equal(openDbCanConfirm("/x/a.locust.db", "/games/title", "auto"), false);
assert.equal(openDbCanConfirm("/x/a.locust.db", "  ", "renpy"), false);
assert.equal(openDbCanConfirm("/games/title", "/games/title", "renpy"), false);

const folderBody = projectOpenHttpBody("C:\\Games\\Title", "html-game");
assert.equal(folderBody.path, "C:\\Games\\Title");
assert.equal(folderBody.format_id, "html-game");
assert.equal("prefer_saved" in folderBody, false, "explicit folder must not prefer saved DB");
assert.equal(
  JSON.stringify(folderBody).includes("prefer_saved"),
  false,
);

const recentBody = projectOpenHttpBody("C:\\Games\\Title", "html-game", true);
assert.equal(recentBody.prefer_saved, true);
assert.equal(JSON.parse(JSON.stringify(recentBody)).prefer_saved, true);

const omittedFormat = projectOpenHttpBody("C:\\Games\\Title", undefined, true);
assert.equal(omittedFormat.prefer_saved, true);
assert.equal(
  Object.prototype.hasOwnProperty.call(JSON.parse(JSON.stringify(omittedFormat)), "format_id"),
  false,
);

const folderTauri = projectOpenTauriArgs("C:\\Games\\Title", "html-game");
assert.equal("preferSaved" in folderTauri, false, "Ctrl+O / Open Folder omit preferSaved");
const recentTauri = projectOpenTauriArgs("C:\\Games\\Title", "html-game", true);
assert.equal(recentTauri.preferSaved, true);

// A rejected native dialog resolves quietly and reports the failed-open message.
{
  const failures: string[] = [];
  const outcome = await runOpenAction(
    () => Promise.reject(new Error("dialog plugin unavailable")),
    (message) => failures.push(message),
  );
  assert.equal(outcome, undefined, "rejected dialog must resolve, not throw");
  assert.equal(failures.length, 1, "failed-open callback fires once");
  assert.equal(failures[0], "dialog plugin unavailable");

  const nonError: string[] = [];
  await runOpenAction(() => Promise.reject("denied"), (m) => nonError.push(m));
  assert.equal(nonError[0], "denied");

  let ran = false;
  const ok: string[] = [];
  await runOpenAction(async () => { ran = true; }, (m) => ok.push(m));
  assert.ok(ran, "action runs");
  assert.equal(ok.length, 0, "no failure callback on success");
}

console.log("openProjectFlow.test.ts: ok");

// Real orchestration with deterministic transport seams; no browser/backend needed.
const resumed = {
  kind: "resume_available", database_path: "/games/title.locust.db",
  project_path: "/games/title", format_id: "renpy",
};
const outcome = {
  format_id: "renpy", format_name: "Ren'Py", total_strings: 1,
  project_path: "/games/title", project_name: "title", supported_modes: ["replace"],
  database_path: "/games/title.locust.db", extraction_warnings: ["Saved warning"],
  added: 0, updated: 0, stale_source_reset: 0, removed: 0, preserved_translations: 0,
};
const h = {
  calls: [] as unknown[][],
  preflight: resumed as Record<string, unknown>,
  error: null as Error | null,
};
(globalThis as any).__resumeFlow = h;
registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/api.ts")) {
      return { format: "module", shortCircuit: true, source: `
        const h=globalThis.__resumeFlow;
        const outcome=${JSON.stringify(outcome)};
        export async function preflightProjectOpen(...args){h.calls.push(['preflight',...args]);if(h.error)throw h.error;return h.preflight;}
        export async function openProject(...args){h.calls.push(['extract',...args]);return outcome;}
        export async function resumeProject(...args){h.calls.push(['resume',...args]);if(h.error)throw h.error;return outcome;}
        export async function openProjectDb(...args){h.calls.push(['open-db',...args]);return outcome;}
      ` };
    }
    return nextLoad(url, context);
  },
});

function deps(choice: ProjectOpenChoice): Parameters<typeof completeOpenProject>[2] {
  return {
    setProject(p: unknown) { h.calls.push(["setProject", p]); },
    queryClient: { removeQueries(q: unknown) { h.calls.push(["removeQueries", q]); } } as unknown as QueryClient,
    async choose(request: unknown) { h.calls.push(["choose", request]); return choice; },
  };
}
function reset(preflight: Record<string, unknown> = resumed) {
  h.calls = []; h.preflight = preflight; h.error = null;
}

for (const choice of ["resume", "refresh", "cancel"] as const) {
  test(`folder preflight ${choice} uses only the explicitly selected path`, async () => {
    reset();
    const result = await completeOpenProject("/selected", undefined, deps(choice));
    nodeAssert.equal(h.calls[0][0], "preflight");
    nodeAssert.equal(h.calls[1][0], "choose");
    if (choice === "cancel") {
      nodeAssert.equal(result, null);
      nodeAssert.deepEqual(h.calls.map(c => c[0]), ["preflight", "choose"]);
    } else {
      nodeAssert.equal(h.calls[2][0], choice === "resume" ? "resume" : "extract");
      if (choice === "resume") nodeAssert.deepEqual(h.calls[2], ["resume", resumed.database_path, resumed.project_path, resumed.format_id]);
      else nodeAssert.deepEqual(h.calls[2], ["extract", "/selected", undefined]);
      nodeAssert.ok(h.calls.some(c => c[0] === "setProject"));
      nodeAssert.equal(h.calls.filter(c => c[0] === "removeQueries").length, 5);
      nodeAssert.deepEqual(result, outcome);
    }
  });
}

test("plain extract keeps ordinary folder behavior without prompting", async () => {
  reset({ kind: "extract" });
  await completeOpenProject("/selected", "renpy", deps("cancel"));
  nodeAssert.deepEqual(h.calls.slice(0, 2), [["preflight", "/selected", "renpy"], ["extract", "/selected", "renpy"]]);
  nodeAssert.ok(!h.calls.some(c => c[0] === "choose" || c[0] === "resume"));
});

for (const choice of ["refresh", "cancel", "resume"] as const) {
  test(`unverifiable evidence requires explicit refresh; ${choice} cannot silently resume`, async () => {
    reset({ kind: "needs_attention", reason: "Physical member drift" });
    const result = await completeOpenProject("/selected", undefined, deps(choice));
    nodeAssert.equal(h.calls[1][0], "choose");
    nodeAssert.ok(JSON.stringify(h.calls[1]).includes("Physical member drift"));
    nodeAssert.ok(!h.calls.some(c => c[0] === "resume"));
    if (choice === "refresh") nodeAssert.equal(h.calls[2][0], "extract");
    else {
      nodeAssert.equal(result, null);
      nodeAssert.equal(h.calls.length, 2);
    }
  });
}

test("legacy recent opening bypasses folder preflight", async () => {
  reset();
  await completeOpenProject("/selected", "renpy", deps("cancel"), true);
  nodeAssert.deepEqual(h.calls[0], ["extract", "/selected", "renpy", true]);
  nodeAssert.ok(!h.calls.some(c => c[0] === "preflight" || c[0] === "choose"));
});

test("failed preflight never falls through to extraction", async () => {
  reset(); h.error = new Error("Preflight unavailable");
  await nodeAssert.rejects(completeOpenProject("/selected", undefined, deps("resume")), /Preflight unavailable/);
  nodeAssert.deepEqual(h.calls.map(c => c[0]), ["preflight"]);
});

test("confirmation drift failure never falls back to extraction or changes project/query state", async () => {
  reset();
  const options = deps("resume");
  options.choose = async (request) => {
    h.calls.push(["choose", request]);
    h.error = new Error("Physical member drift");
    return "resume";
  };
  await nodeAssert.rejects(completeOpenProject("/selected", undefined, options), /Physical member drift/);
  nodeAssert.deepEqual(h.calls.map(c => c[0]), ["preflight", "choose", "resume"]);
});

test("unrecognized preflight never opens a project", async () => {
  reset({ kind: "future_or_corrupt_result" });
  await nodeAssert.rejects(completeOpenProject("/selected", undefined, deps("resume")));
  nodeAssert.deepEqual(h.calls.map(c => c[0]), ["preflight"]);
});

test("explicit saved DB opening bypasses preflight and choice", async () => {
  reset();
  await completeOpenProjectDb("/pivot.locust.db", "/selected", "renpy", deps("cancel"));
  nodeAssert.deepEqual(h.calls[0], ["open-db", "/pivot.locust.db", "/selected", "renpy"]);
  nodeAssert.ok(!h.calls.some(c => c[0] === "preflight" || c[0] === "choose" || c[0] === "extract"));
});

test("already aborted folder choice opens nothing", async () => {
  reset();
  const controller = new AbortController();
  controller.abort();
  nodeAssert.equal(await completeOpenProject("/selected", undefined, { ...deps("resume"), signal: controller.signal }), null);
  nodeAssert.deepEqual(h.calls, []);
});

test("choice broker preserves the pending owner and ignores stale callbacks", async () => {
  const request = { gamePath: "/selected", preflight: {
    kind: "needs_attention" as const, reason: "Unverified",
  } };
  let notifications = 0;
  const unsubscribe = subscribeProjectOpenChoice(() => { notifications++; });
  try {
    const first = requestProjectOpenChoice(request);
    const pending = getProjectOpenChoice();
    nodeAssert.ok(pending);
    nodeAssert.equal(await requestProjectOpenChoice(request), "cancel");
    nodeAssert.equal(getProjectOpenChoice(), pending);
    resolveProjectOpenChoice(pending, "cancel");
    nodeAssert.equal(await first, "cancel");
    const second = requestProjectOpenChoice(request);
    const current = getProjectOpenChoice();
    nodeAssert.ok(current);
    nodeAssert.notEqual(current.id, pending.id);
    resolveProjectOpenChoice(pending, "refresh");
    nodeAssert.equal(getProjectOpenChoice(), current);
    resolveProjectOpenChoice(current, "cancel");
    nodeAssert.equal(await second, "cancel");
    nodeAssert.equal(notifications, 4);
    nodeAssert.equal(getProjectOpenChoice(), null);
  } finally {
    unsubscribe();
  }
});
