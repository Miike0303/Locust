import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";

const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.endsWith("/lib/api.ts")) {
      const source = specifier === "./runtime" ? "export const IS_TAURI=true;"
        : specifier === "@tauri-apps/api/core" ? "export const invoke=async cmd=>{if(cmd!=='get_server_port')throw Error('Unexpected IPC: '+cmd);return 7842;};" : null;
      if (source) return { shortCircuit: true, url: `data:text/javascript,${encodeURIComponent(source)}` };
    }
    return nextResolve(specifier, context);
  },
});
const { speakerOwner } = await import("./speakers.ts");
const { getSpeakers, saveSpeakerNote } = await import("./api.ts");
after(() => hooks.deregister());
const project = { database_path: "C:/db/a & b.db", path: "C:/game?one", format_id: "renpy", name: "Demo" };

test("speaker ownership requires the exact current-project database, game and format", () => {
  assert.deepEqual(speakerOwner(project), { database_path: project.database_path, project_path: project.path, format_id: "renpy" });
  assert.equal(speakerOwner({ ...project, database_path: undefined }), null);
});

test("speaker clients use embedded HTTP with exact ownership, opaque IDs, clear and cancellation", async () => {
  const previous = globalThis.fetch;
  const calls: { url: string; init?: RequestInit }[] = [];
  const groups = { groups: [{ id: '["explicit","Alice"]', name: "Alice", note: "", count: 1, entry_ids: ["row"] }], unassigned_entry_ids: ["unknown"] };
  globalThis.fetch = async (url, init) => {
    calls.push({ url: String(url), init });
    return new Response(JSON.stringify(init?.method === "PUT" ? JSON.parse(String(init.body)) : groups));
  };
  try {
    const owner = speakerOwner(project)!;
    const signal = new AbortController().signal;
    assert.deepEqual(await getSpeakers(owner, signal), groups);
    const url = new URL(calls[0].url);
    assert.equal(url.origin + url.pathname, "http://localhost:7842/api/speakers");
    assert.deepEqual(Object.fromEntries(url.searchParams), owner);
    assert.equal(calls[0].init?.signal, signal);
    for (const note of ["Formal speech", ""]) {
      await saveSpeakerNote({ ...owner, speaker_id: groups.groups[0].id, note });
      const call = calls[calls.length - 1];
      assert.equal(call.url, "http://localhost:7842/api/speakers/note");
      assert.equal(call.init?.method, "PUT");
      assert.deepEqual(JSON.parse(String(call.init?.body)), { ...owner, speaker_id: groups.groups[0].id, note });
    }
    globalThis.fetch = async () => new Response("speaker project changed; reload the project", { status: 409 });
    await assert.rejects(saveSpeakerNote({ ...owner, speaker_id: "opaque", note: "keep" }), /speaker project changed/);
  } finally { globalThis.fetch = previous; }
});
