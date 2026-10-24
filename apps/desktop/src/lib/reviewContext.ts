type ContextEntry = { id: string; file_path: string };

// RPG Maker MV/MZ event commands, as built by crates/formats/src/rpgmaker_mv.rs:
// maps "<File>.json#0#event_<n>#page_<n>#cmd_<n>[#suffix]", troops and common
// events "<File>.json#<index>#page_<n>#cmd_<n>[#suffix]". Anchored on the whole
// id so archive member names that merely contain "#cmd_" are not misread.
const RPG_COMMAND = /^([^#]+\.json)#\d+#(?:event_\d+#)?page_\d+#cmd_\d+(?:#[^#]+)?$/;

function logicalScriptLocator(entry: ContextEntry): string {
  // The whole data file is one script; index/event/page/command order numerically.
  const rpg = RPG_COMMAND.exec(entry.id);
  if (rpg) return rpg[1];
  // Ren'Py archive ids retain the member path before the final #sN position.
  const position = entry.id.lastIndexOf("#");
  return position < 0 ? entry.file_path : entry.id.slice(0, position);
}

export function reviewContext<T extends ContextEntry>(current: T, candidates: readonly T[]): T[] {
  const locator = logicalScriptLocator(current);
  const rows = candidates.filter(row => row.file_path === current.file_path
    && logicalScriptLocator(row) === locator)
    .sort((a, b) => a.id.localeCompare(b.id, "en", { numeric: true }));
  const index = rows.findIndex(row => row.id === current.id);
  return index < 0 ? [current] : rows.slice(Math.max(0, index - 3), index + 4);
}
