import { useEffect, useId, useRef, useState } from "react";
import { useT } from "../lib/i18n";

/** Shared by buffered fields and the Settings page's inputs/selects. */
export const SETTINGS_INPUT_CLASS = "w-full min-w-0 rounded-md border border-border bg-surface p-2 text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg";

/** Keep intermediate typing local; persist a text edit on blur/Enter and a
 * slider edit on release. Responses cannot replace a newer local edit. */
export default function BufferedSetting({
  label, value, onSave, type = "text", min, max, step, required, placeholder, normalize = value => value,
}: {
  label: string | ((draft: string) => string);
  value: string | number;
  onSave: (value: string) => Promise<boolean>;
  type?: "text" | "number" | "range";
  min?: number; max?: number; step?: number; required?: boolean; placeholder?: string;
  normalize?: (value: string) => string;
}) {
  const t = useT();
  const id = useId();
  const [draft, setDraft] = useState(String(value));
  const [saving, setSaving] = useState(0);
  const [invalid, setInvalid] = useState(false);
  const draftRef = useRef(draft);
  const serverValue = useRef(String(value));
  const input = useRef<HTMLInputElement>(null);
  const active = useRef(true);
  const pending = useRef(0);
  const lastScheduled = useRef<string | null>(null);

  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);
  useEffect(() => {
    const next = String(value);
    // A refresh of another setting must not discard unfinished typing.
    if (pending.current === 0 && draftRef.current === serverValue.current) {
      draftRef.current = next;
      setDraft(next);
    }
    serverValue.current = next;
  }, [value]);

  const commit = async () => {
    const candidate = normalize(draftRef.current);
    if (!input.current?.checkValidity() || (required && !candidate.trim())) {
      setInvalid(true);
      return;
    }
    if (candidate !== draftRef.current) {
      draftRef.current = candidate;
      setDraft(candidate);
    }
    if (pending.current === 0 && candidate === serverValue.current) return;
    if (pending.current > 0 && candidate === lastScheduled.current) return;
    // A → B → A while saving must enqueue the final A, even if the server still
    // has its original A. updateConfig serializes these writes across sections.
    pending.current += 1;
    lastScheduled.current = candidate;
    setSaving(pending.current);
    try {
      await onSave(candidate);
    } finally {
      pending.current -= 1;
      if (pending.current === 0) lastScheduled.current = null;
      if (active.current) setSaving(pending.current);
    }
  };
  const dirty = draft !== String(value);
  return <div>
    <label htmlFor={id} className="text-body font-medium">{typeof label === "function" ? label(draft) : label}</label>
    <input id={id} ref={input} type={type} value={draft} min={min} max={max} step={step} required={required} placeholder={placeholder}
      aria-invalid={invalid || undefined} aria-describedby={dirty || saving || invalid ? `${id}-status` : undefined}
      onChange={event => { draftRef.current = event.target.value; setDraft(event.target.value); setInvalid(false); }}
      onBlur={() => void commit()}
      onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); event.currentTarget.blur(); } }}
      onKeyUp={type === "range" ? () => void commit() : undefined}
      onPointerUp={type === "range" ? () => void commit() : undefined}
      className={type === "range" ? "mt-1 w-full accent-accent" : `mt-1 ${SETTINGS_INPUT_CLASS}`} />
    {(dirty || saving > 0 || invalid) && <p id={`${id}-status`} role="status" className="text-caption text-text-muted mt-1">
      {invalid ? t("settings.field.invalid") : saving ? t("settings.field.saving") : t("settings.field.unsaved")}
    </p>}
  </div>;
}
