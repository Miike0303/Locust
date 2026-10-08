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

test("publish command uses PowerShell literals for spaces and Windows separators", () => {
  assert.equal(publishCommand(String.raw`C:\My Game\patch.zip`, String.raw`C:\My Game\entry.md`),
    String.raw`npm run publish-patch -- 'C:\My Game\patch.zip' 'C:\My Game\entry.md'`);
});

test("publish command preserves double quotes inside PowerShell literals", () => {
  assert.equal(publishCommand('/a/"game".zip', '/a/"entry".md'),
    `npm run publish-patch -- '/a/"game".zip' '/a/"entry".md'`);
});

for (const { name, zip, entry, command } of [
  {
    name: "apostrophes, including consecutive apostrophes",
    zip: String.raw`C:\O'Brien''s Game\patch.zip`,
    entry: "/entries/O'Brien's.md",
    command: String.raw`npm run publish-patch -- 'C:\O''Brien''''s Game\patch.zip' '/entries/O''Brien''s.md'`,
  },
  {
    name: "dollar variables and expressions",
    zip: String.raw`C:\$HOME\$(Get-Date)\patch.zip`,
    entry: "/entries/${name} $env:USERPROFILE.md",
    command: "npm run publish-patch -- 'C:\\$HOME\\$(Get-Date)\\patch.zip' '/entries/${name} $env:USERPROFILE.md'",
  },
  {
    name: "backticks",
    zip: "C:\\`whoami`\\patch.zip",
    entry: "/entries/`$(Get-Date)`n.md",
    command: "npm run publish-patch -- 'C:\\`whoami`\\patch.zip' '/entries/`$(Get-Date)`n.md'",
  },
  {
    name: "shell metacharacters",
    zip: "/patches/a; & | < > ( ) { } [ ] * ? ! # % @.zip",
    entry: "/entries/a&&b||c;#comment.md",
    command: "npm run publish-patch -- '/patches/a; & | < > ( ) { } [ ] * ? ! # % @.zip' '/entries/a&&b||c;#comment.md'",
  },
  {
    name: "apostrophes mixed with expressions and backticks",
    zip: "C:\\O'Brien\\$(Get-Date) `tick` & patch.zip",
    entry: "/entries/O'Brien's ${name}; `tick`.md",
    command: "npm run publish-patch -- 'C:\\O''Brien\\$(Get-Date) `tick` & patch.zip' '/entries/O''Brien''s ${name}; `tick`.md'",
  },
  {
    name: "empty arguments",
    zip: "",
    entry: "",
    command: "npm run publish-patch -- '' ''",
  },
]) {
  test(`publish command quotes ${name} as PowerShell literals`, () => {
    assert.equal(publishCommand(zip, entry), command);
  });
}

for (const [codePoint, quote, doubled] of [
  ["U+2018", "\u2018", "\u2018\u2018"],
  ["U+2019", "\u2019", "\u2019\u2019"],
  ["U+201A", "\u201a", "\u201a\u201a"],
  ["U+201B", "\u201b", "\u201b\u201b"],
]) {
  test(`publish command doubles ${codePoint} without changing the path character`, () => {
    const zip = String.raw`C:\O${quote}Brien Game\patch.zip`;
    const entry = String.raw`C:\O${quote}${quote}Brien Game\entry.md`;
    assert.equal(publishCommand(zip, entry),
      String.raw`npm run publish-patch -- 'C:\O${doubled}Brien Game\patch.zip' 'C:\O${doubled}${doubled}Brien Game\entry.md'`);
  });
}

test("publish command preserves mixed quote characters next to shell syntax", () => {
  const zip = "C:\\O'\u2018\u2019\u201a\u201bBrien\\$HOME $(1+2) `tick`; & patch.zip";
  const entry = "/entries/\u201b\u201a\u2019\u2018'OBrien ${name}; `tick`.md";
  assert.equal(publishCommand(zip, entry),
    "npm run publish-patch -- 'C:\\O''\u2018\u2018\u2019\u2019\u201a\u201a\u201b\u201bBrien\\$HOME $(1+2) `tick`; & patch.zip' '/entries/\u201b\u201b\u201a\u201a\u2019\u2019\u2018\u2018''OBrien ${name}; `tick`.md'");
});

test("publish command leaves non-delimiter apostrophe lookalikes unchanged", () => {
  assert.equal(publishCommand("C:\\O\u02bcBrien\\patch.zip", "/entries/O\uff07Brien.md"),
    "npm run publish-patch -- 'C:\\O\u02bcBrien\\patch.zip' '/entries/O\uff07Brien.md'");
});

test("publish fields omit empty metadata and respect explicit detection opt-out", () => {
  assert.deepEqual(patchPublishFields({ rjCode: "", gameVersion: " ", detectId: false, createEntry: false, entryPath: "ignored.md" }), { detect_id: false });
  assert.deepEqual(patchPublishFields({ rjCode: " rj01234567 ", gameVersion: " 1.2 ", detectId: true, createEntry: true, entryPath: " C:\\entry.md " }), {
    rj_code: "rj01234567", game_version: "1.2", detect_id: true, entry_path: "C:\\entry.md",
  });
  assert.deepEqual(patchPublishFields({ rjCode: "", gameVersion: "", detectId: true, createEntry: false, entryPath: "" }), { detect_id: true });
});
