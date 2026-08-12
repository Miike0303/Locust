/**
 * Lightweight asserts for workflowGuide (run: npx --yes tsx src/lib/workflowGuide.test.ts).
 */
import assert from "node:assert/strict";
import { resolveWorkflowGuideStep } from "./workflowGuide.ts";

const stats = (
  pending = 0,
  translated = 0,
  reviewed = 0,
  approved = 0
) => ({ pending, translated, reviewed, approved });

const cases = [
  {
    name: "no project has no contextual step",
    hasProject: false,
    stats: stats(5),
    skipReview: false,
    expected: null,
  },
  {
    name: "pending-only work starts with translate",
    hasProject: true,
    stats: stats(5),
    skipReview: false,
    expected: "translate",
  },
  {
    name: "skip review does not bypass pending-only translation",
    hasProject: true,
    stats: stats(5),
    skipReview: true,
    expected: "translate",
  },
  {
    name: "translated work continues to review",
    hasProject: true,
    stats: stats(0, 5),
    skipReview: false,
    expected: "review",
  },
  {
    name: "reviewed work remains in review until approved",
    hasProject: true,
    stats: stats(0, 0, 5),
    skipReview: false,
    expected: "review",
  },
  {
    name: "skip review sends translated work to inject",
    hasProject: true,
    stats: stats(0, 5),
    skipReview: true,
    expected: "inject",
  },
  {
    name: "skip review sends reviewed work to inject",
    hasProject: true,
    stats: stats(0, 0, 5),
    skipReview: true,
    expected: "inject",
  },
  {
    name: "approved-only work is ready to inject",
    hasProject: true,
    stats: stats(0, 0, 0, 5),
    skipReview: false,
    expected: "inject",
  },
  {
    name: "mixed pending and translated work keeps translate primary",
    hasProject: true,
    stats: stats(2, 3),
    skipReview: false,
    expected: "translate",
  },
  {
    name: "mixed pending and translated work stays on translate when review is skipped",
    hasProject: true,
    stats: stats(2, 3),
    skipReview: true,
    expected: "translate",
  },
] as const;

for (const testCase of cases) {
  assert.equal(
    resolveWorkflowGuideStep({
      hasProject: testCase.hasProject,
      stats: testCase.stats,
      skipReview: testCase.skipReview,
    }),
    testCase.expected,
    testCase.name
  );
}

console.log("workflowGuide.test.ts: ok");
