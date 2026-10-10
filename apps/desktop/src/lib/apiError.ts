/**
 * Map stable English backend error bodies to UI catalog keys.
 *
 * The server/Tauri layer still speaks English (`ApiError` is `(StatusCode, String)`).
 * Toasts interpolate `err.message`, so a Spanish UI was showing those sentences raw.
 * Exact-match the messages we own; leave unknown LocustError Display text as-is
 * (wrapped in a localized HTTP frame when a status prefix is present).
 */

import { getLocale, t, type MessageKey } from "./i18n";

/** Mirrors `locust_server::TRANSLATION_IN_FLIGHT_MESSAGE`. */
export const TRANSLATION_IN_FLIGHT_EN =
	"A translation is still running. Wait for it to finish or cancel it before opening another project.";

/** Mirrors `locust_server::PATCH_APPLY_IN_FLIGHT_MESSAGE`. */
export const PATCH_APPLY_IN_FLIGHT_EN =
	"A patch is already being applied to this game folder. Wait for it to finish or cancel it before starting another.";

/** Mirrors `locust_server::PROJECT_BUSY_MESSAGE`. */
export const PROJECT_BUSY_EN =
	"A project operation is still running. Wait for it to finish before opening another project.";

/** Mirrors `locust_server::INJECT_EMPTY_LANGUAGES_MESSAGE`. */
export const INJECT_EMPTY_LANGUAGES_EN =
	"inject requires at least one language (e.g. [\"es\"])";

const EXACT: Record<string, MessageKey> = {
  "translation changed since it was loaded": "api.error.translationConflict",
  "Configuration could not be loaded. The existing file is protected. Repair the configuration and restart Locust before saving settings.": "settings.configLoadFailed",
  "cost limit must be finite and non-negative": "api.error.invalidCostLimit",
  "provider error: cost limit must be finite and non-negative": "api.error.invalidCostLimit",
	[TRANSLATION_IN_FLIGHT_EN]: "api.error.translationInFlight",
	[PATCH_APPLY_IN_FLIGHT_EN]: "api.error.patchApplyInFlight",
	[PROJECT_BUSY_EN]: "api.error.projectBusy",
	[INJECT_EMPTY_LANGUAGES_EN]: "api.error.injectEmptyLanguages",
	"no project open": "api.error.noProjectOpen",
	"path not found": "api.error.pathNotFound",
	"entry not found": "api.error.entryNotFound",
	"job not found": "api.error.jobNotFound",
	"provider not found": "api.error.providerNotFound",
	"patch backup incomplete: no backup found — factory pristine is unrecoverable":
		"api.error.patchBackupMissing",
	"patch error: a Locust patch is already installed; roll it back before applying a patch without a manifest":
		"api.error.patchManifestlessOverlay",
	"One project database can inject only one target language. Use a separate pivot database for each target language.":
		"api.error.injectSingleTargetLanguage",
	cancelled: "api.error.cancelled",
	"zip_path or zip_url required": "api.error.zipSourceRequired",
	"game_path required": "api.error.gamePathRequired",
	"output_path required": "api.error.outputPathRequired",
	"remote zip too large": "api.error.remoteZipTooLarge",
	"download produced empty file": "api.error.downloadEmpty",
	"format not detected": "api.error.formatNotDetected",
	"Could not detect game format": "api.error.formatNotDetected",
	"path required for Tauri export": "api.error.exportPathRequired",
	"no strings in project — open a game and extract first": "api.error.noStrings",
	"import file is empty": "api.error.importEmpty",
	"batch too large (max 50000 updates)": "api.error.batchTooLarge",
	"no translated, reviewed, or approved strings — nothing to pack yet. Run translate first.":
		"api.error.nothingToPack",
	"handle not found": "api.error.xaiHandleNotFound",
	// project.rs returns InjectionError; also recognize the exact inner reason.
	"injection error: saved Direct project format is incompatible": "api.error.directFormatIncompatible",
	"saved Direct project format is incompatible": "api.error.directFormatIncompatible",
};

// Match the original line so trimming cannot turn a multiline diagnostic into guidance.
// ProviderError adds its prefix in core/error.rs:14; batch entry-ID errors are bare.
const SINGLE_LINE_EXACT: Record<string, MessageKey> = {
	"provider error: xAI session expired — run `locust auth grok` to log in again": "api.error.grokSessionExpired",
	"provider error: provider request timeout: retry deadline exceeded": "api.error.providerConnection",
	"provider error: cannot enforce a cost limit: this provider has no cost estimate": "api.error.unknownProviderCost",
	"provider error: cannot enforce a cost limit: this provider has no valid cost estimate": "api.error.costLimitUnavailable",
	"provider error: cannot enforce a cost limit: preceding calls have unknown cost": "api.error.costLimitUnavailable",
	"provider error: cannot enforce a cost limit: observed calls have unknown cost": "api.error.costLimitUnavailable",
	"provider returned mismatched or duplicate entry IDs": "api.error.providerMalformedResponse",
};

type PrefixRule = { prefix: string; key: MessageKey; detail?: boolean; singleLine?: boolean };

const PREFIXES: PrefixRule[] = [
	// Verified provider Display prefixes. Opaque response bodies belong only in logs.
	...[
		"provider error: OpenAI connection failed: ",
		"provider error: Claude connection failed: ",
		"provider error: DeepL connection failed: ",
		"provider error: Ollama connection failed: ",
		"provider error: Argos connection failed: ",
		"provider error: Google Translate request failed: ",
	].map(prefix => ({ prefix, key: "api.error.providerConnection" as const, singleLine: true })),
	...[
		"provider error: OpenAI malformed response: ",
		"provider error: Claude malformed response: ",
		"provider error: Ollama malformed response: ",
		"provider error: DeepL returned malformed response: ",
		"provider error: Argos returned malformed response: ",
		"provider error: Google Translate returned invalid JSON: ",
		"provider error: could not parse JSON array from response: ",
	].map(prefix => ({ prefix, key: "api.error.providerMalformedResponse" as const, singleLine: true })),
	// Recovery guidance replaces opaque paths and CLI instructions; do not interpolate detail.
	{
		prefix: 'patch error: no injection has been recorded in "',
		key: "api.error.injectionNotRecorded",
	},
	{
		prefix: "patch apply interrupted — run rollback first: ",
		key: "api.error.patchInterrupted",
	},
	// These Display families own only a single original line, before legacy trimming.
	// Verification suffixes can contain long hash/file lists; retain them only in logs.
	{ prefix: "patch verification failed: ", key: "api.error.patchVerificationFailed", singleLine: true },
	{ prefix: "game directory not writable: ", key: "api.error.gameDirNotWritable", singleLine: true },
	{ prefix: "patch already applied: ", key: "api.error.patchAlreadyApplied", singleLine: true },
	{ prefix: "patch downgrade blocked: ", key: "api.error.patchDowngradeBlocked", singleLine: true },
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
	{ prefix: "no translated entries", key: "api.error.noTranslatedToPivot" },
	{
		prefix: "unknown export format: ",
		key: "api.error.unknownExportFormat",
		detail: true,
	},
	{
		prefix: "unknown import format: ",
		key: "api.error.unknownImportFormat",
		detail: true,
	},
];

/** Strip optional `123: ` HTTP status prefix from `request()` throws. */
export function parseApiError(raw: string): { status: number | null; body: string } {
	const m = /^(\d{3}):\s*([\s\S]*)$/.exec(raw);
	if (!m) return { status: null, body: raw };
	return { status: Number(m[1]), body: m[2] };
}

/** Owned PO parser family; keep excerpts opaque and line numbers as exact text. */
function malformedPoStringLine(body: string): string | undefined {
	const match = /^parse error in po: line ([1-9][0-9]*): malformed PO string: ([^\r\n]*)$/.exec(body);
	// JS `$` also matches before a final newline; require the entire original body.
	return match?.[0] === body ? match[1] : undefined;
}

/** Bare Tauri diagnostics only; HTTP request() already logs its original body. */
export function isMalformedPoStringError(raw: string): boolean {
	return malformedPoStringLine(raw) !== undefined;
}

// verify.rs compares SemVer strings; expose only bounded, unambiguous version tokens.
const VERSION_TOKEN = String.raw`[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?`;
const DOWNGRADE_VERSIONS = new RegExp(`^patch downgrade blocked: installed (${VERSION_TOKEN}), incoming (${VERSION_TOKEN})$`);

/** Narrow owned families; IDs/versions are useful, nested diagnostics are not. */
function mapRecoveryDetails(body: string): string | null {
	if (/[\r\n]/.test(body)) return null;
	const backup = /^backup error: backup (\S+) has no readable manifest\.json: [^\r\n]+$/.exec(body);
	if (backup?.[0] === body) return t("api.error.backupManifestUnreadable", { id: backup[1] });

	const destination = /^the export destination is [^\r\n]+; choose a different file$/.exec(body);
	if (destination?.[0] === body) return t("api.error.exportDestinationProtected");

	const versions = DOWNGRADE_VERSIONS.exec(body);
	if (versions?.[0] === body && versions[1].length <= 64 && versions[2].length <= 64) {
		return t("api.error.patchDowngradeVersions", { installed: versions[1], incoming: versions[2] });
	}
	return null;
}

/** Owned provider status/amount/count shapes; never interpolate opaque diagnostics. */
function mapProviderFailure(body: string): string | null {
	if (/[\r\n\u2028\u2029]/.test(body)) return null;
	const exact = SINGLE_LINE_EXACT[body];
	if (exact) return t(exact);

	// Rust http::StatusCode Display includes the canonical reason phrase.
	const credentials = /^provider error: (OpenAI|Claude|DeepL) returned status (?:401(?: Unauthorized)?|403(?: Forbidden)?)(?:: .*)?$/.exec(body);
	if (credentials) return t("api.error.providerCredentialsRejected", { name: credentials[1] });
	if (/^provider error: (?:OpenAI|Claude|DeepL|Ollama|Google Translate|Argos) returned status 429(?: Too Many Requests)?(?:: .*)?$/.test(body)) {
		return t("api.error.providerRateLimited");
	}

	const budget = /^cost limit exceeded: estimated \$([0-9]+(?:\.[0-9]+)?) exceeds limit \$([0-9]+(?:\.[0-9]+)?)$/.exec(body);
	if (budget) return t("api.error.costLimitExceeded", { estimated: budget[1], limit: budget[2] });
	if (/^provider error: translation count mismatch: sent [0-9]+ strings, got [0-9]+ back$/.test(body)) {
		return t("api.error.providerMalformedResponse");
	}
	return null;
}

function mapBody(body: string, originalBody: string): string | null {
	const provider = mapProviderFailure(originalBody);
	if (provider) return provider;

	const exact = EXACT[body];
	if (exact) return t(exact);

	const recovery = mapRecoveryDetails(originalBody);
	if (recovery) return recovery;

	for (const rule of PREFIXES) {
		const candidate = rule.singleLine ? originalBody : body;
		if (rule.singleLine && /[\r\n\u2028\u2029]/.test(candidate)) continue;
		if (candidate.startsWith(rule.prefix)) {
			if (rule.detail) {
				const detail = candidate.slice(rule.prefix.length);
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
	// Strip only request()'s frame, before legacy trimming can hide extra text/newlines.
	const originalBody = raw.replace(/^\d{3}: /, "");
	const poLine = malformedPoStringLine(originalBody);
	// English retains its existing bare diagnostic or HTTP frame byte-for-byte.
	if (poLine !== undefined && getLocale() === "es") {
		return t("api.error.poMalformedString", { line: poLine });
	}

	const trimmed = raw.trim();
	if (!trimmed) return trimmed;

	const { status, body } = parseApiError(trimmed);
	const mapped = mapBody(body, originalBody);
	if (mapped) return mapped;

	if (status != null) {
		return t("api.error.http", { status: String(status), body });
	}
	return body;
}

/** Keep the existing backend message identity when localizing an error for the UI. */
export class ApiError extends Error {
	readonly key: MessageKey | undefined;

	constructor(raw: string) {
		super(localizeApiError(raw));
		this.name = "ApiError";
		this.key = SINGLE_LINE_EXACT[raw.replace(/^\d{3}: /, "")] ?? EXACT[parseApiError(raw.trim()).body];
	}
}
