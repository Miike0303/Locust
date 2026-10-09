import { useState } from "react";
import clsx from "clsx";
import { X, Replace, AlertCircle } from "lucide-react";
import { batchPatchStrings, getStrings, type StringEntry } from "../lib/api";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	MODAL_FOOTER_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import { useT } from "../lib/i18n";

interface SearchReplaceModalProps {
	open: boolean;
	onClose: () => void;
	onDone?: () => void;
}

function countMatches(
	text: string,
	find: string,
	caseSensitive: boolean,
): number {
	if (!find) return 0;
	if (caseSensitive) {
		let n = 0;
		let i = 0;
		while ((i = text.indexOf(find, i)) !== -1) {
			n++;
			i += find.length;
		}
		return n;
	}
	const lower = text.toLowerCase();
	const f = find.toLowerCase();
	let n = 0;
	let i = 0;
	while ((i = lower.indexOf(f, i)) !== -1) {
		n++;
		i += f.length;
	}
	return n;
}

function replaceAll(
	text: string,
	find: string,
	replace: string,
	caseSensitive: boolean,
): string {
	if (!find) return text;
	if (caseSensitive) {
		return text.split(find).join(replace);
	}
	// Case-insensitive replace preserving original match casing is overkill —
	// use the provided replacement string for every hit.
	const re = new RegExp(find.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi");
	return text.replace(re, replace);
}

export default function SearchReplaceModal({
	open,
	onClose,
	onDone,
}: SearchReplaceModalProps) {
	const t = useT();
	const [find, setFind] = useState("");
	const [replace, setReplace] = useState("");
	const [caseSensitive, setCaseSensitive] = useState(false);
	const [loading, setLoading] = useState(false);
	const [preview, setPreview] = useState<{
		entries: number;
		occurrences: number;
		samples: { id: string; before: string; after: string }[];
	} | null>(null);
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open,
		ownEscape: false,
	});

	if (!open) return null;

	const runPreview = async () => {
		if (!find) {
			addToast("error", t("replace.toast.enterFind"));
			return;
		}
		setLoading(true);
		setPreview(null);
		try {
			const res = await getStrings({ search: find, limit: 50_000, offset: 0 });
			let entriesHit = 0;
			let occurrences = 0;
			const samples: { id: string; before: string; after: string }[] = [];
			for (const e of res.entries) {
				const target = e.translation;
				if (!target) continue;
				const nTr = countMatches(target, find, caseSensitive);
				if (nTr === 0) continue;
				entriesHit++;
				occurrences += nTr;
				if (samples.length < 5) {
					samples.push({
						id: e.id,
						before: target.slice(0, 120),
						after: replaceAll(target, find, replace, caseSensitive).slice(
							0,
							120,
						),
					});
				}
			}
			setPreview({ entries: entriesHit, occurrences, samples });
			if (entriesHit === 0) {
				addToast("info", t("replace.toast.noMatches"));
			}
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addToast("error", t("replace.toast.previewFailed", { error: msg }));
		} finally {
			setLoading(false);
		}
	};

	const runReplace = async () => {
		if (!find) {
			addToast("error", t("replace.toast.enterFind"));
			return;
		}
		setLoading(true);
		try {
			const res = await getStrings({ search: find, limit: 50_000, offset: 0 });
			let occurrences = 0;
			const updates: { id: string; translation: string }[] = [];
			for (const e of res.entries as StringEntry[]) {
				if (!e.translation) continue;
				const n = countMatches(e.translation, find, caseSensitive);
				if (n === 0) continue;
				const next = replaceAll(e.translation, find, replace, caseSensitive);
				if (next === e.translation) continue;
				updates.push({ id: e.id, translation: next });
				occurrences += n;
			}
			if (updates.length === 0) {
				addToast("info", t("replace.toast.nothing"));
				return;
			}
			const result = await batchPatchStrings(updates, "search-replace");
			addToast(
				result.skipped ? "warning" : "success",
				result.skipped
					? t("replace.toast.replacedSkipped", {
							applied: result.applied,
							occurrences,
							skipped: result.skipped,
						})
					: t("replace.toast.replaced", {
							applied: result.applied,
							occurrences,
						}),
			);
			addLog(
				"info",
				t("activity.replace.completed", {
					applied: t("activity.replace.applied", { count: result.applied, requested: result.requested }),
					count: occurrences,
				}),
				find.length > 40 ? `${find.slice(0, 40)}…` : find,
				"replace",
			);
			onDone?.();
			onClose();
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			addToast("error", t("replace.toast.replaceFailed", { error: msg }));
			addLog("error", t("activity.replace.failed"), msg, "replace");
		} finally {
			setLoading(false);
		}
	};

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-lg p-6")}
			>
				<div className="flex justify-between items-center mb-4">
					<h2
						{...titleProps}
						className="text-section font-bold flex items-center gap-2"
					>
						<Replace size={18} /> {t("replace.title")}
					</h2>
					<button
						onClick={onClose}
						className="text-text-muted hover:text-text"
					>
						<X size={20} />
					</button>
				</div>

				<div className="space-y-3">
					<div>
						<label className="text-body font-medium">{t("replace.find")}</label>
						<input
							value={find}
							onChange={(e) => {
								setFind(e.target.value);
								setPreview(null);
							}}
							className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg font-mono"
							placeholder={t("replace.findPlaceholder")}
							autoFocus
						/>
					</div>
					<div>
						<label className="text-body font-medium">{t("replace.replaceWith")}</label>
						<input
							value={replace}
							onChange={(e) => {
								setReplace(e.target.value);
								setPreview(null);
							}}
							className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg font-mono"
							placeholder={t("replace.replacePlaceholder")}
						/>
					</div>

					<label className="flex items-center gap-2 text-body cursor-pointer">
						<input
							className="border border-border bg-surface text-text accent-accent focus:outline-none focus:ring-2 focus:ring-accent-fg"
							type="checkbox"
							checked={caseSensitive}
							onChange={(e) => {
								setCaseSensitive(e.target.checked);
								setPreview(null);
							}}
						/>
						{t("replace.caseSensitive")}
					</label>

					<p className="text-caption text-text-muted flex items-start gap-1">
						<AlertCircle size={12} className="mt-0.5 shrink-0" />
						{t("replace.hint")}
					</p>

					{preview && (
						<div className="text-body border border-border rounded p-3 bg-surface-muted">
							<p>
								{t("replace.preview", {
									entries: preview.entries,
									occurrences: preview.occurrences,
								})}
							</p>
							{preview.samples.length > 0 && (
								<ul className="mt-2 space-y-2 text-caption font-mono max-h-32 overflow-y-auto">
									{preview.samples.map((s) => (
										<li
											key={s.id}
											className="border-t border-border pt-1"
										>
											<div className="text-text-muted truncate">{s.id}</div>
											<div className="text-danger truncate">
												− {s.before}
											</div>
											<div className="text-success truncate">
												+ {s.after}
											</div>
										</li>
									))}
								</ul>
							)}
						</div>
					)}

					<div className={clsx(MODAL_FOOTER_CLASS, "-mx-6 -mb-6")}>
						<button
							onClick={onClose}
							className="px-3 py-2 text-body rounded border border-border bg-surface text-text hover:bg-surface-muted"
						>
							{t("common.cancel")}
						</button>
						<button
							onClick={() => {
								void runPreview();
							}}
							disabled={loading || !find}
							className="px-3 py-2 text-body rounded border border-border bg-surface text-text hover:bg-surface-muted disabled:opacity-50"
						>
							{loading ? "…" : t("replace.previewBtn")}
						</button>
						<button
							onClick={() => {
								void runReplace();
							}}
							disabled={loading || !find}
							className="px-4 py-2 text-body font-medium rounded border border-danger bg-danger-muted hover:bg-danger-muted/80 disabled:opacity-50 text-danger"
						>
							{loading ? t("replace.replacing") : t("replace.replaceAll")}
						</button>
					</div>
				</div>
			</div>
		</div>
	);
}
