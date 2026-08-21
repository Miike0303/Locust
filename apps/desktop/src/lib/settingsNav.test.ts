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
  { id: "providers", labelKey: "settings.nav.providers" },
  { id: "defaults", labelKey: "settings.nav.defaults" },
  { id: "appearance", labelKey: "settings.nav.appearance" },
  { id: "glossary", labelKey: "settings.nav.glossary" },
  { id: "history", labelKey: "settings.nav.history" },
  { id: "data", labelKey: "settings.nav.data" },
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
