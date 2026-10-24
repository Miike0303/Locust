import assert from "node:assert/strict";
import { test } from "node:test";
import { placeholderCheck, placeholderParts } from "./placeholders.ts";

const tokens = (text: string, format = "renpy") => placeholderParts(text, format)
  .filter(part => part.token).map(part => part.text);

test("detector ports every Rust placeholder kind and preserves source spans", () => {
  const expected = [String.raw`\g`, String.raw`\V[1]`, String.raw`\N[2]`, String.raw`\C[3]`, String.raw`\p[4]`, String.raw`\i[5]`,
    ...["$", ".", "!", ">", "<", "^", "|", "n", "t"].map(code => `\\${code}`),
    "<b>", "</b>", "%s", "%d", "%i", "%f", "%1", "%(name)g", "{}", "{0}", "{name}", "{b}", "{/b}", "{color=#fff}",
    "[party[0].name]", "[items['name']]", '[items["name"]!q]', "[player.name!rq]", "{PL_0}"];
  const source = `Hello ${expected.join(" ")} world`;
  assert.deepEqual(tokens(source), expected);
  assert.equal(placeholderParts(source, "renpy").map(part => part.text).join(""), source);
});

test("detector respects Rust escaped delimiters and bounded interpolation grammar", () => {
  for (const literal of [String.raw`\\n`, String.raw`\\V[1]`, "[[player]", "{{name}", "%%s", "%%1", "{P}", "[call()]", "[a + b]", "[items['a' + name + 'b']]", "[items[key]]", "%10", "{custom=value}", "{name\n}", "{color=red\n}"]) {
    assert.deepEqual(tokens(literal), [], literal);
  }
  assert.deepEqual(tokens("[items[-12].name!q]"), ["[items[-12].name!q]"]);
  assert.deepEqual(tokens("[items['a]b']]"), ["[items['a]b']]"]);
});

test("RPG Maker profiles keep plain brackets translatable and engine escapes protected", () => {
  for (const format of ["rpgmaker-mv", "rpgmaker-mz", "rpgmaker-vxa", "wolf-rpg"]) {
    assert.deepEqual(tokens(String.raw`[Noble] \V[1] \N[1] \C[1] \g`, format),
      [String.raw`\V[1]`, String.raw`\N[1]`, String.raw`\C[1]`, String.raw`\g`]);
  }
  for (const format of ["renpy", "kirikiri", "tyrano", "future-engine", ""]) {
    assert.deepEqual(tokens("[Noble]", format), ["[Noble]"]);
  }
});

test("name-box exception requires an immediate unescaped known escape and keeps embedded controls", () => {
  for (const escape of [String.raw`\n`, String.raw`\nc`, String.raw`\nr`, String.raw`\N`, String.raw`\NC`, String.raw`\NR`, String.raw`\nC`, String.raw`\Nr`]) {
    assert.deepEqual(tokens(`${escape}<Guard>`), [escape]);
  }
  for (const prefix of [String.raw`\\n`, String.raw`\n `, "\n", String.raw`\fn`, String.raw`\nl`, String.raw`\ntl`, String.raw`\nonsense`, ""]) {
    assert.ok(tokens(`${prefix}<Guard>`).includes("<Guard>"), prefix);
  }
  assert.deepEqual(tokens(String.raw`\n<Lady \c[2]\n[1]><b>Hello</b>`),
    [String.raw`\n`, String.raw`\c[2]`, String.raw`\n[1]`, "<b>", "</b>"]);
});

test("overlapping detectors keep the first complete Rust token", () => {
  assert.deepEqual(tokens(String.raw`<b title="[name] %s"> \V[1]`), ['<b title="[name] %s">', String.raw`\V[1]`]);
});

test("placeholder comparison preserves occurrence counts and missing/extra encounter order", () => {
  assert.deepEqual(placeholderCheck(["%s", "%s", "{0}"], ["{0}", "%s", "%d", "%d"]),
    { missing: ["%s"], extra: ["%d", "%d"], present: 2 });
  assert.deepEqual(placeholderCheck(["%s", "{0}"], ["{0}", "%s"]), { missing: [], extra: [], present: 2 });
});
