import type { ValidationKind } from "./api";
import type { MessageKey, TranslateFn } from "./i18n";

// Distribute over the externally tagged union so adding a known kind requires a label.
type KindName<T> = T extends string ? T : keyof T;
type ValidationKindName = KindName<ValidationKind>;

const BADGE_LABEL_KEYS = {
  MissingPlaceholder: "validate.kind.missingLabel",
  ExtraPlaceholder: "validate.kind.extraLabel",
  ExceedsCharLimit: "validate.kind.charLimitLabel",
  ExceedsBinarySlot: "validate.kind.binarySlotLabel",
  EmptyTranslation: "validate.kind.emptyLabel",
  IdenticalToSource: "validate.kind.identicalLabel",
  StaleTranslation: "validate.kind.staleLabel",
  InvalidInjectionProvenance: "validate.kind.provenanceLabel",
} as const satisfies Record<ValidationKindName, MessageKey>;

/** Unknown kinds from newer backends remain readable without a missing-key lookup. */
export function validationBadgeLabel(kind: string, t: TranslateFn): string {
  return Object.prototype.hasOwnProperty.call(BADGE_LABEL_KEYS, kind)
    ? t(BADGE_LABEL_KEYS[kind as ValidationKindName])
    : kind;
}
