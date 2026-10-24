import { useEffect, useId, useRef, useSyncExternalStore } from "react";
import {
  getProjectOpenChoice, resolveProjectOpenChoice, subscribeProjectOpenChoice,
  type ProjectOpenChoice, type ProjectOpenChoiceRequest,
} from "../lib/openProjectFlow";
import { useT } from "../lib/i18n";
import {
  useModalA11y, MODAL_BACKDROP_CLASS, MODAL_HEADER_CLASS, MODAL_BODY_CLASS, MODAL_FOOTER_CLASS, modalPanelClass,
} from "../lib/modalA11y";

interface Props {
  request: ProjectOpenChoiceRequest | null;
  onChoose: (choice: ProjectOpenChoice) => void;
}

/** No extraction until an explicit choice. Unverified evidence cannot resume. */
export default function ResumeProjectDialog({ request, onChoose }: Props) {
  const t = useT();
  const resumeRef = useRef<HTMLButtonElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const descriptionId = useId();
  const verified = request?.preflight.kind === "resume_available";
  const cancel = () => onChoose("cancel");
  const { dialogRef, dialogProps, titleProps } = useModalA11y({
    open: !!request,
    onClose: cancel,
    ownEscape: true,
    initialFocusRef: verified ? resumeRef : cancelRef,
  });
  if (!request) return null;

  return (
    <div className={MODAL_BACKDROP_CLASS} onClick={cancel}>
      <div ref={dialogRef} {...dialogProps} aria-describedby={descriptionId}
        className={modalPanelClass("max-w-lg")}
        onClick={(event) => event.stopPropagation()}>
        <header className={`${MODAL_HEADER_CLASS} px-5 pt-5 pb-2`}>
          <h2 {...titleProps} className="text-section font-bold">
            {t(verified ? "resume.title" : "resume.attentionTitle")}
          </h2>
        </header>
        <div id={descriptionId} className={`${MODAL_BODY_CLASS} px-5 pb-4 text-body text-text-muted space-y-3`}>
          <p>{t(verified ? "resume.message" : "resume.attentionMessage")}</p>
          <p className="break-all font-mono text-caption">{request.gamePath}</p>
          {request.preflight.kind === "needs_attention" && (
            <p className="whitespace-pre-wrap break-words text-warning">
              {request.preflight.reason}
            </p>
          )}
          <p>{t("resume.refreshWarning")}</p>
        </div>
        <footer className={MODAL_FOOTER_CLASS}>
          <button ref={cancelRef} type="button" onClick={cancel}
            className="px-3 py-2 text-body rounded border border-border bg-surface text-text hover:bg-surface-muted">
            {t("common.cancel")}
          </button>
          <button type="button" onClick={() => onChoose("refresh")}
            className="px-3 py-2 text-body rounded border border-warning bg-warning-muted text-warning hover:bg-warning-muted/80">
            {t("resume.refresh")}
          </button>
          {verified && (
            <button ref={resumeRef} type="button" onClick={() => onChoose("resume")}
              className="px-4 py-2 text-body font-medium rounded bg-accent hover:bg-accent-hover text-white">
              {t("resume.resume")}
            </button>
          )}
        </footer>
      </div>
    </div>
  );
}

/** Mounted once above routes, so a queue or hotkey uses the same modal. */
export function ResumeProjectDialogHost() {
  const request = useSyncExternalStore(subscribeProjectOpenChoice, getProjectOpenChoice, () => null);
  useEffect(() => () => {
    const pending = getProjectOpenChoice();
    if (pending) resolveProjectOpenChoice(pending, "cancel");
  }, []);
  return <ResumeProjectDialog key={request?.id} request={request}
    onChoose={(choice) => { if (request) resolveProjectOpenChoice(request, choice); }} />;
}
