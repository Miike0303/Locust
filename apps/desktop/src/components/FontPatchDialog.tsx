import { useEffect, useState } from "react";
import { FolderOpen, Loader2, X } from "lucide-react";
import { createFontPatch, getTranslationRuns, type FontPatchRequest } from "../lib/api";
import { useT } from "../lib/i18n";
import { readLastUsedTranslationPrefs } from "../lib/translationDefaults";
import { MODAL_BACKDROP_CLASS, MODAL_FOOTER_CLASS, modalPanelClass, useModalA11y } from "../lib/modalA11y";

type Props = { open: boolean; defaultGamePath: string; onClose: () => void; onUsePatch: (path: string, gamePath: string) => void };
const empty = (): FontPatchRequest => ({ game_path: "", source_font: "", target_path: "", language: "", output_path: "" });

export default function FontPatchDialog({ open, defaultGamePath, onClose, onUsePatch }: Props) {
  const t = useT();
  const [draft, setDraft] = useState<FontPatchRequest>(empty);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [output, setOutput] = useState("");
  const close = () => { if (!busy) onClose(); };
  const { dialogRef, dialogProps, titleProps } = useModalA11y({ open, onClose: close, ownEscape: true });
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setDraft({ ...empty(), game_path: defaultGamePath, language: readLastUsedTranslationPrefs().target ?? "" });
    setError(""); setOutput("");
    void getTranslationRuns().then(runs => {
      if (!cancelled && runs.length) setDraft(value => value.language ? value : { ...value, language: runs[0].target_lang });
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [open, defaultGamePath]);
  const update = (key: keyof FontPatchRequest, value: string) => { setDraft(current => ({ ...current, [key]: value })); setOutput(""); setError(""); };
  const choose = async (key: "game_path" | "source_font" | "base_patch" | "output_path") => {
    try {
      if (!("__TAURI_INTERNALS__" in window)) return;
      const dialog = await import("@tauri-apps/plugin-dialog");
      const selected = key === "output_path"
        ? await dialog.save({ title: t("fontPatch.output"), defaultPath: "locust-font-patch.zip", filters: [{ name: "ZIP", extensions: ["zip"] }] })
        : await dialog.open({ directory: key === "game_path", multiple: false, filters: key === "game_path" ? undefined : [{ name: key === "source_font" ? "TTF / OTF" : "ZIP", extensions: key === "source_font" ? ["ttf", "otf"] : ["zip"] }] });
      if (typeof selected === "string") update(key, selected);
    } catch (err) { setError(String(err)); }
  };
  const ready = [draft.game_path, draft.source_font, draft.target_path, draft.language, draft.output_path].every(value => value.trim());
  const submit = async () => {
    setBusy(true); setError(""); setOutput("");
    try {
      const result = await createFontPatch({ ...draft, base_patch: draft.base_patch?.trim() || undefined });
      setOutput(result.output_path);
    } catch (err) { setError(err instanceof Error ? err.message : String(err)); }
    finally { setBusy(false); }
  };
  if (!open) return null;
  const fields = [
    ["game_path", "fontPatch.game", true], ["source_font", "fontPatch.source", true],
    ["target_path", "fontPatch.target", false], ["language", "fontPatch.language", false],
    ["base_patch", "fontPatch.base", true], ["output_path", "fontPatch.output", true],
  ] as const;
  return <div className={MODAL_BACKDROP_CLASS}>
    <div ref={dialogRef} {...dialogProps} className={modalPanelClass("max-w-2xl max-h-[90vh] flex flex-col")}>
      <div className="flex items-center justify-between px-5 py-4 border-b border-gray-200 dark:border-gray-700">
        <h2 {...titleProps} className="text-lg font-semibold">{t("fontPatch.title")}</h2>
        <button onClick={close} disabled={busy} aria-label={t("common.close")}><X size={20} /></button>
      </div>
      <div className="px-5 py-4 space-y-3 overflow-y-auto">
        <p className="text-sm text-gray-600 dark:text-gray-400">{t("fontPatch.description")}</p>
        <p className="text-xs text-gray-600 dark:text-gray-400">{t("fontPatch.combineNote")}</p>
        <fieldset disabled={busy} className="space-y-3">
          {fields.map(([key, label, browse]) => <label key={key} className="block text-sm font-medium">
            {t(label)}
            <span className="flex gap-2 mt-1">
              <input className="flex-1 min-w-0 border border-gray-300 dark:border-gray-600 rounded px-2 py-1.5 bg-white dark:bg-gray-800 font-normal" value={draft[key] ?? ""} onChange={e => update(key, e.target.value)} placeholder={key === "target_path" ? "fonts/gamefont.ttf" : key === "language" ? "es / zh-CN / …" : undefined} />
              {browse && "__TAURI_INTERNALS__" in window && <button type="button" onClick={() => void choose(key as "game_path" | "source_font" | "base_patch" | "output_path")} className="border rounded px-2" aria-label={t("fontPatch.choose", { field: t(label) })}><FolderOpen size={16} /></button>}
            </span>
          </label>)}
        </fieldset>
        <p className="text-xs text-gray-500">{t("fontPatch.license")}</p>
        {error && <p role="alert" className="text-sm text-red-700 dark:text-red-300 break-words">{error}</p>}
        {output && <div role="status" className="text-sm text-emerald-800 dark:text-emerald-300 break-all"><p>{t("fontPatch.created")}</p><p>{output}</p></div>}
      </div>
      <div className={MODAL_FOOTER_CLASS}>
        {output ? <button className="px-4 py-2 rounded bg-emerald-600 text-white" onClick={() => onUsePatch(output, draft.game_path)}>{t("fontPatch.use")}</button>
          : <button disabled={!ready || busy} onClick={() => void submit()} className="flex items-center gap-2 px-4 py-2 rounded bg-emerald-600 text-white disabled:opacity-50">{busy && <Loader2 size={16} className="animate-spin" />}{t("fontPatch.create")}</button>}
        <button onClick={close} disabled={busy} className="px-4 py-2 rounded bg-gray-100 dark:bg-gray-800">{t("common.close")}</button>
      </div>
    </div>
  </div>;
}
