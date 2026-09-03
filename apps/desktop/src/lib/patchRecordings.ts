/**
 * Pack language vs injection recordings (`list_recorded_langs`).
 * JSON `null` is the language-unspecified recording; the pack field uses "".
 */

export type RecordedLang = string | null;

export function packLangFromRecording(lang: RecordedLang): string {
	return lang ?? "";
}

export function isPackLangSelected(
	languagesField: string,
	recording: RecordedLang,
): boolean {
	return languagesField.trim() === packLangFromRecording(recording);
}

/** Empty field is auto only when exactly one recording exists. */
export function canPackFromRecordings(
	recordings: RecordedLang[],
	languagesField: string,
): boolean {
	if (recordings.length === 0) return false;
	const field = languagesField.trim();
	if (!field) return recordings.length === 1;
	return recordings.some((r) => r === field);
}

export function preferredPackLang(
	recordings: RecordedLang[],
	configTarget: string | undefined,
	current: string,
): string {
	if (current.trim()) return current;
	if (configTarget && recordings.some((r) => r === configTarget)) {
		return configTarget;
	}
	if (recordings.length === 1) {
		return packLangFromRecording(recordings[0]);
	}
	return current;
}
