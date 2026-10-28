import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { addToast, useToastStore as store } from "./toastStore";

beforeEach((ctx) => {
  assert.ok("mock" in ctx, "toast fixtures run in a test context");
  ctx.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1000 });
});
afterEach(() => {
  for (const toast of store.getState().toasts) store.getState().dismiss(toast.id);
});
const messages = () => store.getState().toasts.map(toast => toast.message);

for (const type of ["success", "info"] as const) {
  test(`${type} retains the four-second default and public void API`, (t) => {
    assert.equal(addToast(type, "message"), undefined);
    t.mock.timers.tick(3999);
    assert.deepEqual(messages(), ["message"]);
    t.mock.timers.tick(1);
    assert.deepEqual(messages(), []);
  });
}
for (const type of ["error", "warning"] as const) {
  test(`${type} persists by default until dismissed`, (t) => {
    addToast(type, "needs attention");
    t.mock.timers.tick(60000);
    assert.deepEqual(messages(), ["needs attention"]);
    store.getState().dismiss(store.getState().toasts[0].id);
    assert.deepEqual(messages(), []);
  });
}
for (const type of ["success", "info", "warning", "error"] as const) {
  test(`${type} honors an explicit duration through either public add helper`, (t) => {
    store.getState().add(type, "explicit", 8000);
    t.mock.timers.tick(7999);
    assert.deepEqual(messages(), ["explicit"]);
    t.mock.timers.tick(1);
    assert.deepEqual(messages(), []);
  });
}
for (const duration of [0, -20]) {
  test(`explicit ${duration} remains asynchronous immediate expiry, not persistence`, (t) => {
    addToast("error", "immediate", duration);
    assert.deepEqual(messages(), ["immediate"]);
    t.mock.timers.tick(1);
    assert.deepEqual(messages(), []);
  });
}
test("equal-priority burst keeps only the latest five in insertion order", () => {
  for (let i = 0; i < 7; i++) addToast("success", String(i));
  assert.deepEqual(messages(), ["2", "3", "4", "5", "6"]);
});
test("persistent error and warning survive six successes", (t) => {
  addToast("error", "error");
  addToast("warning", "warning");
  for (let i = 0; i < 6; i++) addToast("success", String(i));
  assert.deepEqual(messages(), ["error", "warning", "3", "4", "5"]);
  t.mock.timers.tick(4000);
  assert.deepEqual(messages(), ["error", "warning"]);
});
test("a full error queue drops lower priority arrivals, but admits newest errors", () => {
  for (let i = 0; i < 5; i++) addToast("error", String(i));
  addToast("success", "drop");
  addToast("warning", "drop warning");
  assert.deepEqual(messages(), ["0", "1", "2", "3", "4"]);
  addToast("error", "new error");
  assert.deepEqual(messages(), ["1", "2", "3", "4", "new error"]);
});
test("repeated pause/resume cycles consume only active time and ignore duplicate events", (t) => {
  addToast("info", "cycles", 4000);
  const id = store.getState().toasts[0].id;
  t.mock.timers.tick(1000);
  store.getState().pause(id, "focus");
  t.mock.timers.tick(500);
  store.getState().pause(id, "focus");
  t.mock.timers.tick(5000);
  store.getState().resume(id, "focus");
  t.mock.timers.tick(1000);
  store.getState().resume(id, "focus");
  store.getState().pause(id, "pointer");
  t.mock.timers.tick(5000);
  store.getState().resume(id, "pointer");
  t.mock.timers.tick(1999);
  assert.deepEqual(messages(), ["cycles"]);
  t.mock.timers.tick(1);
  assert.deepEqual(messages(), []);
});
test("interaction cannot add an expiry to a persistent toast", (t) => {
  addToast("error", "persistent");
  const id = store.getState().toasts[0].id;
  store.getState().pause(id, "focus");
  store.getState().resume(id, "focus");
  t.mock.timers.tick(60000);
  assert.deepEqual(messages(), ["persistent"]);
});
for (const removal of ["dismiss", "evict"] as const) {
  test(`${removal} cancels the removed toast's timer instead of publishing stale expiry`, (t) => {
    addToast("info", "timed", 100);
    const id = store.getState().toasts[0].id;
    if (removal === "dismiss") store.getState().dismiss(id);
    else for (let i = 0; i < 5; i++) addToast("info", String(i), 1000);
    let updates = 0;
    const unsubscribe = store.subscribe(() => updates++);
    try {
      t.mock.timers.tick(100);
      assert.equal(updates, 0);
    } finally { unsubscribe(); }
  });
}
