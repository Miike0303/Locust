/**
 * Paged review walk: translated strings first, then reviewed.
 *
 * Approving removes a row from the server's status filter, so offset-based
 * pages would shift and skip or repeat entries. The next fetch offset is the
 * count of loaded items in that bucket that are still reviewable (skipped,
 * not approved) — they occupy the front of the remaining list.
 */

export const REVIEW_PAGE_SIZE = 50;
export const REVIEW_PREFETCH_REMAINING = 8;

export type ReviewBucket = "translated" | "reviewed";

export interface ReviewQueueItem {
  id: string;
  bucket: ReviewBucket;
}

export interface ReviewPage {
  entries: { id: string }[];
  total: number;
}

export interface ReviewQueueState {
  items: ReviewQueueItem[];
  index: number;
  approvedCount: number;
  approvedIds: string[];
  /** Snapshot of translated.total + reviewed.total from bootstrap. */
  total: number;
  translatedExhausted: boolean;
  reviewedExhausted: boolean;
  bootstrapped: boolean;
  complete: boolean;
  /**
   * Approve/skip happened on the last buffered item while more pages exist.
   * The next applied page jumps the cursor to the first new item.
   */
  pendingAdvance: boolean;
}

export type ReviewFetchPlan =
  | { type: "bootstrap" }
  | { type: "translated"; offset: number; limit: number }
  | { type: "reviewed"; offset: number; limit: number }
  | { type: "none" };

export function createReviewQueue(): ReviewQueueState {
  return {
    items: [],
    index: 0,
    approvedCount: 0,
    approvedIds: [],
    total: 0,
    translatedExhausted: false,
    reviewedExhausted: false,
    bootstrapped: false,
    complete: false,
    pendingAdvance: false,
  };
}

/** Loaded items in `bucket` that have not been approved this session. */
export function reviewableLoadedCount(
  state: ReviewQueueState,
  bucket: ReviewBucket,
): number {
  const approved = new Set(state.approvedIds);
  let n = 0;
  for (const item of state.items) {
    if (item.bucket === bucket && !approved.has(item.id)) n += 1;
  }
  return n;
}

export function nextFetchPlan(
  state: ReviewQueueState,
  pageSize = REVIEW_PAGE_SIZE,
): ReviewFetchPlan {
  if (state.complete) return { type: "none" };
  if (!state.bootstrapped) return { type: "bootstrap" };
  if (!state.translatedExhausted) {
    return {
      type: "translated",
      offset: reviewableLoadedCount(state, "translated"),
      limit: pageSize,
    };
  }
  if (!state.reviewedExhausted) {
    return {
      type: "reviewed",
      offset: reviewableLoadedCount(state, "reviewed"),
      limit: pageSize,
    };
  }
  return { type: "none" };
}

export function shouldLoadMore(
  state: ReviewQueueState,
  remaining = REVIEW_PREFETCH_REMAINING,
): boolean {
  if (state.complete) return false;
  if (!state.bootstrapped) return true;
  if (state.translatedExhausted && state.reviewedExhausted) return false;
  if (state.pendingAdvance) return true;
  const last = state.items.length - 1;
  const leftInBuffer = last - state.index;
  return leftInBuffer <= remaining;
}

export function pageMatchesPlan(
  state: ReviewQueueState,
  bucket: ReviewBucket,
  fetchedOffset: number,
): boolean {
  return reviewableLoadedCount(state, bucket) === fetchedOffset;
}

function isExhausted(
  page: ReviewPage,
  offset: number,
  pageSize: number,
): boolean {
  if (page.entries.length < pageSize) return true;
  return offset + page.entries.length >= page.total;
}

function appendBucket(
  items: ReviewQueueItem[],
  entries: { id: string }[],
  bucket: ReviewBucket,
): ReviewQueueItem[] {
  const seen = new Set(items.map((i) => i.id));
  const next = [...items];
  for (const entry of entries) {
    if (seen.has(entry.id)) continue;
    seen.add(entry.id);
    next.push({ id: entry.id, bucket });
  }
  return next;
}

export function applyBootstrap(
  state: ReviewQueueState,
  translated: ReviewPage,
  reviewed: ReviewPage,
  pageSize = REVIEW_PAGE_SIZE,
): ReviewQueueState {
  const translatedExhausted = isExhausted(translated, 0, pageSize);
  let items = appendBucket([], translated.entries, "translated");
  let reviewedExhausted = reviewed.total === 0;
  if (translatedExhausted) {
    items = appendBucket(items, reviewed.entries, "reviewed");
    reviewedExhausted = isExhausted(reviewed, 0, pageSize);
  }
  const total = translated.total + reviewed.total;
  const empty = items.length === 0 && translatedExhausted && reviewedExhausted;
  return {
    ...state,
    items,
    index: 0,
    total,
    translatedExhausted,
    reviewedExhausted,
    bootstrapped: true,
    complete: empty,
    pendingAdvance: false,
  };
}

function settleCursor(state: ReviewQueueState): ReviewQueueState {
  const approved = new Set(state.approvedIds);
  let { index, pendingAdvance, complete } = state;
  const { items } = state;
  if (items.length === 0) {
    if (state.translatedExhausted && state.reviewedExhausted) {
      return { ...state, complete: true, pendingAdvance: false };
    }
    return { ...state, pendingAdvance: false };
  }

  if (pendingAdvance && index < items.length - 1) {
    index += 1;
    pendingAdvance = false;
  }

  while (index < items.length - 1 && approved.has(items[index].id)) {
    index += 1;
    pendingAdvance = false;
  }

  const drained = state.translatedExhausted && state.reviewedExhausted;
  const onApprovedLast =
    approved.has(items[index].id) && index === items.length - 1;
  if (onApprovedLast && drained) {
    complete = true;
    pendingAdvance = false;
  } else if (pendingAdvance && drained) {
    pendingAdvance = false;
  }

  return { ...state, index, pendingAdvance, complete };
}

export function applyFetchedPage(
  state: ReviewQueueState,
  bucket: ReviewBucket,
  page: ReviewPage,
  offset: number,
  pageSize = REVIEW_PAGE_SIZE,
): ReviewQueueState {
  const items = appendBucket(state.items, page.entries, bucket);
  const appended = items.length - state.items.length;
  // Duplicate/empty page: stop, so a shifted offset cannot loop forever.
  const exhausted = appended === 0 || isExhausted(page, offset, pageSize);
  const next: ReviewQueueState = {
    ...state,
    items,
    translatedExhausted:
      bucket === "translated" ? exhausted : state.translatedExhausted,
    reviewedExhausted:
      bucket === "reviewed" ? exhausted : state.reviewedExhausted,
  };
  return settleCursor(next);
}

function advanceFromCurrent(state: ReviewQueueState): ReviewQueueState {
  if (state.index < state.items.length - 1) {
    return { ...state, index: state.index + 1, pendingAdvance: false };
  }
  const drained = state.translatedExhausted && state.reviewedExhausted;
  if (drained) {
    return { ...state, complete: true, pendingAdvance: false };
  }
  return { ...state, pendingAdvance: true };
}

export function approveCurrent(state: ReviewQueueState): ReviewQueueState {
  if (state.complete) return state;
  const current = state.items[state.index];
  if (!current) return state;
  const already = state.approvedIds.includes(current.id);
  const next: ReviewQueueState = {
    ...state,
    approvedIds: already
      ? state.approvedIds
      : [...state.approvedIds, current.id],
    approvedCount: already ? state.approvedCount : state.approvedCount + 1,
  };
  return advanceFromCurrent(next);
}

export function skipCurrent(state: ReviewQueueState): ReviewQueueState {
  if (state.complete) return state;
  if (state.items.length === 0) return state;
  if (state.index < state.items.length - 1) {
    return { ...state, index: state.index + 1, pendingAdvance: false };
  }
  const drained = state.translatedExhausted && state.reviewedExhausted;
  if (drained) return state;
  return { ...state, pendingAdvance: true };
}

export function prevCurrent(state: ReviewQueueState): ReviewQueueState {
  if (state.index <= 0) return { ...state, pendingAdvance: false };
  return { ...state, index: state.index - 1, pendingAdvance: false };
}

export function reviewProgress(state: ReviewQueueState): {
  current: number;
  total: number;
  approved: number;
} {
  return {
    current: state.items.length === 0 ? 0 : state.index + 1,
    total: state.total,
    approved: state.approvedCount,
  };
}

export function mergeEntriesById<T extends { id: string }>(
  prev: Record<string, T>,
  incoming: T[],
): Record<string, T> {
  const next = { ...prev };
  for (const entry of incoming) next[entry.id] = entry;
  return next;
}
