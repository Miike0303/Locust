/**
 * Lightweight asserts for settingsNav (run: npx --yes tsx src/lib/settingsNav.test.ts).
 */
import {
  SETTINGS_SECTIONS,
  buildSettingsPath,
  operationalShortcutTarget,
  parseSettingsSectionParam,
} from "./settingsNav.ts";

const assert = {
  equal(actual: unknown, expected: unknown, message?: string) {
    if (actual !== expected) throw new Error(message ?? `${actual} !== ${expected}`);
  },
  deepEqual(actual: unknown, expected: unknown, message?: string) {
    if (JSON.stringify(actual) !== JSON.stringify(expected)) {
      throw new Error(message ?? "values differ");
    }
  },
};

assert.deepEqual(SETTINGS_SECTIONS, [
  { id: "providers", label: "Providers" },
  { id: "defaults", label: "Translation Defaults" },
  { id: "appearance", label: "Appearance" },
  { id: "glossary", label: "Glossary" },
  { id: "history", label: "History" },
  { id: "data", label: "Data" },
]);

assert.equal(parseSettingsSectionParam("?section=glossary"), "glossary");
assert.equal(parseSettingsSectionParam("?section=unknown"), "providers");
assert.equal(parseSettingsSectionParam(""), "providers");

assert.equal(buildSettingsPath("data"), "/settings?section=data");
for (const { id } of SETTINGS_SECTIONS) {
  const path = buildSettingsPath(id);
  assert.equal(new URL(path, "https://locust.invalid").searchParams.get("section"), id);
}

assert.deepEqual(operationalShortcutTarget("provider-settings"), {
  section: "providers",
  path: "/settings?section=providers",
});
assert.deepEqual(operationalShortcutTarget("manage-glossary"), {
  section: "glossary",
  path: "/settings?section=glossary",
});
assert.deepEqual(operationalShortcutTarget("manage-backups"), {
  section: "data",
  path: "/settings?section=data",
});

console.log("settingsNav.test.ts: ok");
