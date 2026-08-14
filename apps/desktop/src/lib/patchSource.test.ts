/**
 * Lightweight asserts for patchSource (run: npx --yes tsx src/lib/patchSource.test.ts).
 */
import assert from "node:assert/strict";
import {
  PATCH_SOURCE_ERROR,
  isHttpPatchUrl,
  patchSourceReady,
  patchUrlLooksLikeZip,
  resolvePatchSource,
} from "./patchSource.ts";

assert.equal(isHttpPatchUrl(""), false);
assert.equal(isHttpPatchUrl("ftp://x/a.zip"), false);
assert.equal(isHttpPatchUrl("https://"), false);
assert.equal(isHttpPatchUrl("http://"), false);
assert.equal(isHttpPatchUrl("not a url"), false);
assert.equal(isHttpPatchUrl("https://ex.com/p.zip"), true);
assert.equal(isHttpPatchUrl("HTTP://ex.com/p.zip"), true);
assert.equal(isHttpPatchUrl("  https://ex.com/p.zip  "), true);
assert.equal(isHttpPatchUrl("https://ex.com/p.zip?sig=1"), true);
// WHATWG: "https:///name" treats "name" as the host — accepted if host present.
assert.equal(isHttpPatchUrl("https:///cdn.example/p.zip"), true);

assert.equal(patchUrlLooksLikeZip("https://ex.com/p.zip"), true);
assert.equal(patchUrlLooksLikeZip("https://ex.com/p.ZIP?token=1"), true);
assert.equal(patchUrlLooksLikeZip("https://ex.com/download?id=1"), false);
assert.equal(patchUrlLooksLikeZip("not-a-url"), false);

assert.equal(resolvePatchSource("", ""), null);
assert.deepEqual(resolvePatchSource("C:\\a.zip", ""), { zip_path: "C:\\a.zip" });
assert.deepEqual(resolvePatchSource("", "https://ex.com/a.zip"), {
  zip_url: "https://ex.com/a.zip",
});
assert.deepEqual(resolvePatchSource("", "  https://ex.com/a.zip  "), {
  zip_url: "https://ex.com/a.zip",
});

const both = resolvePatchSource("a.zip", "https://x/a.zip");
assert.ok(both && "error" in both);
assert.equal(both.error, PATCH_SOURCE_ERROR.both);

const bad = resolvePatchSource("", "file:///tmp/a.zip");
assert.ok(bad && "error" in bad);
assert.equal(bad.error, PATCH_SOURCE_ERROR.badUrl);

const noHost = resolvePatchSource("", "https://");
assert.ok(noHost && "error" in noHost);
assert.equal(noHost.error, PATCH_SOURCE_ERROR.badUrl);

assert.equal(patchSourceReady(null), false);
assert.equal(patchSourceReady({ error: "x" }), false);
assert.equal(patchSourceReady({ zip_path: "a.zip" }), true);
assert.equal(patchSourceReady({ zip_url: "https://x/a.zip" }), true);

console.log("patchSource.test.ts: ok");
