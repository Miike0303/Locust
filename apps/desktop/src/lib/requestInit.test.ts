import assert from "node:assert/strict";
import { test } from "node:test";
import { mergeRequestInit } from "./requestInit.ts";

test("request headers default to JSON without options", () => {
  assert.equal(new Headers(mergeRequestInit().headers).get("content-type"), "application/json");
});

test("custom object headers keep the JSON default and other request options", () => {
  const controller = new AbortController();
  const options: RequestInit = {
    method: "POST", body: "{}", signal: controller.signal, headers: { "X-Test": "present" },
  };
  const merged = mergeRequestInit(options);
  const headers = new Headers(merged.headers);
  assert.equal(headers.get("content-type"), "application/json");
  assert.equal(headers.get("x-test"), "present");
  assert.equal(merged.method, options.method);
  assert.equal(merged.body, options.body);
  assert.equal(merged.signal, controller.signal);
  assert.deepEqual(options.headers, { "X-Test": "present" });
});

test("caller content type wins regardless of casing", () => {
  for (const name of ["Content-Type", "content-type", "CONTENT-TYPE"]) {
    const headers = new Headers(mergeRequestInit({ headers: { [name]: "text/plain" } }).headers);
    assert.equal(headers.get("content-type"), "text/plain");
  }
});

test("Headers input keeps custom headers without mutation", () => {
  const original = new Headers({ "X-Test": "present" });
  const headers = new Headers(mergeRequestInit({ headers: original }).headers);
  assert.equal(headers.get("content-type"), "application/json");
  assert.equal(headers.get("x-test"), "present");
  assert.equal(original.has("content-type"), false);
  original.set("content-type", "text/plain");
  assert.equal(new Headers(mergeRequestInit({ headers: original }).headers).get("content-type"), "text/plain");
});

test("tuple headers keep JSON by default and allow an explicit content type", () => {
  const original: [string, string][] = [["X-Test", "present"]];
  const headers = new Headers(mergeRequestInit({ headers: original }).headers);
  assert.equal(headers.get("content-type"), "application/json");
  assert.equal(headers.get("x-test"), "present");
  assert.deepEqual(original, [["X-Test", "present"]]);
  assert.equal(new Headers(mergeRequestInit({
    headers: [...original, ["content-type", "text/plain"]],
  }).headers).get("content-type"), "text/plain");
});
