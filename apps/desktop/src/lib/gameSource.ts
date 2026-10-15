import type { PluginInfo } from "./api";

/** Retain executable and editor-project shortcuts alongside native formats. */
export function gameFileExtensions(formats: PluginInfo[] = []): string[] {
  return [...new Set([
    "exe", "html", "htm", "rpy", "rpa", "rpgproject", "rvproj2",
    ...formats.filter(f => f.stability !== "comingsoon")
      .flatMap(f => f.extensions.map(ext => ext.replace(/^\./, "").toLowerCase())),
  ])];
}

export type GameSourceKind = "file" | "directory";
// This is a source-selection constraint, independent of injection capabilities.
export function formatSourceKind(formatId?: string): GameSourceKind | undefined {
  return formatId === "godot-dialogic" ? "file" : formatId ? "directory" : undefined;
}

let pending = false;
let settle: ((kind: GameSourceKind | null) => void) | undefined;
const listeners = new Set<() => void>();
export const gameSourceChoicePending = () => pending;
export const subscribeGameSourceChoice = (listener: () => void) => {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
};
export function resolveGameSourceChoice(kind: GameSourceKind | null): void {
  settle?.(kind);
}
export function requestGameSourceChoice(): Promise<GameSourceKind | null> {
  if (pending) return Promise.resolve(null);
  return new Promise(resolve => {
    pending = true;
    settle = kind => {
      pending = false;
      settle = undefined;
      listeners.forEach(listener => listener());
      resolve(kind);
    };
    listeners.forEach(listener => listener());
  });
}
