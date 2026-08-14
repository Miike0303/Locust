/**
 * Lightweight asserts for pivot helpers
 * (run: npx --yes tsx src/lib/pivot.test.ts).
 */
import {
  defaultPivotFileName,
  errorMessage,
  isExistingOutputError,
  PIVOT_OPEN_DB_HTTP_PATH,
  PIVOT_OPEN_DB_TAURI_CMD,
  pivotCarryOverCount,
  pivotOpenDbArgs,
  pivotOpenDbHttpBody,
  pivotOpenDbTauriArgs,
  projectInfoAfterPivotOpen,
} from "./pivot.ts";

const assert = {
  equal(actual: unknown, expected: unknown, message?: string) {
    if (actual !== expected) throw new Error(message ?? `${actual} !== ${expected}`);
  },
  ok(cond: unknown, message?: string) {
    if (!cond) throw new Error(message ?? "expected truthy");
  },
};

assert.equal(pivotCarryOverCount(undefined), 0);
assert.equal(pivotCarryOverCount(null), 0);
assert.equal(
  pivotCarryOverCount({ translated: 10, reviewed: 2, approved: 3 }),
  15,
);
assert.equal(
  pivotCarryOverCount({ translated: 0, reviewed: 0, approved: 0 }),
  0,
);

assert.equal(defaultPivotFileName("My Game"), "My Game-pivot.locust.db");
assert.equal(defaultPivotFileName("  "), "project-pivot.locust.db");
assert.equal(defaultPivotFileName(""), "project-pivot.locust.db");

assert.ok(isExistingOutputError("409: output file already exists: C:\\a.locust.db"));
assert.ok(isExistingOutputError("File exists"));
assert.ok(isExistingOutputError("EEXIST"));
assert.equal(isExistingOutputError("no translated entries"), false);

assert.equal(errorMessage(new Error("boom")), "boom");
assert.equal(errorMessage("raw"), "raw");

const previous = {
  path: "C:\\Games\\Title",
  format_id: "rpgmaker-mv",
  name: "Title",
  supported_modes: ["replace" as const],
};
const openedDb = projectInfoAfterPivotOpen(previous, {
  format_id: "rpgmaker-mv",
  format_name: "RPG Maker MV",
  total_strings: 12,
  project_path: "C:\\Games\\Title-pivot.locust.db",
  project_name: "Title-pivot",
  supported_modes: ["replace"],
});
assert.equal(openedDb.path, previous.path, "keep game folder when opened path is a db");
assert.equal(openedDb.name, "Title-pivot");

const openedGame = projectInfoAfterPivotOpen(previous, {
  format_id: "renpy",
  format_name: "Ren'Py",
  total_strings: 4,
  project_path: "C:\\Games\\Other",
  project_name: "Other",
  supported_modes: ["replace", "add"],
});
assert.equal(openedGame.path, "C:\\Games\\Other");
assert.equal(openedGame.format_id, "renpy");

assert.equal(PIVOT_OPEN_DB_HTTP_PATH, "/project/open-db");
assert.equal(PIVOT_OPEN_DB_HTTP_PATH === "/project/open", false, "must not hit extract/merge open");
assert.equal(PIVOT_OPEN_DB_TAURI_CMD, "open_project_db");
assert.equal(PIVOT_OPEN_DB_TAURI_CMD === "open_project", false, "must not invoke extract/merge command");

const pivotedDb = "C:\\Games\\Title-pivot.locust.db";
const openDbArgs = pivotOpenDbArgs(pivotedDb, previous);
assert.equal(openDbArgs.databasePath, pivotedDb, "db file is database_path");
assert.equal(openDbArgs.gamePath, previous.path, "game_path is the project we pivoted from");
assert.equal(openDbArgs.formatId, previous.format_id, "format_id is the project we pivoted from");
assert.equal(openDbArgs.gamePath === openDbArgs.databasePath, false);
assert.equal(/\.locust\.db$/i.test(openDbArgs.gamePath), false, "game_path must not be the sqlite file");
assert.ok(/\.locust\.db$/i.test(openDbArgs.databasePath));

const httpBody = pivotOpenDbHttpBody(openDbArgs);
assert.equal(httpBody.database_path, pivotedDb);
assert.equal(httpBody.game_path, "C:\\Games\\Title");
assert.equal(httpBody.format_id, "rpgmaker-mv");
assert.equal(
  Object.keys(httpBody).sort().join(","),
  "database_path,format_id,game_path",
);
assert.equal("path" in httpBody, false, "openProject's path field must not be used");

const tauriArgs = pivotOpenDbTauriArgs(openDbArgs);
assert.equal(tauriArgs.databasePath, pivotedDb);
assert.equal(tauriArgs.gamePath, "C:\\Games\\Title");
assert.equal(tauriArgs.formatId, "rpgmaker-mv");
assert.equal(
  Object.keys(tauriArgs).sort().join(","),
  "databasePath,formatId,gamePath",
);
assert.equal("path" in tauriArgs, false, "open_project's path field must not be used");

const openedFromOpenDb = projectInfoAfterPivotOpen(previous, {
  format_id: previous.format_id,
  format_name: "RPG Maker MV",
  total_strings: 12,
  project_path: previous.path,
  project_name: "Title",
  supported_modes: ["replace"],
});
assert.equal(openedFromOpenDb.path, previous.path, "open-db returns the original game folder");
assert.equal(openedFromOpenDb.format_id, previous.format_id);

console.log("pivot.test.ts: ok");
