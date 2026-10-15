import { useSyncExternalStore } from "react";
import { gameSourceChoicePending, subscribeGameSourceChoice, resolveGameSourceChoice } from "../lib/gameSource";
import { useModalA11y, MODAL_BACKDROP_CLASS, modalPanelClass } from "../lib/modalA11y";
import { useT } from "../lib/i18n";

export default function GameSourceDialog() {
  const t = useT();
  const open = useSyncExternalStore(subscribeGameSourceChoice, gameSourceChoicePending, () => false);
  const { dialogRef, dialogProps, titleProps } = useModalA11y({ open, ownEscape: true, onClose: () => resolveGameSourceChoice(null) });
  if (!open) return null;
  return <div className={MODAL_BACKDROP_CLASS}>
    <div ref={dialogRef} {...dialogProps} className={modalPanelClass("max-w-md p-5")}>
      <h2 {...titleProps} className="text-section font-semibold">{t("welcome.source.title")}</h2>
      <p className="my-3 text-body text-text-muted">{t("welcome.source.hint")}</p>
      <div className="flex gap-2">
        <button className="px-3 py-2 rounded bg-surface-muted hover:bg-accent-muted" onClick={() => resolveGameSourceChoice("directory")}>{t("welcome.openFolder")}</button>
        <button className="px-3 py-2 rounded bg-surface-muted hover:bg-accent-muted" onClick={() => resolveGameSourceChoice("file")}>{t("welcome.openFile")}</button>
        <button className="px-3 py-2 rounded hover:bg-surface-muted" onClick={() => resolveGameSourceChoice(null)}>{t("common.cancel")}</button>
      </div>
    </div>
  </div>;
}
