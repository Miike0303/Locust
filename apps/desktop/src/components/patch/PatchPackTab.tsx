import { Archive, FileArchive, FolderOpen } from "lucide-react";
import type { Dispatch, RefObject, SetStateAction } from "react";
import { type PatchPackResult } from "../../lib/api";
import type { TranslateFn } from "../../lib/i18n";
import { publishCommand } from "../../lib/patchPublish";
import {
	canPackFromRecordings,
	isPackLangSelected,
	packLangFromRecording,
	type RecordedLang,
} from "../../lib/patchRecordings";

interface PatchPackTabProps {
	t: TranslateFn;
	outputPath: string;
	setOutputPath: Dispatch<SetStateAction<string>>;
	pickOutputZip: () => Promise<void>;
	languages: string;
	setLanguages: Dispatch<SetStateAction<string>>;
	recordings: RecordedLang[];
	pristine: boolean;
	setPristine: Dispatch<SetStateAction<boolean>>;
	pristineRecordings: RecordedLang[];
	initialBackupId: string | undefined;
	gamePath: string;
	defaultGamePath: string | undefined;
	pristinePath: string;
	setPristinePath: Dispatch<SetStateAction<string>>;
	pickPristineFolder: () => Promise<void>;
	rjCode: string;
	rjEdited: RefObject<boolean>;
	setRjCode: Dispatch<SetStateAction<string>>;
	detectId: boolean;
	setDetectId: Dispatch<SetStateAction<boolean>>;
	gameVersion: string;
	setGameVersion: Dispatch<SetStateAction<string>>;
	createEntry: boolean;
	setCreateEntry: Dispatch<SetStateAction<boolean>>;
	entryPath: string;
	setEntryPathOverride: Dispatch<SetStateAction<string | null>>;
	pickEntry: () => Promise<void>;
	handlePack: () => Promise<void>;
	loading: boolean;
	applying: boolean;
	packResult: PatchPackResult | null;
	patchValue: (value: string) => string;
	copyPublishCommand: () => Promise<void>;
}

export default function PatchPackTab({
	t,
	outputPath,
	setOutputPath,
	pickOutputZip,
	languages,
	setLanguages,
	recordings,
	pristine,
	setPristine,
	pristineRecordings,
	initialBackupId,
	gamePath,
	defaultGamePath,
	pristinePath,
	setPristinePath,
	pickPristineFolder,
	rjCode,
	rjEdited,
	setRjCode,
	detectId,
	setDetectId,
	gameVersion,
	setGameVersion,
	createEntry,
	setCreateEntry,
	entryPath,
	setEntryPathOverride,
	pickEntry,
	packResult,
	patchValue,
	copyPublishCommand,
}: PatchPackTabProps) {
	return (
		<>
			<div>
				<label className="text-body font-medium">{t("patch.outputZip")}</label>
				<div className="flex gap-2 mt-1">
					<input
						value={outputPath}
						onChange={(e) => setOutputPath(e.target.value)}
						placeholder={t("patch.outputPlaceholder")}
						className="flex-1 p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
					/>
					<button
						onClick={pickOutputZip}
						className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded"
						title={t("common.saveAs")}
					>
						<FileArchive size={16} />
					</button>
				</div>
			</div>

			<div>
				<label className="text-body font-medium">{t("patch.languages")}</label>
				<input
					value={languages}
					onChange={(e) => setLanguages(e.target.value)}
					placeholder={t("patch.languagesPlaceholder")}
					className="mt-1 w-full p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
				/>
				<p className="text-caption font-medium text-text-muted mt-2">
					{t("patch.recordedLangs")}
				</p>
				{recordings.length === 0 ? (
					<p className="text-caption text-warning mt-1">
						{t("patch.recordedLangsEmpty")}
					</p>
				) : (
					<div className="flex flex-wrap gap-1.5 mt-1">
						{recordings.map((lang) => {
							const key = lang ?? "(unspecified)";
							const selected = isPackLangSelected(languages, lang);
							return (
								<button
									key={key}
									type="button"
									onClick={() =>
										setLanguages(packLangFromRecording(lang))
									}
									className={`px-2 py-0.5 rounded text-caption font-medium border ${
										selected
											? "border-accent-fg bg-accent-muted text-accent-fg"
											: "border-border text-text hover:bg-surface-muted"
									}`}
								>
									{lang ?? t("patch.recordedUnspecified")}
								</button>
							);
						})}
					</div>
				)}
				<p className="text-caption text-text-muted mt-1">
					{t("patch.languagesHint")}
				</p>
			</div>

			<label className="flex items-start gap-2 text-body cursor-pointer">
				<input
					type="checkbox"
					className="mt-0.5 border border-border bg-surface text-text accent-accent focus:outline-none focus:ring-2 focus:ring-accent-fg"
					checked={pristine}
					onChange={(e) => setPristine(e.target.checked)}
				/>
				<span>
					<span className="font-medium">{t("patch.pristine")}</span>
					<span className="block text-caption text-text-muted mt-0.5">
						{t("patch.pristineHint")}
					</span>
				</span>
			</label>

			<div>
			<label className="text-body font-medium">{t("patch.pristinePath")}</label>
			{pristine && (pristineRecordings.some(lang => isPackLangSelected(languages, lang)) || (initialBackupId && gamePath.trim() === defaultGamePath?.trim())) && !pristinePath.trim() && <p className="mt-1 text-caption text-success">{t("patch.injectionBackup")}</p>}
				<div className="flex gap-2 mt-1">
					<input
						value={pristinePath}
						onChange={(e) => {
							const value = e.target.value;
							setPristinePath(value);
							if (value.trim()) setPristine(true);
						}}
						placeholder={t("patch.pristinePathPlaceholder")}
						className="flex-1 p-2 border border-border rounded bg-surface text-body text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg"
					/>
					<button
						type="button"
						onClick={() => {
							void pickPristineFolder();
						}}
						className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded"
						title={t("common.browse")}
					>
						<FolderOpen size={16} />
					</button>
				</div>
				<p className="text-caption text-text-muted mt-1">
					{t("patch.pristinePathHint")}
				</p>
			</div>

			<details className="border border-border rounded p-3">
				<summary className="text-body font-medium cursor-pointer">{t("patch.publish.title")}</summary>
				<div className="mt-3 space-y-3">
					<label className="block text-body">
						{t("patch.publish.rjCode")}
						<input value={rjCode} onChange={(e) => { rjEdited.current = true; setRjCode(e.target.value); }} className="block w-full mt-1 p-2 border border-border rounded bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg" />
					</label>
					<label className="flex items-center gap-2 text-body cursor-pointer">
						<input type="checkbox" checked={!detectId} onChange={(e) => setDetectId(!e.target.checked)} />
						{t("patch.publish.noDetect")}
					</label>
					<label className="block text-body">
						{t("patch.publish.gameVersion")}
						<input value={gameVersion} onChange={(e) => setGameVersion(e.target.value)} className="block w-full mt-1 p-2 border border-border rounded bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg" />
					</label>
					<label className="flex items-center gap-2 text-body cursor-pointer">
						<input type="checkbox" checked={createEntry} onChange={(e) => setCreateEntry(e.target.checked)} />
						{t("patch.publish.createEntry")}
					</label>
					{createEntry && <div className="flex items-end gap-2">
						<label className="flex-1 min-w-0 text-body">
							{t("patch.publish.entryPath")}
							<input value={entryPath} onChange={(e) => setEntryPathOverride(e.target.value)} className="block w-full mt-1 p-2 border border-border rounded bg-surface text-text placeholder:text-text-muted focus:outline-none focus:ring-2 focus:ring-accent-fg focus:border-accent-fg" />
						</label>
						<button type="button" onClick={() => { void pickEntry(); }} title={t("common.browse")} aria-label={t("patch.publish.saveEntry")} className="px-3 py-2 bg-surface-muted text-text hover:bg-surface rounded"><FolderOpen size={16} /></button>
					</div>}
				</div>
			</details>

			{packResult && (
				<div className="p-3 border border-success bg-success-muted rounded text-body space-y-1">
					<div className="font-medium">
						{t("patch.packed", {
							id: packResult.patch_id,
							version: packResult.patch_version,
						})}
					</div>
					<div className="text-caption text-text-muted space-y-0.5">
						<div>
							{t("patch.packedStats", {
								files: packResult.files_packed,
								bytes: packResult.size_bytes,
								tier: patchValue(packResult.tier),
							})}
						</div>
						<div>
							{t("patch.packedMeta", {
								engine: packResult.engine,
								language: packResult.language,
							})}
							{packResult.translated_strings != null &&
								t("patch.packedStrings", {
									count: packResult.translated_strings,
								})}
						</div>
						<div className="break-all">{packResult.output_path}</div>
					</div>
					<div className="text-caption">
						{t("patch.publish.identity", { code: packResult.game?.store_ids.dlsite ?? t("patch.publish.noCode"), count: packResult.game?.fingerprint_count ?? 0 })}
					</div>
					{packResult.entry_path && <div className="pt-2 space-y-2">
						<p className="text-caption">{t("patch.publish.commandHint")}</p>
						<pre className="text-caption whitespace-pre-wrap break-all select-text">{publishCommand(packResult.output_path, packResult.entry_path)}</pre>
						<button type="button" onClick={() => { void copyPublishCommand(); }} className="px-3 py-1 bg-surface-muted text-text hover:bg-surface rounded text-caption">{t("common.copy")}</button>
					</div>}
					{packResult.messages?.map((m, i) => (
						<div
							key={i}
							className="text-caption text-warning"
						>
							{m}
						</div>
					))}
				</div>
			)}

			<p className="text-caption text-text-muted">
				{t("patch.packHelp")}
			</p>
		</>
	);
}

export function PatchPackActions({ t, handlePack, loading, applying, recordings, languages }: Pick<PatchPackTabProps, "t" | "handlePack" | "loading" | "applying" | "recordings" | "languages">) {
	return <button
		onClick={handlePack}
		disabled={
			loading ||
			applying ||
			!canPackFromRecordings(recordings, languages)
		}
		className="flex items-center gap-1.5 px-3 py-2 bg-accent hover:bg-accent-hover text-white rounded text-body font-medium disabled:opacity-50"
	>
		<Archive size={16} /> {t("patch.packBtn")}
	</button>;
}
