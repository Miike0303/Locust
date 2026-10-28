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

type MessageIssue = {
  kind: ValidationKind | string;
  kind_data?: ValidationKind | null;
  message: string;
};

/** Shared by live validation results and persisted QA rows. Never parse English text. */
export function validationMessage(issue: MessageIssue, t: TranslateFn): string {
  const kind = typeof issue.kind === "string" ? issue.kind_data ?? issue.kind : issue.kind;
  if (typeof kind === "string") {
    if (typeof issue.kind === "string" && issue.kind !== kind) return issue.message;
    switch (kind) {
      case "EmptyTranslation": return t("validate.message.empty");
      case "IdenticalToSource": return t("validate.message.identical");
      case "StaleTranslation": return t("validate.kind.staleDetail");
      // Provenance has no structured reason yet; preserve its diagnostic details.
      default: return issue.message;
    }
  }
  if (!kind || typeof kind !== "object" || Array.isArray(kind)) return issue.message;
  const entries = Object.entries(kind);
  if (entries.length !== 1) return issue.message;
  const [name, data] = entries[0];
  if (typeof issue.kind === "string" && issue.kind !== name) return issue.message;
  if (!data || typeof data !== "object" || Array.isArray(data)) return issue.message;
  const isCount = (value: unknown): value is number =>
    typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
  switch (name) {
    case "MissingPlaceholder":
    case "ExtraPlaceholder":
      if (typeof data.placeholder === "string") {
        return t(name === "MissingPlaceholder" ? "validate.kind.missing" : "validate.kind.extra", { placeholder: data.placeholder });
      }
      break;
    case "ExceedsCharLimit":
      if (isCount(data.actual) && isCount(data.limit)) {
        return t("validate.kind.charLimit", { actual: data.actual, limit: data.limit });
      }
      break;
    case "ExceedsBinarySlot":
      if (isCount(data.actual) && isCount(data.limit) && typeof data.encoding === "string") {
        return t("validate.kind.binarySlot", { actual: data.actual, limit: data.limit, encoding: data.encoding });
      }
      break;
    case "UnencodableTranslation":
      if (typeof data.encoding === "string") return t("validate.kind.unencodable", { encoding: data.encoding });
      break;
    case "InconsistentTranslation":
      if (isCount(data.variants) && typeof data.common === "string") {
        return t("validate.message.inconsistent", { n: data.variants, text: data.common });
      }
      break;
  }
  return issue.message;
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
