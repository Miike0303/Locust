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

// Same order as localeCompare(b, "en", { numeric: true }), without rebuilding
// collation state per comparison (sorting a 100k-row file is several times faster).
const idOrder = new Intl.Collator("en", { numeric: true });

export function reviewContext<T extends ContextEntry>(current: T, candidates: readonly T[]): T[] {
  const locator = logicalScriptLocator(current);
  const rows = candidates.filter(row => row.file_path === current.file_path
    && logicalScriptLocator(row) === locator)
    .sort((a, b) => idOrder.compare(a.id, b.id));
  const index = rows.findIndex(row => row.id === current.id);
  return index < 0 ? [current] : rows.slice(Math.max(0, index - 3), index + 4);
}

export interface ReviewContextIndex<T extends ContextEntry> {
  lookup(current: T): T[];
}

// Build once from a completed file; selections only copy the visible neighbors.
export function buildReviewContextIndex<T extends ContextEntry>(candidates: readonly T[]): ReviewContextIndex<T> {
  const groups = new Map<string, T[]>();
  for (const row of candidates) {
    const key = JSON.stringify([row.file_path, logicalScriptLocator(row)]);
    const group = groups.get(key);
    if (group) group.push(row);
    else groups.set(key, [row]);
  }
  const positions = new Map<string, { rows: T[]; index: number }>();
  for (const rows of groups.values()) {
    rows.sort((a, b) => idOrder.compare(a.id, b.id));
    rows.forEach((row, index) => {
      const key = JSON.stringify([row.file_path, row.id]);
      // Match findIndex's first occurrence if the input contains duplicate ids.
      if (!positions.has(key)) positions.set(key, { rows, index });
    });
  }
  return {
    lookup(current) {
      const position = positions.get(JSON.stringify([current.file_path, current.id]));
      if (!position) return [current];
      const { rows, index } = position;
      return rows.slice(Math.max(0, index - 3), index + 4);
    },
  };
}
