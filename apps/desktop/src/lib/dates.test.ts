import assert from "node:assert/strict";
import { test } from "node:test";
import { formatDateTime } from "./dates.ts";

const iso = "2026-03-04T05:06:07Z";
const short: Intl.DateTimeFormatOptions = { year: "numeric", month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit" };

for (const locale of ["en", "es"] as const) {
  test(`formatDateTime uses the ${locale} interface language, not the system locale`, () => {
    assert.equal(formatDateTime(iso, locale), new Date(iso).toLocaleString(locale));
    assert.equal(formatDateTime(iso, locale, short), new Date(iso).toLocaleString(locale, short));
  });
}

test("formatDateTime renders English and Spanish differently for the same instant", () => {
  assert.notEqual(formatDateTime(iso, "en", short), formatDateTime(iso, "es", short));
});

test("formatDateTime keeps a readable fallback for missing or unparsable values", () => {
  assert.equal(formatDateTime("", "en"), "—");
  assert.equal(formatDateTime("not a date", "es"), "not a date");
  assert.equal(formatDateTime("2026-99-99 99:99:99 local", "en"), "2026-99-99 99:99");
});
