import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { AlertCircle, CheckCircle2, Circle, Clock3, Loader2, MinusCircle, X, Zap } from "lucide-react";
import { getConfig, getProviders, type PatchCompression, type ProjectInfo } from "../lib/api";
import { localizeApiError } from "../lib/apiError";
import {
	canResumeFastWorkflow,
	createFastWorkflowReceipt,
	estimateWorkflowRemainingSeconds,
	type FastWorkflowOptions,
	type FastWorkflowReceipt,
	type FastWorkflowStageReceipt,
} from "../lib/fastWorkflow";
import { useLocale, useT, type MessageKey, type TranslateFn } from "../lib/i18n";
import { availableInjectModes, defaultInjectMode, type InjectUiMode } from "../lib/injectModes";
import { LANGUAGES } from "../lib/languages";
import {
	MODAL_BACKDROP_CLASS, MODAL_BODY_CLASS, MODAL_FOOTER_CLASS,
	MODAL_HEADER_CLASS, modalPanelClass, useModalA11y,
} from "../lib/modalA11y";
import { formatProviderOptionLabel, resolveProviderReadiness } from "../lib/providerReadiness";
import { detectSourceLanguage } from "../lib/translationDefaults";
import { draftProjectKey } from "../stores/draftStore";
import { useFastWorkflowStore } from "../stores/fastWorkflowStore";

function displayError(message: string, t: TranslateFn): string {
	return message.startsWith("fastWorkflow.") ? t(message as MessageKey) : localizeApiError(message);
}

function stageLabel(stage: FastWorkflowStageReceipt, receipt: FastWorkflowReceipt, t: TranslateFn): string {
	if (stage.id === "translate") return t(receipt.options.englishPivot ? "fastWorkflow.stage.translateEnglish" : "fastWorkflow.stage.translate");
	return t(`fastWorkflow.stage.${stage.id}` as MessageKey);
}

function StageIcon({ status }: { status: FastWorkflowStageReceipt["status"] }) {
	const props = { size: 18, "aria-hidden": true as const };
	if (status === "running") return <Loader2 {...props} className="animate-spin text-accent-fg" />;
	if (status === "done") return <CheckCircle2 {...props} className="text-success" />;
	if (status === "failed") return <AlertCircle {...props} className="text-danger" />;
	if (status === "skipped") return <MinusCircle {...props} className="text-text-muted" />;
	return <Circle {...props} className="text-text-muted" />;
}

/** Stage display kept separate so every persisted render state can be tested. */
export function FastWorkflowView({ receipt, isRunning, cancelRequested = false, error = null }: {
	receipt: FastWorkflowReceipt;
	isRunning: boolean;
	cancelRequested?: boolean;
	error?: string | null;
}) {
	const t = useT();
	const [now, setNow] = useState(Date.now);
	useEffect(() => {
		if (!isRunning) return;
		const timer = window.setInterval(() => setNow(Date.now()), 1000);
		return () => window.clearInterval(timer);
	}, [isRunning]);
	const current = receipt.stages.find((stage) => stage.status === "running");
	const remaining = estimateWorkflowRemainingSeconds(receipt, now);
	const translations = receipt.stages.filter((stage) => stage.id === "translate" || stage.id === "translateTarget");
	const providerComplete = translations.every((stage) => stage.status === "done" || stage.status === "skipped");
	const progress = current?.progress;
	const completed = Math.max(0, progress?.completed ?? 0);
	const total = Math.max(0, progress?.total ?? 0);
	const summary = cancelRequested ? t("fastWorkflow.cancelling") : receipt.status === "completed"
		? t("fastWorkflow.completed") : current ? t("fastWorkflow.currentStage", { stage: stageLabel(current, receipt, t) })
			: t(`fastWorkflow.workflowStatus.${receipt.status}` as MessageKey);
	return <section data-fast-workflow="" className="space-y-3" aria-label={t("fastWorkflow.stages")}>
		<p role="status" aria-live="polite" aria-atomic="true" className="text-body font-medium">{summary}</p>
		<ol className="space-y-2">
			{receipt.stages.map((stage) => <li key={stage.id} data-stage={stage.id} data-status={stage.status} className="flex items-start gap-2.5 rounded border border-border px-3 py-2">
				<StageIcon status={stage.status} />
				<div className="flex-1 min-w-0 text-body">
					<div className="flex flex-wrap justify-between gap-x-3 gap-y-1">
						<span className={stage.status === "running" ? "font-medium text-accent-fg" : ""}>{stageLabel(stage, receipt, t)}</span>
						<span className="text-caption text-text-muted">{t(`fastWorkflow.stageStatus.${stage.status}` as MessageKey)}</span>
					</div>
					{stage.error && <p className="mt-1 text-caption text-danger break-words">{displayError(stage.error, t)}</p>}
					{stage.id === "validate" && stage.status === "done" && (stage.result?.validationIssues ?? 0) > 0 && <p className="mt-1 text-caption text-warning">{t("fastWorkflow.validationAdvisory", { count: stage.result!.validationIssues! })}</p>}
				</div>
			</li>)}
		</ol>
		{current && <div className="space-y-1">
			{total > 0 ? <>
				<label htmlFor="fast-workflow-progress" className="text-caption text-text-muted">{t("fastWorkflow.progress", { completed: Math.min(completed, total), total })}</label>
				<progress id="fast-workflow-progress" className="w-full h-2 accent-accent" value={Math.min(completed, total)} max={total} />
			</> : <p className="text-caption text-text-muted flex items-center gap-1.5"><Loader2 size={13} className="animate-spin" aria-hidden="true" />{t("fastWorkflow.stageWorking")}</p>}
		</div>}
		<p className="text-caption text-text-muted flex items-center gap-1.5"><Clock3 size={14} aria-hidden="true" />{remaining === null ? t("fastWorkflow.etaLearning") : t("fastWorkflow.eta", { minutes: Math.max(0, Math.ceil(remaining / 60)) })}</p>
		{providerComplete && receipt.status !== "completed" && <p className="text-body text-accent-fg">{t("fastWorkflow.providerComplete")}</p>}
		{receipt.status === "completed" && <p className="text-caption text-success break-all">{t("fastWorkflow.patchReady", { path: receipt.options.patchOutputPath })}</p>}
		{error && <p role="alert" className="text-body text-danger break-words">{displayError(error, t)}</p>}
	</section>;
}

function initialOptions(project: ProjectInfo, samples: readonly string[], locale: string): FastWorkflowOptions {
	const sourceLang = detectSourceLanguage(samples) ?? "auto";
	const targetLang = sourceLang === locale ? (locale === "en" ? "es" : "en") : locale;
	const base = project.path.replace(/[\\/]+$/, "");
	const runId = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
	return {
		sourceLang, targetLang, englishPivot: false, providerId: "google",
		injectMode: defaultInjectMode(project.supported_modes), compression: "fast",
		outputDir: `${base}-translated-${targetLang}`,
		patchOutputPath: `${base}-patch-${targetLang}-${runId}.zip`,
		pivotOutputPath: `${base}-english-${runId}.locust.db`,
	};
}

interface FastWorkflowModalProps {
	open: boolean;
	onClose: () => void;
	project: ProjectInfo | null;
	sampleSources?: readonly string[];
	onComplete?: () => void;
}

export default function FastWorkflowModal({ open, onClose, project, sampleSources = [], onComplete }: FastWorkflowModalProps) {
	const t = useT();
	const { locale } = useLocale();
	const { receipt, isRunning, cancelRequested, error, load, start, cancel } = useFastWorkflowStore();
	const [options, setOptions] = useState<FastWorkflowOptions | null>(null);
	const openedKey = useRef<string | null>(null);
	const completionSeen = useRef<number | null>(null);
	const projectKey = project ? draftProjectKey(project) : "";
	const { data: providers } = useQuery({ queryKey: ["providers"], queryFn: getProviders, enabled: open });
	const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig, enabled: open });
	useEffect(() => {
		if (!open || !project) { openedKey.current = null; return; }
		if (openedKey.current === projectKey) return;
		openedKey.current = projectKey;
		load(project);
		const saved = useFastWorkflowStore.getState().receipt;
		setOptions(saved && canResumeFastWorkflow(saved, projectKey, saved.options) ? saved.options : initialOptions(project, sampleSources, locale));
	}, [open, project, projectKey, load, sampleSources, locale]);
	useEffect(() => {
		if (receipt?.status !== "completed" || completionSeen.current === receipt.updatedAt) return;
		completionSeen.current = receipt.updatedAt;
		onComplete?.();
	}, [receipt?.status, receipt?.updatedAt, onComplete]);
	const close = () => { if (!isRunning) onClose(); };
	const { dialogRef, dialogProps, titleProps } = useModalA11y({ open: open && !!project, ownEscape: true, onClose: close });
	if (!open || !project) return null;
	const saved = receipt && canResumeFastWorkflow(receipt, projectKey, receipt.options) ? receipt : null;
	const selected = options ?? (saved ? saved.options : initialOptions(project, sampleSources, locale));
	const matches = saved ? canResumeFastWorkflow(saved, projectKey, selected) : false;
	const changed = !!saved && !matches;
	const uncertainWrite = saved?.stages.some((stage) => ((stage.id === "inject" || stage.id === "pack") && stage.startedAt !== undefined && stage.status !== "done" && stage.status !== "skipped") ||
		((stage.id === "translate" || stage.id === "translateTarget") && stage.status !== "done" && !!stage.result?.jobId && !stage.result.providerCompleted));
	// A partially edited form is displayable even when it is not valid to start.
	const preview = createFastWorkflowReceipt(project, initialOptions(project, sampleSources, locale));
	preview.options = selected;
	preview.stages = preview.stages.map((stage) => ({ ...stage, status: !selected.englishPivot && (stage.id === "pivot" || stage.id === "translateTarget") ? "skipped" : "pending" }));
	const visibleReceipt = saved && (matches || isRunning) ? saved : preview;
	const patch = (change: Partial<FastWorkflowOptions>) => setOptions({ ...selected, ...change });
	const ready = !providers || resolveProviderReadiness(selected.providerId, providers, config).ready;
	const invalid = selected.sourceLang === selected.targetLang || !selected.targetLang || !selected.providerId || !selected.patchOutputPath.trim()
		|| (selected.injectMode === "replace" && !selected.outputDir.trim()) || (selected.englishPivot && !selected.pivotOutputPath.trim());
	const fieldClass = "mt-1 w-full rounded border border-border bg-surface px-2.5 py-2 text-body text-text focus:outline-none focus:ring-2 focus:ring-accent-fg disabled:opacity-60";
	return <div className={`${MODAL_BACKDROP_CLASS} px-4`}>
		<div ref={dialogRef} {...dialogProps} className={modalPanelClass("max-w-2xl")}>
			<header className={`${MODAL_HEADER_CLASS} flex items-center justify-between gap-3 border-b border-border px-5 py-4`}>
				<h2 {...titleProps} className="text-section font-bold flex items-center gap-2"><Zap size={20} aria-hidden="true" />{t("fastWorkflow.title")}</h2>
				<button type="button" onClick={close} disabled={isRunning} aria-label={t("common.close")} className="rounded p-1 text-text-muted hover:text-text focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-40"><X size={20} aria-hidden="true" /></button>
			</header>
			<div className={`${MODAL_BODY_CLASS} px-5 py-4 space-y-4`}>
				<p className="text-body text-text-muted">{t("fastWorkflow.description")}</p>
				<details open={!isRunning && (!saved || changed || saved.status === "idle")} className="space-y-3">
					<summary tabIndex={0} className="cursor-pointer text-body font-medium focus-visible:ring-2 focus-visible:ring-accent-fg">{t("fastWorkflow.options")}</summary>
				<fieldset disabled={isRunning} className="grid grid-cols-1 sm:grid-cols-2 gap-3">
					<legend className="sr-only">{t("fastWorkflow.options")}</legend>
					<label className="text-caption font-medium">{t("fastWorkflow.source")}
						<select value={selected.sourceLang} onChange={(event) => patch({ sourceLang: event.target.value, englishPivot: false })} className={fieldClass}>
							<option value="auto">{t("fastWorkflow.autoSource")}</option>{LANGUAGES.map((language) => <option key={language.code} value={language.code}>{language.label}</option>)}
						</select>
					</label>
					<label className="text-caption font-medium">{t("fastWorkflow.target")}
						<select value={selected.targetLang} onChange={(event) => patch({ targetLang: event.target.value, englishPivot: event.target.value !== "en" && selected.englishPivot })} className={fieldClass}>
							{LANGUAGES.map((language) => <option key={language.code} value={language.code}>{language.label}</option>)}
						</select>
					</label>
					<label className="text-caption font-medium">{t("fastWorkflow.provider")}
						<select value={selected.providerId} onChange={(event) => patch({ providerId: event.target.value })} className={fieldClass}>
							<option value="google">{t("fastWorkflow.googleFast")}</option>
							{providers?.filter((provider) => provider.id !== "google").map((provider) => <option key={provider.id} value={provider.id} disabled={!resolveProviderReadiness(provider.id, providers, config).ready}>{formatProviderOptionLabel(provider)}</option>)}
						</select>
					</label>
					<label className="text-caption font-medium">{t("inject.mode")}
						<select value={selected.injectMode} onChange={(event) => patch({ injectMode: event.target.value as InjectUiMode })} className={fieldClass}>{availableInjectModes(project.supported_modes).map((mode) => <option key={mode} value={mode}>{t(`inject.mode.${mode}`)}</option>)}</select>
					</label>
					<label className="text-caption font-medium">{t("patch.compression")}
						<select value={selected.compression} onChange={(event) => patch({ compression: event.target.value as PatchCompression })} className={fieldClass}><option value="fast">{t("patch.compressionFast")}</option><option value="default">{t("patch.compressionDefault")}</option><option value="store">{t("patch.compressionStore")}</option></select>
					</label>
					<label className="flex items-center gap-2 text-body sm:self-end sm:pb-2"><input type="checkbox" checked={selected.englishPivot} disabled={selected.sourceLang !== "ja" || selected.targetLang === "en"} onChange={(event) => patch({ englishPivot: event.target.checked })} className="accent-accent" />{t("fastWorkflow.englishPivot")}</label>
					{selected.injectMode === "replace" && <label className="text-caption font-medium sm:col-span-2">{t("fastWorkflow.outputDir")}<input value={selected.outputDir} onChange={(event) => patch({ outputDir: event.target.value })} className={fieldClass} /></label>}
					<label className="text-caption font-medium sm:col-span-2">{t("fastWorkflow.patchOutput")}<input value={selected.patchOutputPath} onChange={(event) => patch({ patchOutputPath: event.target.value })} className={fieldClass} /></label>
					{selected.englishPivot && <label className="text-caption font-medium sm:col-span-2">{t("fastWorkflow.pivotOutput")}<input value={selected.pivotOutputPath} onChange={(event) => patch({ pivotOutputPath: event.target.value })} className={fieldClass} /></label>}
				</fieldset>
				<p className="text-caption text-text-muted">{t("fastWorkflow.pivotHint")}</p>
				</details>
				{changed && <div role="status" className="rounded border border-warning bg-warning-muted p-3 text-body text-warning"><p>{t(uncertainWrite ? "fastWorkflow.restoreBeforeResume" : "fastWorkflow.optionsChanged")}</p><button type="button" onClick={() => saved && setOptions(saved.options)} disabled={isRunning} className="mt-2 underline underline-offset-2 focus-visible:ring-2 focus-visible:ring-accent-fg">{t("fastWorkflow.restoreOptions")}</button></div>}
				{saved && matches && saved.status !== "completed" && !isRunning && <p className="text-caption text-accent-fg">{t("fastWorkflow.resumeHint")}</p>}
				{!ready && <p role="alert" className="text-body text-danger">{t("fastWorkflow.providerUnavailable")}</p>}
				{invalid && <p role="alert" className="text-caption text-danger">{t("fastWorkflow.invalidOptions")}</p>}
				<FastWorkflowView receipt={visibleReceipt} isRunning={isRunning} cancelRequested={cancelRequested} error={error} />
			</div>
			<footer className={MODAL_FOOTER_CLASS}>
				{isRunning ? <button type="button" onClick={cancel} disabled={cancelRequested} className="rounded border border-border px-4 py-2 text-body hover:bg-surface-muted disabled:opacity-50">{t(cancelRequested ? "fastWorkflow.cancelling" : "fastWorkflow.cancel")}</button> : <>
					<button type="button" onClick={close} className="rounded border border-border px-4 py-2 text-body hover:bg-surface-muted">{t("common.close")}</button>
					<button type="button" onClick={() => void start(project, selected, changed)} disabled={invalid || !ready || (changed && uncertainWrite) || (matches && saved?.status === "completed")} className="flex items-center gap-1.5 rounded bg-accent px-4 py-2 text-body font-medium text-white hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed"><Zap size={15} aria-hidden="true" />{t(changed ? "fastWorkflow.startFresh" : saved && saved.status !== "idle" ? "fastWorkflow.resume" : "fastWorkflow.start")}</button>
				</>}
			</footer>
		</div>
	</div>;
}
