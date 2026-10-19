import { AlertCircle, FileArchive, Loader2, Package, RotateCcw, ShieldCheck } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import {
	type PatchApplyResult,
	type PatchRollbackResult,
	type PatchStatusResult,
	type PatchVerifyResult,
} from "../../lib/api";
import type { TranslateFn } from "../../lib/i18n";
import { isHttpPatchUrl, patchSourceReady, resolvePatchSource } from "../../lib/patchSource";

interface PatchApplyTabProps {
	t: TranslateFn;
	zipPath: string;
	setZipPath: Dispatch<SetStateAction<string>>;
	setZipUrl: Dispatch<SetStateAction<string>>;
	pickZip: () => Promise<void>;
	zipUrl: string;
	urlFieldError: string | null;
	urlZipHint: string | null;
	resolvedSource: ReturnType<typeof resolvePatchSource>;
	force: boolean;
	setForce: Dispatch<SetStateAction<boolean>>;
	confirmLegacy: boolean;
	setConfirmLegacy: Dispatch<SetStateAction<boolean>>;
	dryRun: boolean;
	setDryRun: Dispatch<SetStateAction<boolean>>;
	handleVerify: () => Promise<void>;
	loading: boolean;
	applying: boolean;
	canVerifyApply: boolean;
	gamePath: string;
	sourceOk: boolean;
	handleApply: () => Promise<void>;
	handlePreviewRollback: () => Promise<void>;
	handleRollback: () => Promise<void>;
	showPartialWarning: boolean;
	refreshStatus: () => Promise<void>;
	applyProgress: { current: number; total: number; path: string; phase: string; } | null;
	handleCancelApply: () => Promise<void>;
	cancellingApply: boolean;
	preview: PatchRollbackResult | null;
	status: PatchStatusResult | null;
	patchValue: (value: string) => string;
	verify: PatchVerifyResult | null;
	applyResult: PatchApplyResult | null;
}

export default function PatchApplyTab({
	t,
	zipPath,
	setZipPath,
	setZipUrl,
	pickZip,
	zipUrl,
	urlFieldError,
	urlZipHint,
	resolvedSource,
	force,
	setForce,
	confirmLegacy,
	setConfirmLegacy,
	dryRun,
	setDryRun,
	handleVerify,
	loading,
	applying,
	canVerifyApply,
	gamePath,
	sourceOk,
	handleApply,
	handlePreviewRollback,
	handleRollback,
	showPartialWarning,
	refreshStatus,
	applyProgress,
	handleCancelApply,
	cancellingApply,
	preview,
	status,
	patchValue,
	verify,
	applyResult,
}: PatchApplyTabProps) {
	return (
		<>
			<div>
				<label className="text-body font-medium">{t("patch.zipLocal")}</label>
				<div className="flex gap-2 mt-1">
					<input
						value={zipPath}
						onChange={(e) => {
							setZipPath(e.target.value);
							if (e.target.value.trim()) setZipUrl("");
						}}
						placeholder={t("patch.zipPlaceholder")}
						className="flex-1 p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
					/>
					<button
						onClick={pickZip}
						className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded"
						title={t("common.browse")}
					>
						<FileArchive size={16} />
					</button>
				</div>
			</div>

			<div>
				<label className="text-body font-medium">{t("patch.orUrl")}</label>
				<input
					value={zipUrl}
					onChange={(e) => {
						setZipUrl(e.target.value);
						if (e.target.value.trim()) setZipPath("");
					}}
					placeholder={t("patch.urlPlaceholder")}
					className={`w-full mt-1 p-2 border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg ${
						urlFieldError
							? "border-danger"
							: zipUrl.trim() && isHttpPatchUrl(zipUrl)
								? "border-success"
								: "border-border"
					}`}
				/>
				{urlFieldError ? (
					<p className="text-caption text-danger mt-1">
						{urlFieldError}
					</p>
				) : urlZipHint ? (
					<p className="text-caption text-warning mt-1">
						{urlZipHint}
					</p>
				) : (
					<p className="text-caption text-text-muted mt-1">
						{t("patch.urlHelp")}
					</p>
				)}
				{patchSourceReady(resolvedSource) && (
					<p className="text-caption text-success mt-0.5">
						{"zip_url" in resolvedSource
							? t("patch.activeUrl", {
									url: `${resolvedSource.zip_url.slice(0, 48)}${
										resolvedSource.zip_url.length > 48 ? "…" : ""
									}`,
								})
							: t("patch.activeLocal")}
					</p>
				)}
			</div>

			<div className="space-y-2 text-body">
				<label className="flex items-start gap-2 cursor-pointer">
					<input
						type="checkbox"
						className="mt-0.5 border border-border bg-surface text-text accent-accent focus:outline-none focus:ring-2 focus:ring-accent-fg"
						checked={force}
						onChange={(e) => setForce(e.target.checked)}
					/>
					<span>
						<span className="font-medium">{t("patch.force")}</span>
						<span className="block text-caption text-text-muted mt-0.5">
							{t("patch.forceHint")}
						</span>
					</span>
				</label>
				<label className="flex items-start gap-2 cursor-pointer">
					<input
						type="checkbox"
						className="mt-0.5 border border-border bg-surface text-text accent-accent focus:outline-none focus:ring-2 focus:ring-accent-fg"
						checked={confirmLegacy}
						onChange={(e) => setConfirmLegacy(e.target.checked)}
					/>
					<span>
						<span className="font-medium">{t("patch.legacy")}</span>
						<span className="block text-caption text-text-muted mt-0.5">
							{t("patch.legacyHint")}
						</span>
					</span>
				</label>
				<label className="flex items-start gap-2 cursor-pointer">
					<input
						type="checkbox"
						className="mt-0.5 border border-border bg-surface text-text accent-accent focus:outline-none focus:ring-2 focus:ring-accent-fg"
						checked={dryRun}
						onChange={(e) => setDryRun(e.target.checked)}
					/>
					<span>
						<span className="font-medium">{t("patch.dryRun")}</span>
						<span className="block text-caption text-text-muted mt-0.5">
							{t("patch.dryRunHint")}
						</span>
					</span>
				</label>
			</div>

			<div className="flex flex-wrap gap-2">
				<button
					onClick={handleVerify}
					disabled={loading || applying || !canVerifyApply}
					title={
						!gamePath.trim()
							? t("patch.selectGame")
							: !sourceOk
								? t("patch.selectSource")
								: undefined
					}
					className="flex items-center gap-1.5 px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded text-body font-medium disabled:opacity-50"
				>
					<ShieldCheck size={16} /> {t("patch.verify")}
				</button>
				<button
					onClick={handleApply}
					disabled={loading || applying || !canVerifyApply}
					title={
						!gamePath.trim()
							? t("patch.selectGame")
							: !sourceOk
								? t("patch.selectSource")
								: undefined
					}
					className="flex items-center gap-1.5 px-3 py-2 bg-accent hover:bg-accent-hover text-white rounded text-body font-medium disabled:opacity-50"
				>
					{applying ? (
						<Loader2 size={16} className="animate-spin" />
					) : (
						<Package size={16} />
					)}{" "}
					{dryRun ? t("patch.planApply") : t("patch.applyBtn")}
				</button>
				<button
					type="button"
					onClick={handlePreviewRollback}
					disabled={loading || applying || !gamePath.trim()}
					className="flex items-center gap-1.5 px-3 py-2 border border-warning text-warning bg-surface hover:bg-warning-muted rounded text-body font-medium disabled:opacity-50"
				>
					<RotateCcw size={16} /> {t("patch.rollbackPreview")}
				</button>
				<button
					onClick={handleRollback}
					disabled={loading || applying}
					className={
						showPartialWarning
							? "flex items-center gap-1.5 px-3 py-2 bg-amber-700 hover:bg-amber-800 text-white rounded text-body font-medium disabled:opacity-50 ring-2 ring-warning ring-offset-1 ring-offset-surface"
							: "flex items-center gap-1.5 px-3 py-2 bg-warning-muted hover:bg-warning-muted/80 text-warning rounded text-body font-medium disabled:opacity-50"
					}
				>
					<RotateCcw size={16} /> {t("patch.rollback")}
				</button>
				<button
					onClick={refreshStatus}
					disabled={loading || applying || !gamePath.trim()}
					className="px-3 py-2 text-body text-text-muted hover:underline disabled:opacity-50"
				>
					{t("patch.refreshStatus")}
				</button>
			</div>

			{applying && (
				<div className="p-3 border border-success bg-success-muted rounded text-body space-y-2">
					<div className="flex items-center gap-2 font-medium">
						<Loader2
							size={16}
							className="animate-spin text-accent-fg"
						/>
						{t("patch.applying")}
					</div>
					{applyProgress && (
						<>
							{applyProgress.phase && (
								<div className="text-caption text-text-muted">
									{t("patch.progress.phase", {
										phase: applyProgress.phase,
									})}
								</div>
							)}
							<div className="w-full bg-border rounded-full h-2">
								<div
									className="bg-accent h-2 rounded-full transition-all"
									style={{
										width: `${
											applyProgress.total > 0
												? Math.min(
														100,
														(applyProgress.current /
															applyProgress.total) *
															100,
													)
												: 0
										}%`,
									}}
								/>
							</div>
							<div className="text-caption tabular-nums">
								{t("patch.progress.counts", {
									current: applyProgress.current,
									total: applyProgress.total,
								})}
							</div>
							{applyProgress.path && (
								<div className="text-caption text-text-muted truncate" title={applyProgress.path}>
									{t("patch.progress.file", {
										path: applyProgress.path,
									})}
								</div>
							)}
						</>
					)}
					<button
						type="button"
						onClick={handleCancelApply}
						disabled={cancellingApply}
						className="w-full py-1.5 bg-red-600 hover:bg-red-700 disabled:opacity-50 text-white rounded text-body font-medium"
					>
						{cancellingApply
							? t("patch.cancelling")
							: t("patch.cancelApply")}
					</button>
				</div>
			)}

			{!applying && showPartialWarning && (
				<div className="p-3 border border-warning bg-warning-muted rounded text-body space-y-2">
					<div className="flex gap-2 text-warning">
						<AlertCircle size={16} className="shrink-0 mt-0.5" />
						<span>{t("patch.partialWarning")}</span>
					</div>
					<button
						type="button"
						onClick={handlePreviewRollback}
						disabled={loading || !gamePath.trim()}
						className="w-full flex items-center justify-center gap-1.5 py-2 border border-warning text-warning bg-surface hover:bg-warning-muted rounded text-body font-medium disabled:opacity-50"
					>
						<RotateCcw size={16} /> {t("patch.rollbackPreview")}
					</button>
					<button
						type="button"
						onClick={handleRollback}
						disabled={loading}
						className="w-full flex items-center justify-center gap-1.5 py-2 bg-amber-700 hover:bg-amber-800 disabled:opacity-50 text-white rounded text-body font-medium"
					>
						<RotateCcw size={16} /> {t("patch.rollbackNow")}
					</button>
				</div>
			)}

			{preview && (
				<div role="status" className="p-3 border border-warning rounded text-body space-y-2">
					<h3 className="font-medium">{t("patch.rollbackPreview.title")}</h3>
					<p className="text-caption">{t("patch.rollbackPreview.hint")}</p>
					<p>{t("patch.rollbackPreview.counts", { restored: preview.restored, deleted: preview.deleted })}</p>
					{preview.messages?.map((message, index) => (
						<p key={index} className="text-caption break-words">{message}</p>
					))}
					{preview.aborted_edited?.length > 0 && (
						<div className="text-warning">
							<p>{t("patch.rollbackPreview.blocked")}</p>
							<ul className="list-disc pl-5 text-caption break-all">
								{preview.aborted_edited.map((path) => <li key={path}>{path}</li>)}
							</ul>
						</div>
					)}
					{preview.torn_deleted?.length > 0 && (
						<div className="text-warning">
							<p>{t("patch.rollbackPreview.forcedDeletes")}</p>
							<ul className="list-disc pl-5 text-caption break-all">
								{preview.torn_deleted.map((path) => <li key={path}>{path}</li>)}
							</ul>
						</div>
					)}
				</div>
			)}

			{status && (
				<div className="p-3 bg-surface-muted rounded text-body space-y-1">
					<div className="font-medium">{t("patch.status", { status: patchValue(status.status) })}</div>
					{status.status === "patched" && (
						<div className="text-caption text-text-muted">
							{t("patch.statusDetail", {
								id: status.patch_id ?? "",
								version: status.patch_version ?? "",
								engine: status.engine ?? "",
								language: status.language ?? "",
								baseline: patchValue(status.baseline ?? ""),
								replaced: status.replaced ?? 0,
								added: status.added ?? 0,
							})}
						</div>
					)}
					{status.status === "interrupted" && (
						<div className="text-caption text-warning">
							{t("patch.interrupted", { id: status.patch_id ?? "" })}
						</div>
					)}
				</div>
			)}

			{verify && (
				<div className="p-3 border border-border rounded text-body space-y-1">
					<div className="font-medium">{t("patch.verifyLine", { outcome: patchValue(verify.outcome) })}</div>
					{verify.tier && (
						<div className="text-caption">{t("patch.tier", { tier: patchValue(verify.tier) })}</div>
					)}
					<div className="text-caption text-text-muted">
						{t("patch.plan", {
							replace: verify.replaced?.length ?? 0,
							add: verify.added?.length ?? 0,
						})}
						{(verify.conflicts?.length ?? 0) > 0 &&
							t("patch.conflicts", { count: verify.conflicts.length })}
					</div>
					{verify.backup_compromised && (
						<div className="text-caption text-warning">
							{t("patch.backupCompromised")}
						</div>
					)}
					{verify.messages?.map((m, i) => (
						<div key={i} className="text-caption text-text-muted">
							{m}
						</div>
					))}
				</div>
			)}

			{applyResult && (
				<div className="p-3 border border-success bg-success-muted rounded text-body space-y-1">
					<div className="font-medium">
						{t(
							applyResult.dry_run ? "patch.planned" : "patch.applied",
							{
								id: applyResult.patch_id,
								version: applyResult.patch_version,
							},
						)}
					</div>
					<div className="text-caption">
						{t("patch.applyDetail", {
							replaced: applyResult.replaced,
							added: applyResult.added,
							baseline: patchValue(applyResult.baseline),
						})}
					</div>
					{applyResult.user_edits_overwritten?.length > 0 && (
						<div className="text-caption text-warning">
							{t("patch.overwrote", {
								files: applyResult.user_edits_overwritten.join(", "),
							})}
						</div>
					)}
				</div>
			)}

			<p className="text-caption text-text-muted">
				{t("patch.applyHelp")}
			</p>
		</>
	);
}
