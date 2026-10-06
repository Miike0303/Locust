/**
 * Pure decisions for translation-job reattachment (modal close must not
 * drop a running job, and reopen must not double-subscribe).
 */

export type TranslationModalStep = "configure" | "progress";

export interface TranslationExecutionState {
  queueRunning: boolean;
  /** Includes the start request before the backend returns a job ID. */
  singleJobRunning: boolean;
}

export function canStartQueue(state: TranslationExecutionState): boolean {
  return !state.queueRunning && !state.singleJobRunning;
}

export function canStartSingleJob(state: TranslationExecutionState): boolean {
  return !state.queueRunning && !state.singleJobRunning;
}

export function shouldReattachTranslationJob(
  jobId: string | null,
  isTranslating: boolean,
): boolean {
  return Boolean(jobId && isTranslating);
}

export function shouldSubscribeToJob(
  jobId: string,
  alreadySubscribedJobId: string | null,
): boolean {
  return alreadySubscribedJobId !== jobId;
}

export function snapshotIsTerminal(
  snapshot: {
    done: boolean;
    cancelled: boolean;
    error: string | null;
  } | null,
): boolean {
  if (!snapshot) return false;
  return snapshot.done || snapshot.cancelled || Boolean(snapshot.error);
}

export function translationModalStep(opts: {
  isTranslating: boolean;
  snapshot: {
    done: boolean;
    cancelled: boolean;
    error: string | null;
  } | null;
}): TranslationModalStep {
  if (opts.isTranslating || snapshotIsTerminal(opts.snapshot)) return "progress";
  return "configure";
}
