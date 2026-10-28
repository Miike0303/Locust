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
  UnencodableTranslation: "validate.kind.unencodableLabel",
  EmptyTranslation: "validate.kind.emptyLabel",
  IdenticalToSource: "validate.kind.identicalLabel",
  InconsistentTranslation: "validate.kind.inconsistentLabel",
  StaleTranslation: "validate.kind.staleLabel",
  InvalidInjectionProvenance: "validate.kind.provenanceLabel",
} as const satisfies Record<ValidationKindName, MessageKey>;

const MESSAGE_KEYS = {
  "translation is empty": "validate.message.empty",
  "translation is identical to source": "validate.message.identical",
} as const satisfies Record<string, MessageKey>;

/** Only exact owned messages are localized; diagnostic/provenance details stay intact. */
export function localizeValidationMessage(message: string, t: TranslateFn): string {
  return Object.prototype.hasOwnProperty.call(MESSAGE_KEYS, message)
    ? t(MESSAGE_KEYS[message as keyof typeof MESSAGE_KEYS])
    : message;
}

/** Unknown kinds from newer backends remain readable without a missing-key lookup. */
export function validationBadgeLabel(kind: string, t: TranslateFn): string {
  return Object.prototype.hasOwnProperty.call(BADGE_LABEL_KEYS, kind)
    ? t(BADGE_LABEL_KEYS[kind as ValidationKindName])
    : kind;
}

/** Optional guidance for advisory kinds, shown on QA badges and filter chips. */
export function validationKindHint(kind: string, t: TranslateFn): string | undefined {
  return kind === "InconsistentTranslation" ? t("validate.kind.inconsistentHint") : undefined;
}
