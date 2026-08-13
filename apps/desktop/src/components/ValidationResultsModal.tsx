import { X, Shield, AlertTriangle, Type } from "lucide-react";
import clsx from "clsx";
import type {
	ValidationResponse,
	ValidationIssue,
	ValidationKind,
	FontCoverageReport,
} from "../lib/api";
import { validationKindLabel } from "../lib/api";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	MODAL_FOOTER_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";

interface ValidationResultsModalProps {
	open: boolean;
	result: ValidationResponse | null;
	onClose: () => void;
	/** Select entry in the editor and close this panel. */
	onSelectEntry: (entryId: string) => void;
}

const KIND_BADGE: Record<string, string> = {
	MissingPlaceholder:
		"bg-amber-100 text-amber-800 dark:bg-amber-900/40 dark:text-amber-300",
	ExtraPlaceholder:
		"bg-orange-100 text-orange-800 dark:bg-orange-900/40 dark:text-orange-300",
	ExceedsCharLimit:
		"bg-red-100 text-red-800 dark:bg-red-900/40 dark:text-red-300",
	ExceedsBinarySlot:
		"bg-red-100 text-red-800 dark:bg-red-900/40 dark:text-red-300",
	EmptyTranslation:
		"bg-gray-200 text-gray-800 dark:bg-gray-700 dark:text-gray-200",
	IdenticalToSource:
		"bg-blue-100 text-blue-800 dark:bg-blue-900/40 dark:text-blue-300",
};

function kindDetail(kind: ValidationKind): string | null {
	if (typeof kind === "string") return null;
	if ("MissingPlaceholder" in kind)
		return `missing ${kind.MissingPlaceholder.placeholder}`;
	if ("ExtraPlaceholder" in kind)
		return `extra ${kind.ExtraPlaceholder.placeholder}`;
	if ("ExceedsCharLimit" in kind) {
		const { limit, actual } = kind.ExceedsCharLimit;
		return `${actual} chars > limit ${limit}`;
	}
	if ("ExceedsBinarySlot" in kind) {
		const { encoding, limit, actual } = kind.ExceedsBinarySlot;
		return `${actual} > ${limit} bytes (${encoding})`;
	}
	return null;
}

function FontSection({ fonts }: { fonts: FontCoverageReport[] }) {
	const withMissing = fonts.filter((f) => f.missing_count > 0);
	if (withMissing.length === 0) return null;

	return (
		<div className="mt-4">
			<h3 className="text-sm font-semibold text-gray-500 uppercase mb-2 flex items-center gap-1.5">
				<Type size={14} /> Font coverage
			</h3>
			<div className="space-y-2 max-h-40 overflow-y-auto">
				{withMissing.map((f) => {
					const name =
						f.font_name || f.font_path.split(/[/\\]/).pop() || f.font_path;
					const sample = f.missing_chars.slice(0, 24).join(" ");
					const more =
						f.missing_count > 24 ? ` +${f.missing_count - 24} more` : "";
					return (
						<div
							key={f.font_path}
							className="text-sm border border-gray-200 dark:border-gray-700 rounded p-2"
						>
							<div className="font-medium truncate" title={f.font_path}>
								{name}
							</div>
							<div className="text-xs text-gray-500 mt-0.5">
								{f.missing_count} missing glyph
								{f.missing_count === 1 ? "" : "s"} ·{" "}
								{f.coverage_percent.toFixed(1)}% coverage
							</div>
							{sample && (
								<div className="text-xs font-mono mt-1 text-gray-600 dark:text-gray-400 break-all">
									{sample}
									{more}
								</div>
							)}
						</div>
					);
				})}
			</div>
		</div>
	);
}

export default function ValidationResultsModal({
	open,
	result,
	onClose,
	onSelectEntry,
}: ValidationResultsModalProps) {
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open: open && !!result,
		ownEscape: false,
	});
	if (!open || !result) return null;

	const { validation, fonts } = result;
	const issues = validation.issues ?? [];
	const kindEntries = Object.entries(validation.by_kind || {}).sort(
		(a, b) => b[1] - a[1],
	);

	const handleClick = (issue: ValidationIssue) => {
		onSelectEntry(issue.entry_id);
	};

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-2xl max-h-[85vh] flex flex-col")}
			>
				<div className="flex justify-between items-center px-5 py-4 border-b border-gray-200 dark:border-gray-700">
					<div className="flex items-center gap-2">
						<Shield size={18} className="text-emerald-600" />
						<h2 {...titleProps} className="text-lg font-bold">
							Validation results
						</h2>
					</div>
					<button
						onClick={onClose}
						className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
					>
						<X size={20} />
					</button>
				</div>

				<div className="px-5 py-4 overflow-y-auto flex-1 space-y-4">
					{/* Summary */}
					<div className="flex flex-wrap gap-3 text-sm">
						<span className="text-gray-600 dark:text-gray-400">
							Checked{" "}
							<strong className="text-gray-900 dark:text-gray-100">
								{validation.total_checked}
							</strong>
						</span>
						<span className="text-gray-600 dark:text-gray-400">
							Issues{" "}
							<strong
								className={
									validation.issues_found > 0
										? "text-red-600 dark:text-red-400"
										: "text-emerald-600"
								}
							>
								{validation.issues_found}
							</strong>
						</span>
						<span className="text-gray-600 dark:text-gray-400">
							Entries{" "}
							<strong className="text-gray-900 dark:text-gray-100">
								{validation.entries_with_issues}
							</strong>
						</span>
					</div>

					{kindEntries.length > 0 && (
						<div className="flex flex-wrap gap-2">
							{kindEntries.map(([kind, n]) => (
								<span
									key={kind}
									className={clsx(
										"px-2 py-0.5 rounded-full text-xs font-medium",
										KIND_BADGE[kind] ||
											"bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300",
									)}
								>
									{kind}: {n}
								</span>
							))}
						</div>
					)}

					{/* Issue list */}
					{issues.length === 0 ? (
						<div className="py-10 text-center text-gray-500">
							<Shield
								size={32}
								className="mx-auto mb-2 text-emerald-500 opacity-80"
							/>
							<p className="font-medium text-gray-700 dark:text-gray-300">
								No issues found
							</p>
							<p className="text-sm mt-1">
								Validated {validation.total_checked} string
								{validation.total_checked === 1 ? "" : "s"}.
							</p>
						</div>
					) : (
						<div>
							<h3 className="text-sm font-semibold text-gray-500 uppercase mb-2 flex items-center gap-1.5">
								<AlertTriangle size={14} /> Issues
							</h3>
							<ul className="divide-y divide-gray-100 dark:divide-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg overflow-hidden">
								{issues.map((issue, i) => {
									const label = validationKindLabel(issue.kind);
									const detail = kindDetail(issue.kind);
									return (
										<li key={`${issue.entry_id}-${label}-${i}`}>
											<button
												type="button"
												onClick={() => handleClick(issue)}
												className="w-full text-left px-3 py-2.5 hover:bg-gray-50 dark:hover:bg-gray-800/60 transition-colors"
											>
												<div className="flex items-start gap-2">
													<span
														className={clsx(
															"shrink-0 px-2 py-0.5 rounded text-xs font-medium mt-0.5",
															KIND_BADGE[label] || "bg-gray-100 text-gray-700",
														)}
													>
														{label}
													</span>
													<div className="min-w-0 flex-1">
														<div className="font-mono text-xs text-gray-500 truncate">
															{issue.entry_id}
														</div>
														{issue.source && (
															<div
																className="text-sm text-gray-800 dark:text-gray-200 truncate mt-0.5"
																title={issue.source}
															>
																{issue.source}
															</div>
														)}
														<div className="text-xs text-gray-500 mt-0.5">
															{detail || issue.message}
														</div>
													</div>
												</div>
											</button>
										</li>
									);
								})}
							</ul>
							<p className="text-xs text-gray-400 mt-2">
								Click an issue to open that string in the editor.
							</p>
						</div>
					)}

					<FontSection fonts={fonts ?? []} />
				</div>

				<div className={MODAL_FOOTER_CLASS}>
					<button
						onClick={onClose}
						className="px-4 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium"
					>
						Close
					</button>
				</div>
			</div>
		</div>
	);
}
