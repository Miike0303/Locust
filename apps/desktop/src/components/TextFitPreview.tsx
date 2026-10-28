import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { estimateTextFit, type StringEntry, type TextFitRequest, type TextFitResponse, type TextFitLine } from "../lib/api";
import { useT, type TranslateFn } from "../lib/i18n";
import { IS_TAURI } from "../lib/runtime";
import { browserStorage, type StringStorage } from "../lib/storage";
import { draftProjectKey, isProjectScopedQueryEnabled } from "../lib/projectScope";
import { useProjectStore } from "../stores/projectStore";

export interface TextFitProfile { fontPath: string; fontSize: string; usableWidth: string }
const emptyProfile = (): TextFitProfile => ({ fontPath: "", fontSize: "", usableWidth: "" });
const profileKey = (projectKey: string) => `locust.textFit.v1:${projectKey}`;

export function isTextFitEligible(entry: StringEntry): boolean {
  return entry.metadata?.format_id === "rpgmaker-mv" && entry.tags.includes("dialogue") && !entry.tags.includes("scroll_text")
    && /#cmd_[0-9]+#msg$/.test(entry.id);
}

export function loadTextFitProfile(projectKey: string, storage: StringStorage | null = browserStorage()): TextFitProfile {
  try {
    const value: unknown = JSON.parse(storage?.getItem(profileKey(projectKey)) ?? "null");
    if (value && typeof value === "object") {
      const record = value as Record<string, unknown>;
      return { fontPath: typeof record.fontPath === "string" ? record.fontPath : "",
        fontSize: typeof record.fontSize === "string" ? record.fontSize : "",
        usableWidth: typeof record.usableWidth === "string" ? record.usableWidth : "" };
    }
  } catch { /* Missing, restricted or malformed storage leaves every input explicit. */ }
  return emptyProfile();
}

export function persistTextFitProfile(projectKey: string, profile: TextFitProfile, storage: StringStorage | null = browserStorage()): void {
  try { storage?.setItem(profileKey(projectKey), JSON.stringify(profile)); }
  catch { /* Storage failure must not prevent a read-only estimate. */ }
}

export function textFitRequest(entryId: string, draft: string, profile: TextFitProfile): TextFitRequest | null {
  const size = Number(profile.fontSize), width = Number(profile.usableWidth);
  if (!profile.fontPath.trim() || !profile.fontSize.trim() || !profile.usableWidth.trim()
    || !Number.isFinite(size) || size <= 0 || !Number.isFinite(width) || width <= 0) return null;
  return { entry_id: entryId, draft_text: draft, font_path: profile.fontPath.trim(), font_size_px: size, usable_width_px: width };
}

const reasonKeys = {
  no_text: "textFit.noText", font_not_selected: "textFit.reason.fontNotSelected", font_read_failed: "textFit.reason.fontReadFailed",
  unsupported_font_format: "textFit.reason.fontFormat", font_changed_during_read: "textFit.reason.fontChanged",
  invalid_font_size: "textFit.reason.fontSize", invalid_usable_width: "textFit.reason.width", font_too_large: "textFit.reason.fontTooLarge",
  font_collection: "textFit.reason.collection", invalid_font: "textFit.reason.invalidFont", variable_font: "textFit.reason.variableFont",
  missing_metrics: "textFit.reason.metrics", backslash_control: "textFit.reason.backslash", tab: "textFit.reason.tab",
  inline_markup: "textFit.reason.markup", unsupported_character: "textFit.reason.character", missing_glyph: "textFit.reason.glyph",
  nonfinite_advance: "textFit.reason.advance", input_limit: "textFit.reason.inputLimit",
} as const;

export function textFitReason(reason: string | null, t: TranslateFn): string {
  if (!reason) return t("textFit.unknownReason");
  const separator = reason.indexOf(":");
  const code = separator < 0 ? reason : reason.slice(0, separator);
  const key = reasonKeys[code as keyof typeof reasonKeys];
  if (!key) return reason;
  const detail = separator < 0 ? "" : reason.slice(separator + 1).trim();
  const diagnostic = code === "font_read_failed" || code === "invalid_font";
  const suffix = diagnostic ? detail : detail.match(/\bU\+[0-9A-Fa-f]{4,6}\b/g)?.join(", ");
  return `${t(key)}${suffix ? `: ${suffix}` : ""}`;
}

export function textFitRequestError(message: string, t: TranslateFn): string {
  const reason = message.replace(/^\d{3}:\s*/, "");
  return reason.split(":", 1)[0] === "input_limit" ? textFitReason(reason, t) : message;
}

interface EstimateOptions {
  projectKey: string; entry: StringEntry; draft: string; profile: TextFitProfile; enabled: boolean; active: boolean;
}
type Result = { key: string; generation: number; data?: TextFitResponse; error?: string };

export function useTextFitEstimate({ projectKey, entry, draft, profile, enabled, active }: EstimateOptions) {
  const confirmed = useProjectStore(state => !state.projectTransitionInProgress && state.confirmedOpenProjectKey === projectKey);
  const body = textFitRequest(entry.id, draft, profile);
  const allowed = enabled && active && confirmed && isTextFitEligible(entry) && body !== null;
  const key = JSON.stringify([projectKey, entry.id, entry.source, draft, profile, allowed]);
  const identity = useRef({ key, generation: 0 });
  const pending = useRef<{ controller: AbortController; key: string } | null>(null);
  // Invalidate during render: effect cleanup alone can expose a completed old
  // request for one frame when row, project, source, draft or profile changes.
  if (identity.current.key !== key) {
    identity.current = { key, generation: identity.current.generation + 1 };
    pending.current?.controller.abort();
    pending.current = null;
  }
  const generation = identity.current.generation;
  const [result, setResult] = useState<Result | null>(null);
  useEffect(() => {
    if (!allowed || !body) return;
    const ticket = { controller: new AbortController(), key };
    pending.current = ticket;
    const owns = () => pending.current === ticket && identity.current.key === key
      && identity.current.generation === generation && !ticket.controller.signal.aborted && isProjectScopedQueryEnabled(projectKey);
    const timer = setTimeout(() => {
      if (!owns()) return;
      void estimateTextFit(body, ticket.controller.signal).then(data => {
        if (owns()) setResult({ key, generation, data });
      }).catch(error => {
        if (owns()) setResult({ key, generation, error: error instanceof Error ? error.message : String(error) });
      });
    }, 250);
    return () => {
      clearTimeout(timer);
      ticket.controller.abort();
      if (pending.current === ticket) pending.current = null;
    };
  }, [key]); // The key includes the complete request and its ownership context.
  const visible = allowed && result?.key === key && result.generation === generation ? result : null;
  return { configured: body !== null, data: visible?.data, error: visible?.error,
    loading: allowed && !visible, active: allowed };
}

interface Props { projectKey: string; entry: StringEntry; draft: string }

export default function TextFitPreview({ projectKey, entry, draft }: Props) {
  const t = useT();
  const active = useProjectStore(state => !!state.project && draftProjectKey(state.project) === projectKey
    && !state.projectTransitionInProgress && state.confirmedOpenProjectKey === projectKey);
  const [expanded, setExpanded] = useState(false);
  const [savedProfile, setSavedProfile] = useState(() => ({ projectKey, value: loadTextFitProfile(projectKey) }));
  const profile = savedProfile.projectKey === projectKey ? savedProfile.value : loadTextFitProfile(projectKey);
  const pickerOwner = useRef({ projectKey, revision: 0 });
  if (pickerOwner.current.projectKey !== projectKey) pickerOwner.current = { projectKey, revision: pickerOwner.current.revision + 1 };
  const [pickerError, setPickerError] = useState<{ projectKey: string; message: string } | null>(null);
  useEffect(() => () => { pickerOwner.current.revision++; }, []);
  const estimate = useTextFitEstimate({ projectKey, entry, draft, profile, enabled: expanded, active });
  const update = (field: keyof TextFitProfile, value: string) => {
    pickerOwner.current.revision++;
    const next = { ...profile, [field]: value };
    setSavedProfile({ projectKey, value: next });
    persistTextFitProfile(projectKey, next);
    setPickerError(null);
  };
  const chooseFont = async () => {
    const owner = pickerOwner.current, revision = ++owner.revision;
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({ multiple: false, filters: [{ name: "TTF / OTF", extensions: ["ttf", "otf"] }] });
      if (pickerOwner.current === owner && owner.revision === revision && typeof selected === "string") update("fontPath", selected);
    } catch (error) {
      if (pickerOwner.current === owner && owner.revision === revision) setPickerError({ projectKey, message: String(error) });
    }
  };
  if (!isTextFitEligible(entry) || !active) return null;
  const id = `text-fit-${encodeURIComponent(entry.id)}`;
  const lines = (label: string, values: TextFitLine[], text: string) => <div className="space-y-1.5">
    <h4 className="font-medium">{label}</h4>
    {values.map((line, index) => <div key={index} className="space-y-1">
      <p className={line.status === "exceeds_width" ? "text-warning" : "text-text-muted"}>
        {t("textFit.line", { line: index + 1 })}: {line.status === "unavailable" || line.advance_px === null
          ? `${t("textFit.unavailable")}: ${textFitReason(line.reason, t)}`
          : <>{line.advance_px.toLocaleString(undefined, { maximumFractionDigits: 3 })} px / {line.usable_width_px} px · {line.reason === "no_text" || text.length === 0 ? t("textFit.noText") : t(line.status === "exceeds_width" ? "textFit.exceeds" : "textFit.within")}</>}
      </p>
      {line.status !== "unavailable" && line.advance_px !== null && line.reason !== "no_text" && text.length > 0 && <div aria-hidden="true" className="h-1 rounded bg-surface-muted overflow-hidden">
        <div className={`h-full ${line.status === "exceeds_width" ? "bg-warning" : "bg-text-muted"}`}
          style={{ width: `${Math.min(100, Math.max(0, line.advance_px / line.usable_width_px * 100))}%` }} />
      </div>}
    </div>)}
  </div>;
  return <section data-text-fit-preview="" className="text-caption text-text space-y-2">
    <button type="button" aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded(value => !value)}
      className="flex items-center gap-1 text-text-muted hover:text-text">
      {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}{t("textFit.title")}
    </button>
    {expanded && <div id={id} className="space-y-2">
      <p className="text-text-muted">{t("textFit.linesHint")}</p>
      <label className="block" htmlFor={`${id}-font`}>{t("textFit.font")}</label>
      <div className="flex gap-1">
        <input id={`${id}-font`} value={profile.fontPath} onChange={event => update("fontPath", event.target.value)}
          className="min-w-0 w-full p-1 border border-border rounded bg-surface" />
        {IS_TAURI && <button type="button" onClick={chooseFont} className="underline">{t("textFit.browse")}</button>}
      </div>
      <div className="grid grid-cols-2 gap-2">
        <label htmlFor={`${id}-size`}>{t("textFit.size")}
          <input id={`${id}-size`} type="number" min="0" step="any" value={profile.fontSize}
            onChange={event => update("fontSize", event.target.value)} className="mt-1 w-full p-1 border border-border rounded bg-surface" />
        </label>
        <label htmlFor={`${id}-width`}>{t("textFit.width")}
          <input id={`${id}-width`} type="number" min="0" step="any" value={profile.usableWidth}
            onChange={event => update("usableWidth", event.target.value)} className="mt-1 w-full p-1 border border-border rounded bg-surface" />
        </label>
      </div>
      {!estimate.configured && <p className="text-text-muted">{t("textFit.configure")}</p>}
      {pickerError?.projectKey === projectKey && <p role="status" className="text-text-muted">{t("textFit.unavailable")}: {pickerError.message}</p>}
      {estimate.loading && <p role="status" className="text-text-muted">{t("textFit.loading")}</p>}
      {estimate.error && <p role="status" className="text-text-muted">{t("textFit.unavailable")}: {textFitRequestError(estimate.error, t)}</p>}
      {estimate.data && <div className="space-y-3" aria-live="polite">
        {lines(t("textFit.source"), estimate.data.source, entry.source)}
        {lines(t("textFit.draft"), estimate.data.draft, draft)}
      </div>}
      <p className="text-text-muted">{t("textFit.limits")}</p>
    </div>}
  </section>;
}
