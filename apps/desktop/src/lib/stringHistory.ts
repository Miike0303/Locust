import type { StringStatus } from "./api";
import type { MessageKey, TranslateFn } from "./i18n";

const statusLabels = {
  pending: "detail.status.pending",
  translated: "detail.status.translated",
  reviewed: "detail.status.reviewed",
  approved: "detail.status.approved",
  error: "detail.status.error",
} as const satisfies Record<StringStatus, MessageKey>;

export function historyStatusLabel(status: string | null, t: TranslateFn): string {
  if (status === null) return "—";
  return Object.prototype.hasOwnProperty.call(statusLabels, status)
    ? t(statusLabels[status as StringStatus]) : status;
}

export function canRestoreRevision(translation: string | null, current: string | null): translation is string {
  return translation !== null && translation !== current;
}
