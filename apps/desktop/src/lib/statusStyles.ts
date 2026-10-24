import { createElement } from "react";
import { Check } from "lucide-react";
import type { StringStatus } from "./api";
import { useT, type MessageKey } from "./i18n";

interface StatusStyle {
  labelKey: MessageKey;
  text: string;
  dot: string;
  chip: string;
  bar: string;
  check: boolean;
}

/** One semantic palette for string statuses across all workspaces. */
export const statusStyles = {
  pending: {
    labelKey: "detail.status.pending", text: "text-text-muted",
    dot: "bg-transparent ring-1 ring-inset ring-text-muted",
    chip: "bg-surface-muted", bar: "bg-border", check: false,
  },
  translated: {
    labelKey: "detail.status.translated", text: "text-accent-fg",
    dot: "bg-transparent ring-1 ring-inset ring-accent",
    chip: "bg-accent-muted", bar: "bg-accent/60", check: false,
  },
  reviewed: {
    labelKey: "detail.status.reviewed", text: "text-accent-fg",
    dot: "bg-accent", chip: "bg-accent-muted", bar: "bg-accent", check: false,
  },
  approved: {
    labelKey: "detail.status.approved", text: "text-success",
    dot: "bg-success", chip: "bg-success-muted", bar: "bg-success", check: true,
  },
  error: {
    labelKey: "detail.status.error", text: "text-danger",
    dot: "bg-danger", chip: "bg-danger-muted", bar: "bg-danger", check: false,
  },
} as const satisfies Record<StringStatus, StatusStyle>;

/** Keep labels visible; hollow/filled markers and the approval check add shape cues. */
export function StatusBadge({ status, labelKey, className = "" }: {
  status: string;
  labelKey?: MessageKey;
  className?: string;
}) {
  const t = useT();
  const known = Object.prototype.hasOwnProperty.call(statusStyles, status);
  const style = known ? statusStyles[status as StringStatus] : statusStyles.pending;
  return createElement("span", {
    className: `inline-flex items-center gap-1.5 whitespace-nowrap text-caption font-medium ${style.text} ${className}`.trim(),
  }, createElement("span", {
    "aria-hidden": true,
    className: `inline-flex size-3 shrink-0 items-center justify-center rounded-full ${style.dot}`,
  }, style.check ? createElement(Check, { size: 10, strokeWidth: 3, className: "text-surface" }) : null),
  labelKey ? t(labelKey) : known ? t(style.labelKey) : status);
}
