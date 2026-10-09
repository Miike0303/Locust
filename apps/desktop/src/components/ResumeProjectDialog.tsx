import { useEffect, useId, useRef, useSyncExternalStore } from "react";
import {
  getProjectOpenChoice, resolveProjectOpenChoice, subscribeProjectOpenChoice,
  type ProjectOpenChoice, type ProjectOpenChoiceRequest,
} from "../lib/openProjectFlow";
import { useT } from "../lib/i18n";
import {
  useModalA11y, MODAL_BACKDROP_CLASS, MODAL_FOOTER_CLASS, modalPanelClass,
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
        className={modalPanelClass("max-w-lg p-5 max-h-[90vh] overflow-y-auto")}
        onClick={(event) => event.stopPropagation()}>
        <h2 {...titleProps} className="text-section font-bold mb-2">
          {t(verified ? "resume.title" : "resume.attentionTitle")}
        </h2>
        <div id={descriptionId} className="text-body text-text-muted space-y-3 mb-4">
          <p>{t(verified ? "resume.message" : "resume.attentionMessage")}</p>
          <p className="break-all font-mono text-caption">{request.gamePath}</p>
          {request.preflight.kind === "needs_attention" && (
            <p className="whitespace-pre-wrap break-words text-warning">
              {request.preflight.reason}
            </p>
          )}
          <p>{t("resume.refreshWarning")}</p>
        </div>
        <div className={`${MODAL_FOOTER_CLASS} -mx-5 -mb-5 flex-wrap`}>
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
        </div>
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
