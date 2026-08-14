/**
 * Lightweight asserts for reviewQueue
 * (run: npx --yes tsx src/lib/reviewQueue.test.ts).
 */
import assert from "node:assert/strict";
import {
  applyBootstrap,
  applyFetchedPage,
  approveCurrent,
  createReviewQueue,
  mergeEntriesById,
  nextFetchPlan,
  pageMatchesPlan,
  prevCurrent,
  reviewableLoadedCount,
  reviewProgress,
  shouldLoadMore,
  skipCurrent,
} from "./reviewQueue.ts";

const PAGE = 3;

function page(ids: string[], total: number) {
  return { entries: ids.map((id) => ({ id })), total };
}

function ids(state: ReturnType<typeof createReviewQueue>): string[] {
  return state.items.map((i) => i.id);
}

function currentId(state: ReturnType<typeof createReviewQueue>): string | undefined {
  return state.items[state.index]?.id;
}

// ── bootstrap: first paint is one page, total is server totals ───────────
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 7),
    page(["r0", "r1", "r2"], 3),
    PAGE,
  );
  assert.deepEqual(ids(q), ["t0", "t1", "t2"]);
  assert.equal(q.translatedExhausted, false);
  assert.equal(q.reviewedExhausted, false);
  assert.equal(q.bootstrapped, true);
  assert.equal(q.complete, false);
  assert.deepEqual(reviewProgress(q), { current: 1, total: 10, approved: 0 });
  assert.equal(shouldLoadMore(q, 0), false);
  assert.equal(shouldLoadMore(q, 2), true);
  assert.deepEqual(nextFetchPlan(q, PAGE), {
    type: "translated",
    offset: 3,
    limit: PAGE,
  });
}

// Empty project
{
  const q = applyBootstrap(
    createReviewQueue(),
    page([], 0),
    page([], 0),
    PAGE,
  );
  assert.equal(q.complete, true);
  assert.equal(q.total, 0);
  assert.deepEqual(ids(q), []);
}

// Translated empty → first page is reviewed
{
  const q = applyBootstrap(
    createReviewQueue(),
    page([], 0),
    page(["r0", "r1"], 2),
    PAGE,
  );
  assert.deepEqual(ids(q), ["r0", "r1"]);
  assert.equal(q.translatedExhausted, true);
  assert.equal(q.reviewedExhausted, true);
  assert.equal(q.items[0].bucket, "reviewed");
  assert.deepEqual(reviewProgress(q), { current: 1, total: 2, approved: 0 });
}

// ── THE skip bug: offset must be skipped-still-in-set, not page size ─────
// Remaining after approve t0+t2, skip t1: [t1, t3, t4, t5, t6]
// Fetch offset 1 → t3,t4,t5. Offset 3 (page size) would skip t3+t4.
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 7),
    page(["r0"], 3),
    PAGE,
  );
  q = approveCurrent(q); // t0
  q = skipCurrent(q); // t1
  q = approveCurrent(q); // t2, last in buffer
  assert.equal(currentId(q), "t2");
  assert.equal(q.pendingAdvance, true);
  assert.equal(reviewableLoadedCount(q, "translated"), 1);
  assert.deepEqual(nextFetchPlan(q, PAGE), {
    type: "translated",
    offset: 1,
    limit: PAGE,
  });
  assert.equal(pageMatchesPlan(q, "translated", 1), true);
  assert.equal(pageMatchesPlan(q, "translated", 3), false);

  q = applyFetchedPage(q, "translated", page(["t3", "t4", "t5"], 6), 1, PAGE);
  assert.deepEqual(ids(q), ["t0", "t1", "t2", "t3", "t4", "t5"]);
  assert.equal(currentId(q), "t3");
  assert.equal(q.pendingAdvance, false);
  assert.equal(q.approvedCount, 2);
  assert.deepEqual(reviewProgress(q), { current: 4, total: 10, approved: 2 });
}

// Approve the whole first page → next offset is 0 (nothing left in-set)
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 7),
    page([], 0),
    PAGE,
  );
  q = approveCurrent(q);
  q = approveCurrent(q);
  q = approveCurrent(q);
  assert.equal(q.pendingAdvance, true);
  assert.equal(reviewableLoadedCount(q, "translated"), 0);
  assert.deepEqual(nextFetchPlan(q, PAGE), {
    type: "translated",
    offset: 0,
    limit: PAGE,
  });
  q = applyFetchedPage(q, "translated", page(["t3", "t4", "t5"], 4), 0, PAGE);
  assert.equal(currentId(q), "t3");
  assert.equal(q.translatedExhausted, false);
}

// ── in-flight fetch discarded when approvals shift the offset ────────────
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 7),
    page([], 0),
    PAGE,
  );
  q = skipCurrent(q);
  q = skipCurrent(q);
  q = skipCurrent(q); // pendingAdvance, offset 3
  assert.deepEqual(nextFetchPlan(q, PAGE), {
    type: "translated",
    offset: 3,
    limit: PAGE,
  });
  q = prevCurrent(q);
  q = approveCurrent(q); // approve t1 while a fetch at offset 3 is in flight
  assert.equal(pageMatchesPlan(q, "translated", 3), false);
  assert.equal(reviewableLoadedCount(q, "translated"), 2); // t0, t2 skipped
}

// ── no duplicate ids if an overlapping page is applied anyway ────────────
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 7),
    page([], 0),
    PAGE,
  );
  q = applyFetchedPage(q, "translated", page(["t1", "t2", "t3"], 7), 1, PAGE);
  assert.deepEqual(ids(q), ["t0", "t1", "t2", "t3"]);
}

// Duplicate-only page marks the bucket exhausted (no refetch loop)
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 7),
    page([], 0),
    PAGE,
  );
  q = applyFetchedPage(q, "translated", page(["t0", "t1", "t2"], 7), 3, PAGE);
  assert.deepEqual(ids(q), ["t0", "t1", "t2"]);
  assert.equal(q.translatedExhausted, true);
}

// ── skip at true end does not complete; approve does ─────────────────────
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1"], 2),
    page([], 0),
    PAGE,
  );
  assert.equal(q.translatedExhausted, true);
  q = skipCurrent(q);
  q = skipCurrent(q);
  assert.equal(q.complete, false);
  assert.equal(currentId(q), "t1");
  q = approveCurrent(q);
  assert.equal(q.complete, true);
  assert.equal(q.approvedCount, 1);
}

// ── prev / re-approve does not double-count ──────────────────────────────
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 3),
    page([], 0),
    PAGE,
  );
  q = approveCurrent(q);
  q = prevCurrent(q);
  assert.equal(currentId(q), "t0");
  q = approveCurrent(q);
  assert.equal(q.approvedCount, 1);
  assert.equal(currentId(q), "t1");
}

// ── reviewed only after translated is exhausted ──────────────────────────
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 4),
    page(["r0", "r1"], 2),
    PAGE,
  );
  q = approveCurrent(q);
  q = approveCurrent(q);
  q = approveCurrent(q);
  q = applyFetchedPage(q, "translated", page(["t3"], 1), 0, PAGE);
  assert.equal(q.translatedExhausted, true);
  assert.equal(currentId(q), "t3");
  q = skipCurrent(q);
  assert.equal(q.pendingAdvance, true);
  assert.deepEqual(nextFetchPlan(q, PAGE), {
    type: "reviewed",
    offset: 0,
    limit: PAGE,
  });
  q = applyFetchedPage(q, "reviewed", page(["r0", "r1"], 2), 0, PAGE);
  assert.equal(currentId(q), "r0");
  assert.equal(q.items[q.index].bucket, "reviewed");
  assert.equal(q.reviewedExhausted, true);
  q = approveCurrent(q);
  q = approveCurrent(q);
  assert.equal(q.complete, true);
  assert.deepEqual(reviewProgress(q), { current: 6, total: 6, approved: 5 });
}

// ── progress denominator stays the bootstrap total after approvals ───────
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 20),
    page(["r0"], 5),
    PAGE,
  );
  q = approveCurrent(q);
  q = applyFetchedPage(q, "translated", page(["t3", "t4", "t5"], 19), 2, PAGE);
  assert.equal(reviewProgress(q).total, 25);
  assert.equal(reviewProgress(q).approved, 1);
}

// ── shouldLoadMore near end of buffer ────────────────────────────────────
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0", "t1", "t2"], 10),
    page([], 0),
    PAGE,
  );
  assert.equal(shouldLoadMore(q, 1), false); // 2 left in buffer, threshold 1
  q = skipCurrent(q);
  q = skipCurrent(q); // on t2, 0 left
  assert.equal(shouldLoadMore(q, 1), true);
}

// ── skip pendingAdvance then empty exhausted reviewed page completes walk ─
{
  let q = applyBootstrap(
    createReviewQueue(),
    page(["t0"], 1),
    page([], 0),
    PAGE,
  );
  q = skipCurrent(q);
  assert.equal(q.complete, false);
  assert.equal(q.pendingAdvance, false);
}

// ── mergeEntriesById ─────────────────────────────────────────────────────
{
  const merged = mergeEntriesById({ a: { id: "a", n: 1 } }, [
    { id: "b", n: 2 },
    { id: "a", n: 9 },
  ]);
  assert.equal(merged.a.n, 9);
  assert.equal(merged.b.n, 2);
}

console.log("reviewQueue.test.ts: ok");
