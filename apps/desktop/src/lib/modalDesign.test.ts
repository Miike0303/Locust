import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { registerHooks } from "node:module";
import { createElement, type ComponentType } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import * as modal from "./modalA11y.ts";
import { t } from "./i18n/index.ts";
import { useQueueStore } from "../stores/queueStore.ts";
import { MODAL_BACKDROP_CLASS, MODAL_FOOTER_CLASS, modalPanelClass } from "./modalA11y.ts";

// Inspect every authored branch, including closed/conditional modal content.
// Guard each migrated modal's shared chrome and local styling.
const files = [
  "./modalA11y.ts",
  "../components/ConfirmDialog.tsx",
  "../components/ResumeProjectDialog.tsx",
  "../components/FontPatchDialog.tsx",
  "../components/InjectionRecoveryModal.tsx",
  "../components/ExportModal.tsx",
  "../components/PatchModal.tsx",
  "../components/InjectModal.tsx",
  "../components/PivotModal.tsx",
  "../components/SearchReplaceModal.tsx",
  "../components/HotkeyHelp.tsx",
  "../components/TranslationModal.tsx",
  "../components/ValidationResultsModal.tsx",
  "../components/QueuePanel.tsx",
] as const;

for (const file of files) {
  const sources: string[] = [file];
  if (file === "../components/PatchModal.tsx") {
    sources.push("../components/patch/PatchApplyTab.tsx", "../components/patch/PatchPackTab.tsx");
  }
  const source = sources.map(path => readFileSync(new URL(path, import.meta.url), "utf8")).join("\n");

  test(`${file} has no loose gray palette utilities`, () => {
    const literals = source.match(/[\w:/-]*gray-[\w/.-]+/g) ?? [];
    assert.equal(literals.length, 0, `${file} still uses loose grays: ${literals.join(", ")}`);
  });

  test(`${file} has no arbitrary pixel text sizes`, () => {
    const literals = source.match(/text-\[\d+(?:\.\d+)?px\]/g) ?? [];
    assert.equal(literals.length, 0, `${file} still uses pixel text sizes: ${literals.join(", ")}`);
  });
}

test("Inject language grid delegates scrolling to the modal without a mid-row height cap", () => {
  const source = readFileSync(new URL("../components/InjectModal.tsx", import.meta.url), "utf8");
  const grid = source.match(/className="([^"]*grid-cols-3[^"]*)"/);
  assert.ok(grid);
  assert.ok(!/\bmax-h-|\boverflow-y-/.test(grid[1]), "all language rows must fit inside the grid");
  assert.match(source, /MODAL_BODY_CLASS/);
});

test("shared panel bounds the frame and preserves caller layout extras", () => {
  const base = modalPanelClass();
  assert.deepEqual(new Set(base.split(/\s+/)), new Set([
    "bg-surface", "text-text", "border", "border-border", "rounded-lg", "shadow-xl", "w-full",
    "max-h-[calc(100vh-2rem)]", "flex", "flex-col", "my-4",
  ]));
  const extras = "max-w-2xl";
  assert.equal(modalPanelClass(extras), `${base} ${extras}`);
});

test("shared regions keep headers and wrapping actions fixed while the body scrolls", () => {
  assert.equal(modal.MODAL_HEADER_CLASS, "shrink-0");
  assert.equal(modal.MODAL_BODY_CLASS, "flex-1 min-h-0 overflow-y-auto");
  assert.equal(MODAL_FOOTER_CLASS, "shrink-0 flex flex-wrap justify-end gap-2 border-t border-border px-5 py-3");
});

test("shared backdrop retains its translucent black overlay", () => {
  assert.equal(MODAL_BACKDROP_CLASS, "fixed inset-0 bg-black/50 flex items-center justify-center z-50");
});

test("dialog layouts keep primary and secondary actions outside scrolling content", async (context) => {
  // Real SSR with store snapshots: Zustand SSR otherwise reads its initial closed state.
  const fixture = {
    project: { path: "/fixture", name: "Fixture", format_id: "rpgmaker-mz", supported_modes: ["replace", "add"] },
    queue: { ...useQueueStore.getState(), isPanelOpen: true },
  };
  (globalThis as any).__modalLayout = fixture;
  const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", { configurable: true, value: { addEventListener() { }, removeEventListener() { } } });
  const hooks = registerHooks({
    resolve(specifier, context, nextResolve) {
      if (/\/components\/[^/]+\.tsx$/.test(context.parentURL ?? "")) {
        const source = specifier === "../stores/projectStore"
          ? "export const useProjectStore=()=>({project:globalThis.__modalLayout.project});"
          : specifier === "../stores/queueStore"
            ? "export const useQueueStore=()=>globalThis.__modalLayout.queue;" : null;
        if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
      }
      return nextResolve(specifier, context);
    },
  });
  context.after(() => {
    hooks.deregister();
    Reflect.deleteProperty(globalThis, "__modalLayout");
    if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
    else Reflect.deleteProperty(globalThis, "window");
  });
  const noop = () => { };
  const cases = [
    ["FontPatchDialog", { open: true, defaultGamePath: "/fixture", onClose: noop, onUsePatch: noop }, ["fontPatch.create", "common.close"]],
    ["HotkeyHelp", { open: true, onClose: noop }, ["hotkeys.closeHint"]],
    ["InjectModal", { open: true, onClose: noop }, ["inject.injectAction", "inject.registerOnly"]],
    ["InjectionRecoveryModal", { defaultGamePath: "/fixture", onClose: noop }, ["recovery.inspect"]],
    ["PatchModal", { open: true, onClose: noop }, ["patch.applyBtn", "patch.verify"]],
    ["PatchModal", { open: true, onClose: noop, initialTab: "pack" }, ["patch.packBtn"]],
    ["QueuePanel", {}, ["queue.start"]],
    ["ResumeProjectDialog", { request: { gamePath: "/fixture", preflight: { kind: "resume_available" } }, onChoose: noop }, ["resume.resume", "resume.refresh", "common.cancel"]],
    ["ResumeProjectDialog", { request: { gamePath: "/fixture", preflight: { kind: "needs_attention", reason: "Long warning" } }, onChoose: noop }, ["resume.refresh", "common.cancel"]],
    ["ValidationResultsModal", { open: true, result: { validation: { issues: [], by_kind: {}, total_checked: 0 }, fonts: [] }, onClose: noop, onSelectEntry: noop, onReviewInEditor: noop }, ["validate.reviewInEditor", "common.close"]],
    ["PivotModal", { open: true, onClose: noop, carryOverCount: 1, totalEntries: 2 }, ["pivot.create", "common.cancel"]],
    ["ConfirmDialog", { open: true, title: "Confirm", message: "Long message", onConfirm: noop, onCancel: noop }, ["common.confirm", "common.cancel"]],
  ] as const;
  for (const [index, [name, props, actions]] of cases.entries()) {
    const Component = (await import(`../components/${name}.tsx`)).default as ComponentType<any>;
    await context.test(`${name} branch ${index} renders bounded regions and footer actions`, () => {
      const client = new QueryClient();
      try {
        const html = renderToStaticMarkup(createElement(QueryClientProvider, { client },
          createElement(MemoryRouter, null, createElement(Component, props))));
        assert.ok(html.includes("max-h-[calc(100vh-2rem)]"));
        assert.match(html, /<header[^>]*class="[^"]*shrink-0/);
        assert.ok(html.includes(modal.MODAL_BODY_CLASS));
        const footer = html.match(/<footer\b[^>]*>([\s\S]*?)<\/footer>/);
        assert.ok(footer, "actions need a footer outside the scrolling body");
        assert.ok(footer[0].includes(MODAL_FOOTER_CLASS));
        for (const key of actions) {
          const label = t(key, { langs: "es", count: 0 });
          assert.ok(footer[1].includes(label), `missing footer action: ${label}`);
        }
      } finally { client.clear(); }
    });
  }
});
