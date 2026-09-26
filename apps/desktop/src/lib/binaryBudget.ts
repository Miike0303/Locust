import type { StringEntry } from "./api";

/** Display hints only; the Rust validator remains authoritative for injection. */
export function encodedByteLen(encoding: string, text: string): number | null {
  switch (encoding) {
    case "utf8": return new TextEncoder().encode(text).length;
    case "utf16le": return text.length * 2;
    default: return null;
  }
}

export function binaryBudgetHint(entry: StringEntry): {
  encoding: string | null; capacity: number | null; grouped: boolean; expandable: boolean;
} {
  const meta = entry.metadata ?? {};
  const method = meta.extraction_method;
  const grouped = method === "textasset_loc_line" || method === "textasset_csv_cell";
  const version = meta.unity_serialized_version;
  const expandable = (grouped || method === "textasset")
    && meta.textasset_rewrite === "serialized-v1"
    && typeof version === "number" && Number.isInteger(version) && version >= 17 && version <= 22;
  const encoding = typeof meta.binary_slot === "string" ? meta.binary_slot : expandable ? "utf8" : null;
  const unknown = { encoding, capacity: null, grouped, expandable };
  // A cell does not own its entire TextAsset budget. Show the group hint and
  // let Validate reconstruct the full source with all translated cells.
  if (grouped || !encoding) return unknown;
  const physical = meta.locust_injection_source ?? entry.source;
  if (typeof physical !== "string") return unknown;
  const sourceBytes = encodedByteLen(encoding, physical);
  if (sourceBytes === null) return unknown;
  const explicit = meta.locust_injection_capacity;
  if (explicit !== undefined) {
    if (!explicit || typeof explicit !== "object" || Array.isArray(explicit)) return unknown;
    const { encoding: immutableEncoding, bytes } = explicit as Record<string, unknown>;
    if (immutableEncoding !== encoding || bytes !== sourceBytes || sourceBytes <= 0) return unknown;
  }
  return { encoding, capacity: expandable && encoding === "utf8" ? 1_048_576 : sourceBytes, grouped, expandable };
}
