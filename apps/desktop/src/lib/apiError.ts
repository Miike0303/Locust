/**
 * Map stable English backend error bodies to UI catalog keys.
 *
 * The server/Tauri layer still speaks English (`ApiError` is `(StatusCode, String)`).
 * Toasts interpolate `err.message`, so a Spanish UI was showing those sentences raw.
 * Exact-match the messages we own; leave unknown LocustError Display text as-is
 * (wrapped in a localized HTTP frame when a status prefix is present).
 */

import { t, type MessageKey } from "./i18n";

/** Mirrors `locust_server::TRANSLATION_IN_FLIGHT_MESSAGE`. */
export const TRANSLATION_IN_FLIGHT_EN =
	"A translation is still running. Wait for it to finish or cancel it before opening another project.";

/** Mirrors `locust_server::PATCH_APPLY_IN_FLIGHT_MESSAGE`. */
export const PATCH_APPLY_IN_FLIGHT_EN =
	"A patch is already being applied to this game folder. Wait for it to finish or cancel it before starting another.";

/** Mirrors `locust_server::PROJECT_BUSY_MESSAGE`. */
export const PROJECT_BUSY_EN =
	"An inject is still running. Wait for it to finish before opening another project.";

const EXACT: Record<string, MessageKey> = {
	[TRANSLATION_IN_FLIGHT_EN]: "api.error.translationInFlight",
	[PATCH_APPLY_IN_FLIGHT_EN]: "api.error.patchApplyInFlight",
	[PROJECT_BUSY_EN]: "api.error.projectBusy",
	"no project open": "api.error.noProjectOpen",
	"path not found": "api.error.pathNotFound",
	"entry not found": "api.error.entryNotFound",
	"job not found": "api.error.jobNotFound",
	cancelled: "api.error.cancelled",
	"zip_path or zip_url required": "api.error.zipSourceRequired",
	"game_path required": "api.error.gamePathRequired",
	"output_path required": "api.error.outputPathRequired",
	"remote zip too large": "api.error.remoteZipTooLarge",
	"download produced empty file": "api.error.downloadEmpty",
	"format not detected": "api.error.formatNotDetected",
	"Could not detect game format": "api.error.formatNotDetected",
	"path required for Tauri export": "api.error.exportPathRequired",
};

type PrefixRule = { prefix: string; key: MessageKey; detail?: boolean };

const PREFIXES: PrefixRule[] = [
	{ prefix: "format not found: ", key: "api.error.formatNotFound", detail: true },
	{ prefix: "zip_path not found: ", key: "api.error.zipPathNotFound", detail: true },
	{ prefix: "invalid zip_url: ", key: "api.error.invalidZipUrl", detail: true },
	{
		prefix: "only http/https zip_url allowed",
		key: "api.error.zipUrlScheme",
	},
	{ prefix: "download failed: ", key: "api.error.downloadFailed", detail: true },
	{
		prefix: "download HTTP error: ",
		key: "api.error.downloadHttp",
		detail: true,
	},
	{ prefix: "download body: ", key: "api.error.downloadBody", detail: true },
	{
		prefix: "remote zip too large: ",
		key: "api.error.remoteZipTooLargeDetail",
		detail: true,
	},
	{
		prefix: "output file already exists: ",
		key: "api.error.outputExists",
		detail: true,
	},
];

/** Strip optional `123: ` HTTP status prefix from `request()` throws. */
export function parseApiError(raw: string): { status: number | null; body: string } {
	const m = /^(\d{3}):\s*([\s\S]*)$/.exec(raw);
	if (!m) return { status: null, body: raw };
	return { status: Number(m[1]), body: m[2] };
}

function mapBody(body: string): string | null {
	const exact = EXACT[body];
	if (exact) return t(exact);

	for (const rule of PREFIXES) {
		if (body.startsWith(rule.prefix)) {
			if (rule.detail) {
				const detail = body.slice(rule.prefix.length);
				return t(rule.key, { detail });
			}
			return t(rule.key);
		}
	}
	return null;
}

/**
 * Turn a raw HTTP/Tauri error string into a UI-locale message.
 * Activity logs should keep the raw string; toasts should use this.
 */
export function localizeApiError(raw: string): string {
	const trimmed = raw.trim();
	if (!trimmed) return trimmed;

	const { status, body } = parseApiError(trimmed);
	const mapped = mapBody(body);
	if (mapped) return mapped;

	if (status != null) {
		return t("api.error.http", { status: String(status), body });
	}
	return body;
}
