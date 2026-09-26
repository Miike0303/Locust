import { useState, useEffect, useRef } from "react";
import { AlertTriangle, ChevronDown, ChevronRight } from "lucide-react";
import clsx from "clsx";
import type { StringEntry, StringStatus } from "../lib/api";
import { encodedByteLen, patchString } from "../lib/api";
import { binaryBudgetHint } from "../lib/binaryBudget";
import { useT } from "../lib/i18n";
import { acknowledgeDraft, draftAlternatives, selectDraftAlternative, draftEntryKey, draftProjectKey, editDraft, saveDraft, useDraftStore } from "../stores/draftStore";
import { useProjectStore } from "../stores/projectStore";

import type { MessageKey } from "../lib/i18n";

const statusButtons: { value: StringStatus; labelKey: MessageKey; color: string }[] = [
	{
		value: "pending",
		labelKey: "detail.status.pending",
		color: "bg-gray-200 text-gray-700 dark:bg-gray-700 dark:text-gray-200",
	},
	{
		value: "reviewed",
		labelKey: "detail.status.reviewed",
		color:
			"bg-amber-100 text-amber-800 dark:bg-amber-900/60 dark:text-amber-200",
	},
	{
		value: "approved",
		labelKey: "detail.status.approved",
		color:
			"bg-green-100 text-green-800 dark:bg-green-900/60 dark:text-green-200",
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

	const handleSave = (): Promise<boolean> => {
		if (!isCurrentProject()) return Promise.resolve(false);
		const id = entry.id;
		setActionError(null);
		return saveDraft(draftKey, entry.translation || "", async (text) => {
			if (!isCurrentProject()) throw new Error(t("detail.projectChanged"));
			await patchString(id, { translation: text });
			if (isCurrentProject()) onRefetch();
		});
	};

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
		<aside className="w-[340px] border-l border-gray-200 dark:border-gray-700 overflow-y-auto bg-white dark:bg-gray-900 flex flex-col">
			<div className="px-3 py-2 border-b border-gray-200 dark:border-gray-700 flex justify-between items-center">
				<h3 className="text-xs font-semibold text-gray-700 dark:text-gray-200">
					{t("detail.title")}
				</h3>
				<button
					onClick={onClose}
					className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 text-lg"
					aria-label={t("detail.closeAria")}
				>
					&times;
				</button>
			</div>

			<div className="p-3 space-y-3 flex-1">
				{Object.prototype.hasOwnProperty.call(entry.metadata ?? {}, "locust_stale_translation") && (
					<p className="text-xs text-amber-800 dark:text-amber-200 bg-amber-50 dark:bg-amber-950/40 rounded p-2">
						<strong>{t("validate.kind.staleLabel")}. </strong>{t("validate.kind.staleDetail")}
					</p>
				)}
				{(actionError || draft?.error) && <p role="alert" className="text-xs text-red-700 dark:text-red-300 break-words">{actionError || draft?.error}</p>}
                {persistenceIssue && <p role="alert" className="text-xs text-red-700 dark:text-red-300">{t("detail.draftStorageFailed")}</p>}
                {alternatives.length > 0 && <div className="text-xs text-amber-800 dark:text-amber-200">
                    <p>{t("detail.draftConflict")}</p>
                    {alternatives.map(alternative => <button key={alternative.revision} type="button" disabled={saving || changingStatus}
                        className="block w-full text-left border rounded p-1 mt-1 whitespace-pre-wrap break-words"
                        onClick={() => selectDraftAlternative(draftKey, alternative.revision)}>
                        {t("detail.draftRestore")}: {alternative.text.length ? alternative.text.slice(0, 100) : t("detail.draftEmpty")}
                    </button>)}
                </div>}
				{/* Source */}
				<div>
					<label className="text-[11px] font-semibold text-gray-500 dark:text-gray-400 uppercase">
						{t("detail.source")}
					</label>
					<div className="mt-1 p-2 bg-gray-50 dark:bg-gray-800 rounded font-mono text-xs text-gray-800 dark:text-gray-200 select-all whitespace-pre-wrap">
						{entry.source}
					</div>
				</div>

				{/* Translation */}
				<div>
					<label className="text-[11px] font-semibold text-gray-500 dark:text-gray-400 uppercase">
						{t("detail.translation")}
					</label>
					<textarea
						disabled={saving || changingStatus}
						value={translation}
						onChange={(e) => editDraft(draftKey, e.target.value)}
						onBlur={handleSave}
						onKeyDown={(e) => {
							if (e.key === "Enter" && e.ctrlKey) handleSave();
						}}
						className="mt-1 w-full p-2 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-800 text-xs text-gray-900 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-emerald-500 resize-y min-h-[72px]"
						rows={4}
					/>
					<div
						className={clsx(
							"text-[11px] mt-1",
							limitExceeded || binarySlotExceeded
								? "text-red-500 font-semibold"
								: "text-gray-400 dark:text-gray-500",
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
							<span className="ml-2 text-gray-400 font-normal">
								{t("detail.sjisNote")}
							</span>
						)}
						{budgetHint.grouped && <p className="mt-1">{t("detail.sharedBudget")}</p>}
						{budgetHint.expandable && !budgetHint.grouped && <p className="mt-1">{t("detail.expandableBudget")}</p>}
					</div>
				</div>

				{/* Status */}
				<div>
					<label className="text-[11px] font-semibold text-gray-500 dark:text-gray-400 uppercase">
						{t("detail.status")}
					</label>
					<div className="flex gap-1.5 mt-1">
						{statusButtons.map(({ value, labelKey, color }) => (
							<button
								key={value}
								disabled={changingStatus}
								onClick={() => handleStatusChange(value)}
								className={clsx(
									"px-2.5 py-0.5 rounded-full text-[11px] font-medium transition-colors",
									entry.status === value
										? color
										: "bg-gray-100 text-gray-500 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-400 dark:hover:bg-gray-700",
								)}
							>
								{t(labelKey)}
							</button>
						))}
					</div>
				</div>

				{/* Validation warnings */}
				{limitExceeded && (
					<div className="flex items-center gap-2 p-2 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded text-sm text-red-700 dark:text-red-400">
						<AlertTriangle size={16} />
						{t("detail.exceedsLimit")}
					</div>
				)}
				{binarySlotExceeded && (
					<div className="flex items-center gap-2 p-2 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded text-sm text-red-700 dark:text-red-400">
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
						className="flex items-center gap-1 text-[11px] font-semibold text-gray-500 dark:text-gray-400 uppercase hover:text-gray-700 dark:hover:text-gray-200"
					>
						{showMeta ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
						{t("detail.metadata")}
					</button>
					{showMeta && (
						<dl className="mt-2 space-y-1 text-[11px] text-gray-700 dark:text-gray-300">
							{entry.context && (
								<>
									<dt className="text-gray-500 dark:text-gray-400">{t("detail.context")}</dt>
									<dd>{entry.context}</dd>
								</>
							)}
							<dt className="text-gray-500 dark:text-gray-400">{t("detail.file")}</dt>
							<dd className="break-all">{entry.file_path}</dd>
							<dt className="text-gray-500 dark:text-gray-400">{t("detail.entryId")}</dt>
							<dd className="font-mono break-all">{entry.id}</dd>
							{entry.tags.length > 0 && (
								<>
									<dt className="text-gray-500 dark:text-gray-400">{t("detail.tags")}</dt>
									<dd className="flex gap-1 flex-wrap">
										{entry.tags.map((t) => (
											<span
												key={t}
												className="px-1.5 py-0.5 bg-gray-100 dark:bg-gray-700 rounded"
											>
												{t}
											</span>
										))}
									</dd>
								</>
							)}
							{entry.provider_used && (
								<>
									<dt className="text-gray-500 dark:text-gray-400">{t("detail.provider")}</dt>
									<dd>{entry.provider_used}</dd>
								</>
							)}
							<dt className="text-gray-500 dark:text-gray-400">{t("detail.created")}</dt>
							<dd>{new Date(entry.created_at).toLocaleString()}</dd>
							{entry.translated_at && (
								<>
									<dt className="text-gray-500 dark:text-gray-400">
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
