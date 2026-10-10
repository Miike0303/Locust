import { useState, useEffect, useRef } from "react";
import { AlertTriangle, ChevronDown, ChevronRight } from "lucide-react";
import clsx from "clsx";
import type { StringEntry, StringStatus } from "../lib/api";
import { encodedByteLen, getString, patchString } from "../lib/api";
import { binaryBudgetHint } from "../lib/binaryBudget";
import { useT } from "../lib/i18n";
import { acknowledgeDraft, draftAlternatives, selectDraftAlternative, draftEntryKey, draftProjectKey, editDraft, loadLatestDraft, saveDraft, useDraftStore } from "../stores/draftStore";
import { useProjectStore } from "../stores/projectStore";

import type { MessageKey } from "../lib/i18n";

const statusButtons: { value: StringStatus; labelKey: MessageKey; color: string }[] = [
	{
		value: "pending",
		labelKey: "detail.status.pending",
		color: "bg-border text-text",
	},
	{
		value: "reviewed",
		labelKey: "detail.status.reviewed",
		color:
			"bg-warning-muted text-warning",
	},
	{
		value: "approved",
		labelKey: "detail.status.approved",
		color:
			"bg-success-muted text-success",
	},
];

interface DetailPanelProps {
	projectKey: string;
	entry: StringEntry;
	onRefetch: () => void;
	onClose: () => void;
}

export default function DetailPanel({
	projectKey,
	entry,
	onRefetch,
	onClose,
}: DetailPanelProps) {
	const t = useT();
	const draftKey = draftEntryKey(projectKey, entry.id);
	const draft = useDraftStore((state) => state.drafts[draftKey]);
	const records = useDraftStore((state) => state.records);
	const persistenceIssue = useDraftStore((state) => state.persistenceIssues[draftKey] ?? state.persistenceIssue);
	const alternatives = draftAlternatives(draftKey, records, draft);
	const translation = draft?.text ?? entry.translation ?? "";
	const [showMeta, setShowMeta] = useState(false);
	const [actionError, setActionError] = useState<string | null>(null);
	const saving = draft?.saving ?? false;
	const [changingStatus, setChangingStatus] = useState(false);
	const currentEntryId = useRef(entry.id);
	currentEntryId.current = entry.id;
	const isCurrentProject = () => {
		const project = useProjectStore.getState().project;
		return !!project && draftProjectKey(project) === projectKey;
	};

	useEffect(() => {
		acknowledgeDraft(draftKey, entry.translation || "");
	}, [draftKey, entry.translation, saving]);
	useEffect(() => { setActionError(null); }, [entry.id]);

	const handleSave = (overwrite = false): Promise<boolean> => {
		if (!isCurrentProject()) return Promise.resolve(false);
		const id = entry.id;
		setActionError(null);
		return saveDraft(draftKey, entry.translation || "", async (text, expectedTranslation) => {
			if (!isCurrentProject()) throw new Error(t("detail.projectChanged"));
			await patchString(id, { translation: text, ...(expectedTranslation === undefined ? {} : { expected_translation: expectedTranslation }) });
			if (isCurrentProject()) onRefetch();
		}, overwrite);
	};

	const handleLoadLatest = () => loadLatestDraft(draftKey, async () => {
		if (!isCurrentProject()) throw new Error(t("detail.projectChanged"));
		const latest = await getString(entry.id);
		if (!isCurrentProject()) throw new Error(t("detail.projectChanged"));
		onRefetch();
		return latest.translation;
	});

	const handleStatusChange = async (status: StringStatus) => {
		const id = entry.id;
		setChangingStatus(true);
		try {
			if (!(await handleSave()) || currentEntryId.current !== id || !isCurrentProject()) return;
			await patchString(id, { status });
			setActionError(null); onRefetch();
		} catch (error) {
			if (currentEntryId.current === id) setActionError(error instanceof Error ? error.message : String(error));
		} finally { setChangingStatus(false); }
	};

	const charCount = Array.from(translation).length;
	const limitExceeded =
		entry.char_limit != null && charCount > entry.char_limit;
	const budgetHint = binaryBudgetHint(entry);
	const binarySlot = budgetHint.encoding;
	const srcSlotBytes = budgetHint.capacity;
	const trSlotBytes =
		binarySlot != null ? encodedByteLen(binarySlot, translation) : null;
	const binarySlotExceeded =
		srcSlotBytes != null && trSlotBytes != null && trSlotBytes > srcSlotBytes;

	return (
		<aside className="w-[340px] border-l border-border overflow-y-auto bg-surface flex flex-col">
			<div className="px-3 py-2 border-b border-border flex justify-between items-center">
				<h3 className="text-caption font-semibold text-text">
					{t("detail.title")}
				</h3>
				<button
					onClick={onClose}
					className="text-text-muted hover:text-text text-section"
					aria-label={t("detail.closeAria")}
				>
					&times;
				</button>
			</div>

			<div className="p-3 space-y-3 flex-1">
				{Object.prototype.hasOwnProperty.call(entry.metadata ?? {}, "locust_stale_translation") && (
					<p className="text-caption text-warning bg-warning-muted rounded p-2">
						<strong>{t("validate.kind.staleLabel")}. </strong>{t("validate.kind.staleDetail")}
					</p>
				)}
				{(actionError || draft?.error) && <p role="alert" className="text-caption text-danger break-words">{actionError || draft?.error}</p>}
				{draft?.conflict && <div className="text-caption">
					<p role="alert" className="text-danger">{t("api.error.translationConflict")}</p>
					<div className="flex gap-3">
					<button type="button" disabled={saving || changingStatus} onClick={handleLoadLatest} className="underline">{t("detail.loadLatest")}</button>
					<button type="button" disabled={saving || changingStatus} onClick={() => handleSave(true)} className="underline">{t("detail.overwrite")}</button>
					</div>
				</div>}
                {persistenceIssue && <p role="alert" className="text-caption text-danger">{t("detail.draftStorageFailed")}</p>}
                {alternatives.length > 0 && <div className="text-caption text-warning">
                    <p>{t("detail.draftConflict")}</p>
                    {alternatives.map(alternative => <button key={alternative.revision} type="button" disabled={saving || changingStatus}
                        className="block w-full text-left border border-warning rounded p-1 mt-1 whitespace-pre-wrap break-words"
                        onClick={() => selectDraftAlternative(draftKey, alternative.revision)}>
                        {t("detail.draftRestore")}: {alternative.text.length ? alternative.text.slice(0, 100) : t("detail.draftEmpty")}
                    </button>)}
                </div>}
				{/* Source */}
				<div>
					<label className="text-caption font-medium text-text-muted">
						{t("detail.source")}
					</label>
					<div className="mt-1 p-2 bg-surface-muted rounded font-mono text-body text-text select-all whitespace-pre-wrap">
						{entry.source}
					</div>
				</div>

				{/* Translation */}
				<div>
					<label className="text-caption font-medium text-text-muted">
						{t("detail.translation")}
					</label>
					<textarea
						disabled={saving || changingStatus}
						value={translation}
						onFocus={() => {
							if (!useDraftStore.getState().drafts[draftKey]) editDraft(draftKey, translation, entry.translation);
						}}
						onChange={(e) => editDraft(draftKey, e.target.value, entry.translation)}
						onBlur={() => handleSave()}
						onKeyDown={(e) => {
							if (e.key === "Enter" && e.ctrlKey) handleSave();
						}}
						className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text focus:outline-none focus:ring-2 focus:ring-accent-fg resize-y min-h-[72px]"
						rows={4}
					/>
					<div
						className={clsx(
							"text-caption mt-1",
							limitExceeded || binarySlotExceeded
								? "text-danger font-semibold"
								: "text-text-muted",
						)}
					>
						{t("detail.chars", { count: charCount })}
						{entry.char_limit != null && t("detail.charsLimit", { limit: entry.char_limit })}
						{binarySlot && srcSlotBytes != null && trSlotBytes != null && (
							<span className="ml-2">
								· {binarySlot} {trSlotBytes}/{srcSlotBytes} B
							</span>
						)}
						{binarySlot === "sjis" && (
							<span className="ml-2 text-text-muted font-normal">
								{t("detail.sjisNote")}
							</span>
						)}
						{budgetHint.grouped && <p className="mt-1">{t("detail.sharedBudget")}</p>}
						{budgetHint.expandable && !budgetHint.grouped && <p className="mt-1">{t("detail.expandableBudget")}</p>}
					</div>
				</div>

				{/* Status */}
				<div>
					<label className="text-caption font-medium text-text-muted">
						{t("detail.status")}
					</label>
					<div className="flex gap-1.5 mt-1">
						{statusButtons.map(({ value, labelKey, color }) => (
							<button
								key={value}
								disabled={changingStatus}
								onClick={() => handleStatusChange(value)}
								className={clsx(
									"px-2.5 py-0.5 rounded-full text-caption font-medium transition-colors",
									entry.status === value
										? color
										: "bg-surface-muted text-text-muted hover:bg-border",
								)}
							>
								{t(labelKey)}
							</button>
						))}
					</div>
				</div>

				{/* Validation warnings */}
				{limitExceeded && (
					<div className="flex items-center gap-2 p-2 bg-danger-muted border border-danger rounded text-body text-danger">
						<AlertTriangle size={16} />
						{t("detail.exceedsLimit")}
					</div>
				)}
				{binarySlotExceeded && (
					<div className="flex items-center gap-2 p-2 bg-danger-muted border border-danger rounded text-body text-danger">
						<AlertTriangle size={16} />
						{t("detail.exceedsSlot", {
							slot: binarySlot ?? "",
							actual: trSlotBytes ?? 0,
							source: srcSlotBytes ?? 0,
						})}
					</div>
				)}

				{/* Metadata */}
				<div>
					<button
						onClick={() => setShowMeta(!showMeta)}
						className="flex items-center gap-1 text-caption font-medium text-text-muted hover:text-text"
					>
						{showMeta ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
						{t("detail.metadata")}
					</button>
					{showMeta && (
						<dl className="mt-2 space-y-1 text-caption text-text">
							{entry.context && (
								<>
									<dt className="text-text-muted">{t("detail.context")}</dt>
									<dd>{entry.context}</dd>
								</>
							)}
							<dt className="text-text-muted">{t("detail.file")}</dt>
							<dd className="break-all">{entry.file_path}</dd>
							<dt className="text-text-muted">{t("detail.entryId")}</dt>
							<dd className="font-mono break-all">{entry.id}</dd>
							{entry.tags.length > 0 && (
								<>
									<dt className="text-text-muted">{t("detail.tags")}</dt>
									<dd className="flex gap-1 flex-wrap">
										{entry.tags.map((t) => (
											<span
												key={t}
												className="px-1.5 py-0.5 bg-surface-muted rounded"
											>
												{t}
											</span>
										))}
									</dd>
								</>
							)}
							{entry.provider_used && (
								<>
									<dt className="text-text-muted">{t("detail.provider")}</dt>
									<dd>{entry.provider_used}</dd>
								</>
							)}
							<dt className="text-text-muted">{t("detail.created")}</dt>
							<dd>{new Date(entry.created_at).toLocaleString()}</dd>
							{entry.translated_at && (
								<>
									<dt className="text-text-muted">
										{t("detail.translated")}
									</dt>
									<dd>{new Date(entry.translated_at).toLocaleString()}</dd>
								</>
							)}
						</dl>
					)}
				</div>
			</div>
		</aside>
	);
}
