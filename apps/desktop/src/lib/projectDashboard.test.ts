import assert from "node:assert/strict";
import { test } from "node:test";
import type { FileStats, ProjectInfo, ProjectStats } from "./api.ts";
import { buildProjectDashboard, formatDashboardCost } from "./projectDashboard.ts";

for (const [cost, expected] of [[0, "$0.00"], [0.004, "$0.004"], [0.0123, "$0.01"], [12.5, "$12.50"]] as const) {
  test(`dashboard cost ${cost} formats as ${expected}`, () => {
    assert.equal(formatDashboardCost(cost), expected);
  });
}

const project: ProjectInfo = { path: "C:\\Games\\Example\\", name: "Saved name", format_id: "html" };
const stats: ProjectStats = { total: 10, pending: 3, translated: 2, reviewed: 1, approved: 3, error: 1, total_cost_usd: 0 };
const build = (overrides: Partial<Parameters<typeof buildProjectDashboard>[0]> = {}) =>
  buildProjectDashboard({ project, stats, files: [], skipReview: false, ...overrides });

test("progress counts translated, reviewed and approved and identifies the game folder", () => {
  const model = build();
  assert.equal(model.percent, 60);
  assert.equal(model.projectName, "Example");
  assert.deepEqual(model.segments.map(s => [s.status, s.count, s.width]), [
    ["pending", 3, 30], ["translated", 2, 20], ["reviewed", 1, 10], ["approved", 3, 30], ["error", 1, 10],
  ]);
});

test("zero totals produce zero progress and a full neutral empty track without NaN", () => {
  const model = build({ stats: { ...stats, total: 0, pending: 0, translated: 0, reviewed: 0, approved: 0, error: 0 } });
  assert.equal(model.percent, 0);
  assert.equal(model.segments.reduce((sum, segment) => sum + segment.width, 0), 100);
  assert.equal(model.segments[0].width, 100);
  assert.ok(model.segments.every(segment => Number.isFinite(segment.width)));
  assert.equal(model.nextStep, null);
});

test("fractional segment widths sum to 100", () => {
  const model = build({ stats: { ...stats, total: 3, pending: 1, translated: 1, reviewed: 0, approved: 0, error: 1 } });
  assert.equal(model.percent, 33);
  assert.equal(model.segments.reduce((sum, segment) => sum + segment.width, 0), 100);
});

test("files sort by pending then error, take ten, preserve input and show relative paths", () => {
  const files: FileStats[] = Array.from({ length: 12 }, (_, index) => ({
    file_path: `C:\\Games\\Example\\data\\${index}.json`, total: 10,
    pending: Math.floor(index / 2), translated: 2, reviewed: 1, approved: 2, error: index % 2,
  }));
  const original = structuredClone(files);
  const model = build({ files });
  assert.deepEqual(model.topFiles.map(f => f.file_path), [...files].reverse().slice(0, 10).map(f => f.file_path));
  assert.equal(model.topFiles[0].relativePath, "data/11.json");
  assert.equal(model.topFiles[0].percent, 50);
  assert.equal(model.errorFiles.length, 6);
  assert.deepEqual(files, original);
  assert.equal(build({ files: [{ ...files[0], total: 0 }] }).topFiles[0].percent, 0);
});

test("warnings are bounded to ten with a remaining count", () => {
  const warnings = Array.from({ length: 13 }, (_, i) => `Warning ${i}`);
  const model = build({ project: { ...project, extraction_warnings: warnings } });
  assert.deepEqual(model.warnings, warnings.slice(0, 10));
  assert.equal(model.moreWarnings, 3);
  assert.equal(build().moreWarnings, 0);
});

test("next steps reuse workflow priorities and map review to Review, translate and inject to Editor", () => {
  assert.deepEqual(build().nextStep, { step: "translate", path: "/editor" });
  assert.deepEqual(build({ stats: { ...stats, pending: 0 } }).nextStep, { step: "review", path: "/review" });
  assert.deepEqual(build({ stats: { ...stats, pending: 0 }, skipReview: true }).nextStep, { step: "inject", path: "/editor" });
  assert.deepEqual(build({ stats: { ...stats, pending: 0, translated: 0, reviewed: 0 } }).nextStep, { step: "inject", path: "/editor" });
});
