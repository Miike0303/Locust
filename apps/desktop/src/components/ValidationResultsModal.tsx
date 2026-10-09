import { pathBasename } from "../lib/path";
import { X, Shield, AlertTriangle, Type } from "lucide-react";
import clsx from "clsx";
import type {
	ValidationResponse,
	ValidationIssue,
	ValidationKind,
	FontCoverageReport,
	FontSuggestion,
} from "../lib/api";
import { validationKindLabel } from "../lib/api";
import { validationBadgeLabel } from "../lib/validationLabels";
import { useT, type TranslateFn } from "../lib/i18n";
import {
	useModalA11y,
	MODAL_BACKDROP_CLASS,
	MODAL_FOOTER_CLASS,
	modalPanelClass,
} from "../lib/modalA11y";
import {
	canStartValidationWorklist,
	uniqueIssueEntryIds,
} from "../lib/validationWorklist";

interface ValidationResultsModalProps {
	open: boolean;
	result: ValidationResponse | null;
	onClose: () => void;
	/** Select entry in the editor and close this panel. */
	onSelectEntry: (entryId: string) => void;
	/** Start a Prev/Next walk over unique issue entry ids in the Editor. */
	onReviewInEditor?: (entryIds: string[]) => void;
	onFontPatch?: () => void;
}

const KIND_BADGE: Record<string, string> = {
	MissingPlaceholder:
		"bg-warning-muted text-warning",
	ExtraPlaceholder:
		"bg-warning-muted text-warning",
	ExceedsCharLimit:
		"bg-danger-muted text-danger",
	ExceedsBinarySlot:
		"bg-danger-muted text-danger",
	EmptyTranslation:
		"bg-surface-muted text-text",
	IdenticalToSource:
		"bg-blue-100 text-blue-800 dark:bg-blue-900/40 dark:text-blue-300",
	StaleTranslation:
		"bg-danger-muted text-danger",
};

function kindDetail(
	kind: ValidationKind,
	t: TranslateFn,
): string | null {
	if (kind === "StaleTranslation") return t("validate.kind.staleDetail");
	if (typeof kind === "string") return null;
	if ("MissingPlaceholder" in kind)
		return t("validate.kind.missing", {
			placeholder: kind.MissingPlaceholder.placeholder,
		});
	if ("ExtraPlaceholder" in kind)
		return t("validate.kind.extra", {
			placeholder: kind.ExtraPlaceholder.placeholder,
		});
	if ("ExceedsCharLimit" in kind) {
		const { limit, actual } = kind.ExceedsCharLimit;
		return t("validate.kind.charLimit", { actual, limit });
	}
	if ("ExceedsBinarySlot" in kind) {
		const { encoding, limit, actual } = kind.ExceedsBinarySlot;
		return t("validate.kind.binarySlot", { actual, limit, encoding });
	}
	return null;
}

function FontSection({
	fonts,
	suggestions = [],
	issues = [],
	limitations,
}: {
	fonts: FontCoverageReport[];
	suggestions?: FontSuggestion[];
	issues?: NonNullable<ValidationResponse["font_issues"]>;
	limitations?: string;
}) {
	const t = useT();
	const withMissing = fonts.filter((f) => f.missing_count > 0);
	if (fonts.length === 0 && issues.length === 0 && !limitations) return null;

	return (
		<div className="mt-4">
			<h3 className="text-body font-semibold text-text-muted uppercase mb-2 flex items-center gap-1.5">
				<Type size={14} /> {t("validate.fontCoverage")}
			</h3>
			<p className="text-caption text-text-muted mb-2">
				{fonts.length ? t("validate.fontsChecked", { count: fonts.length }) : t("validate.noLooseFonts")}
			</p>
			{limitations && (
				<p className="text-caption text-text-muted mb-2">
					{t("validate.fontLimitations")}
				</p>
			)}
			{issues.map((issue, index) => (
				<div key={`${issue.font_path}:${index}`} className="text-caption border border-warning rounded p-2 mb-2 break-words">
					<div className="font-medium">{t("validate.fontUnreadable")}: {issue.font_path}</div>
					<div>{issue.message}</div>
				</div>
			))}
			<div className="space-y-2 max-h-40 overflow-y-auto">
				{withMissing.map((f) => {
					const name =
						f.font_name || pathBasename(f.font_path);
					const sample = f.missing_chars.slice(0, 24).join(" ");
					const more =
						f.missing_count > 24
							? t("validate.more", { count: f.missing_count - 24 })
							: "";
					return (
						<div
							key={`${f.font_path}:${f.face_index ?? 0}`}
							className="text-body border border-border rounded p-2"
						>
							<div className="font-medium truncate" title={f.font_path}>
								{name}
							</div>
							<div className="text-caption text-text-muted mt-0.5">
								{t("validate.missingGlyphs", {
									count: f.missing_count,
									percent: f.coverage_percent.toFixed(1),
								})}
							</div>
							{sample && (
								<div className="text-caption font-mono mt-1 text-text-muted break-all">
									{sample}
									{more}
								</div>
							)}
						</div>
					);
				})}
			</div>
			{suggestions.length > 0 && (
				<div className="mt-3 space-y-2">
					<h4 className="text-caption font-semibold text-text-muted uppercase">
						{t("validate.fontSuggestions")}
					</h4>
					{suggestions.map((s) => (
						<div
							key={s.font_name}
							className="text-body border border-accent-fg bg-accent-muted rounded p-2"
						>
							<div className="font-medium text-accent-fg">
								{s.font_name}
							</div>
							<div className="text-caption text-text-muted mt-0.5">
								{t("validate.fontCovers", {
									scripts: s.covers_scripts.join(", "),
								})}
							</div>
							<div className="text-caption text-text-muted mt-0.5">{s.license}</div>
							<button
								type="button"
								onClick={() => window.open(s.download_url, "_blank", "noopener,noreferrer")}
								className="text-caption font-medium text-accent-fg hover:underline mt-1"
							>
								{t("validate.fontDownload")}
							</button>
						</div>
					))}
				</div>
			)}
		</div>
	);
}

export default function ValidationResultsModal({
	open,
	result,
	onClose,
	onSelectEntry,
	onReviewInEditor,
	onFontPatch,
}: ValidationResultsModalProps) {
	const t = useT();
	const { dialogRef, dialogProps, titleProps } = useModalA11y({
		open: open && !!result,
		ownEscape: false,
	});
	if (!open || !result) return null;

	const { validation, fonts } = result;
	const issues = validation.issues ?? [];
	const fontProblems = (fonts ?? []).filter((font) => font.missing_count > 0).length + (result.font_issues?.length ?? 0);
	const ResultIcon = fontProblems > 0 ? AlertTriangle : Shield;
	const worklistIds = uniqueIssueEntryIds(issues);
	const canReview = canStartValidationWorklist(worklistIds.length);
	const kindEntries = Object.entries(validation.by_kind || {}).sort(
		(a, b) => b[1] - a[1],
	);

	const handleClick = (issue: ValidationIssue) => {
		onSelectEntry(issue.entry_id);
	};

	const handleReviewInEditor = () => {
		if (!canReview || !onReviewInEditor) return;
		onReviewInEditor(worklistIds);
	};

	return (
		<div className={MODAL_BACKDROP_CLASS}>
			<div
				ref={dialogRef}
				{...dialogProps}
				className={modalPanelClass("max-w-2xl max-h-[85vh] flex flex-col")}
			>
				<div className="flex justify-between items-center px-5 py-4 border-b border-border">
					<div className="flex items-center gap-2">
						<Shield size={18} className="text-accent-fg" />
						<h2 {...titleProps} className="text-section font-bold">
							{t("validate.title")}
						</h2>
					</div>
					<button
						onClick={onClose}
						className="text-text-muted hover:text-text"
					>
						<X size={20} />
					</button>
				</div>

				<div className="px-5 py-4 overflow-y-auto flex-1 space-y-4">
					{/* Summary */}
					<div className="flex flex-wrap gap-3 text-body">
						<span className="text-text-muted">
							{t("validate.checked")}{" "}
							<strong className="text-text">
								{validation.total_checked}
							</strong>
						</span>
						<span className="text-text-muted">
							{t("validate.issues")}{" "}
							<strong
								className={
									validation.issues_found > 0
										? "text-danger"
										: "text-success"
								}
							>
								{validation.issues_found}
							</strong>
						</span>
						<span className="text-text-muted">
							{t("validate.entries")}{" "}
							<strong className="text-text">
								{validation.entries_with_issues}
							</strong>
						</span>
						{fontProblems > 0 && <span className="font-medium text-warning">{t("validate.fontProblems", { count: fontProblems })}</span>}
					</div>

					{kindEntries.length > 0 && (
						<div className="flex flex-wrap gap-2">
							{kindEntries.map(([kind, n]) => (
								<span
									key={kind}
									className={clsx(
										"px-2 py-0.5 rounded-full text-caption font-medium",
										KIND_BADGE[kind] ||
											"bg-surface-muted text-text-muted",
									)}
								>
									{validationBadgeLabel(kind, t)}: {n}
								</span>
							))}
						</div>
					)}

					{/* Issue list */}
					{issues.length === 0 ? (
						<div className="py-10 text-center text-text-muted">
							<ResultIcon
								size={32}
								className={clsx("mx-auto mb-2 opacity-80", fontProblems > 0 ? "text-warning" : "text-success")}
							/>
							<p className="font-medium text-text">
								{t(fontProblems > 0 ? "validate.textPassedFontsPending" : "validate.noIssues")}
							</p>
							<p className="text-body mt-1">
								{t("validate.validated", { count: validation.total_checked })}
							</p>
						</div>
					) : (
						<div>
							<h3 className="text-body font-semibold text-text-muted uppercase mb-2 flex items-center gap-1.5">
								<AlertTriangle size={14} /> {t("validate.issuesHeading")}
							</h3>
							<ul className="divide-y divide-border border border-border rounded-lg overflow-hidden">
								{issues.map((issue, i) => {
									const label = validationKindLabel(issue.kind);
									const detail = kindDetail(issue.kind, t);
									return (
										<li key={`${issue.entry_id}-${label}-${i}`}>
											<button
												type="button"
												onClick={() => handleClick(issue)}
												className="w-full text-left px-3 py-2.5 hover:bg-surface-muted transition-colors"
											>
												<div className="flex items-start gap-2">
													<span
														className={clsx(
															"shrink-0 px-2 py-0.5 rounded text-caption font-medium mt-0.5",
															KIND_BADGE[label] || "bg-surface-muted text-text-muted",
														)}
													>
														{validationBadgeLabel(label, t)}
													</span>
													<div className="min-w-0 flex-1">
														<div className="font-mono text-caption text-text-muted truncate">
															{issue.entry_id}
														</div>
														{issue.source && (
															<div
																className="text-body text-text truncate mt-0.5"
																title={issue.source}
															>
																{issue.source}
															</div>
														)}
														<div className="text-caption text-text-muted mt-0.5">
															{detail || issue.message}
														</div>
													</div>
												</div>
											</button>
										</li>
									);
								})}
							</ul>
							<p className="text-caption text-text-muted mt-2">
								{t("validate.clickHint")}
							</p>
						</div>
					)}

					<FontSection fonts={fonts ?? []} suggestions={result.font_suggestions ?? []} issues={result.font_issues ?? []} limitations={result.font_limitations} />
				</div>

				<div className={MODAL_FOOTER_CLASS}>
					{onFontPatch && <button type="button" onClick={onFontPatch} className="px-4 py-2 border border-border bg-surface text-text hover:bg-surface-muted rounded text-body font-medium">{t("fontPatch.title")}</button>}
					{onReviewInEditor && (
						<button
							type="button"
							onClick={handleReviewInEditor}
							disabled={!canReview}
							title={
								canReview
									? t("validate.reviewInEditorTitle")
									: t("validate.reviewInEditorDisabled")
							}
							className="px-4 py-2 bg-accent hover:bg-accent-hover disabled:opacity-50 disabled:cursor-not-allowed text-white rounded text-body font-medium"
						>
							{t("validate.reviewInEditor")}
						</button>
					)}
					<button
						onClick={onClose}
						className="px-4 py-2 bg-surface-muted text-text hover:bg-surface rounded text-body font-medium"
					>
						{t("common.close")}
					</button>
				</div>
			</div>
		</div>
	);
}
