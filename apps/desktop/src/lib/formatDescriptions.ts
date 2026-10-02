import { en } from "./i18n/en";
import type { MessageKey } from "./i18n";

/** Format ids that ship a user-facing, translated description. The backend
 * `description` is developer documentation (codecs, XOR keys, test notes) and
 * stays only as the fallback for a format the UI does not know yet. */
export const KNOWN_FORMAT_IDS = [
  "renpy",
  "rpgmaker-mv",
  "rpgmaker-vxa",
  "html-game",
  "kirikiri",
  "nscripter",
  "qsp",
  "sugarcube",
  "tyrano",
  "unity",
  "unreal",
  "vntextpatch",
  "wolf-rpg",
  "yuris",
] as const;

export function formatDescriptionKey(formatId: string): MessageKey | null {
  const key = `format.desc.${formatId}`;
  return key in en ? (key as MessageKey) : null;
}
