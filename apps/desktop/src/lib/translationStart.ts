import { startTranslation, type TranslationStartParams } from "./api";
import { t } from "./i18n";
import { canStartSingleJob } from "./translationJob";
import { attachTranslationJob } from "./translationJobSession";
import { useEditorStore } from "../stores/editorStore";
import { useQueueStore } from "../stores/queueStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";

/** Reserve the editor job before the API call, including time without a job ID. */
export async function startSingleTranslation(
  params: TranslationStartParams,
  opts: { projectName: string; providerLabel: string },
): Promise<{ job_id: string } | null> {
  const queueRunning = useQueueStore.getState().isRunning;
  const editor = useEditorStore.getState();
  if (!canStartSingleJob({ queueRunning, singleJobRunning: editor.isTranslating })) {
    addToast("info", t(queueRunning ? "translate.toast.queueRunning" : "translate.toast.singleJobRunning"));
    return null;
  }

  editor.setTranslating(true);
  editor.setJobSnapshot(null);
  let result: { job_id: string };
  try {
    addLog("info", t("activity.translation.callingApi"), JSON.stringify(params, null, 2), "translation");
    result = await startTranslation(params);
  } catch (err) {
    editor.setTranslating(false);
    throw err;
  }

  addLog("info", t("activity.translation.jobReceived", { jobId: result.job_id }), undefined, "translation");
  attachTranslationJob({ jobId: result.job_id, ...opts });
  return result;
}
