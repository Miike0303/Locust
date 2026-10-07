import assert from "node:assert/strict";
import { test } from "node:test";
import { defaultEntryPath, publishCommand, patchPublishFields } from "./patchPublish.ts";

test("default entry path follows the zip stem on Windows and POSIX", () => {
  for (const [zip, entry] of [
    [String.raw`C:\My Game\patch.zip`, String.raw`C:\My Game\patch.md`],
    ["/releases/game.es.ZIP", "/releases/game.es.md"],
    ["/releases.v1/patch", "/releases.v1/patch.md"],
    ["patch.zip", "patch.md"], ["", ""],
  ]) assert.equal(defaultEntryPath(zip), entry);
});

test("publish command quotes both paths and preserves Windows separators", () => {
  assert.equal(publishCommand(String.raw`C:\My Game\patch.zip`, String.raw`C:\My Game\entry.md`),
    String.raw`npm run publish-patch -- "C:\My Game\patch.zip" "C:\My Game\entry.md"`);
  assert.equal(publishCommand('/a/"game".zip', "/a/entry.md"),
    'npm run publish-patch -- "/a/\\"game\\".zip" "/a/entry.md"');
});

test("publish fields omit empty metadata and respect explicit detection opt-out", () => {
  assert.deepEqual(patchPublishFields({ rjCode: "", gameVersion: " ", detectId: false, createEntry: false, entryPath: "ignored.md" }), { detect_id: false });
  assert.deepEqual(patchPublishFields({ rjCode: " rj01234567 ", gameVersion: " 1.2 ", detectId: true, createEntry: true, entryPath: " C:\\entry.md " }), {
    rj_code: "rj01234567", game_version: "1.2", detect_id: true, entry_path: "C:\\entry.md",
  });
  assert.deepEqual(patchPublishFields({ rjCode: "", gameVersion: "", detectId: true, createEntry: false, entryPath: "" }), { detect_id: true });
});
