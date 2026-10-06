/** Minimal localStorage surface, also accepted by preference test doubles. */
export type StringStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export function browserStorage(): StringStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function readStorageFlag(key: string, storage?: StringStorage | null): boolean {
  const target = storage === undefined ? browserStorage() : storage;
  if (!target) return false;
  try {
    return target.getItem(key) === "1";
  } catch {
    return false;
  }
}

export function writeStorageFlag(
  key: string,
  enabled: boolean,
  storage?: StringStorage | null,
): void {
  const target = storage === undefined ? browserStorage() : storage;
  if (!target) return;
  try {
    if (enabled) target.setItem(key, "1");
    else target.removeItem(key);
  } catch {
    /* Storage can be unavailable in private or restricted browser contexts. */
  }
}
