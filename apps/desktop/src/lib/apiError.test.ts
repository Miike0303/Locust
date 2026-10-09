import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { useLogStore } from "../stores/logStore";
import {
	ApiError,
	INJECT_EMPTY_LANGUAGES_EN,
	localizeApiError,
	parseApiError,
	PATCH_APPLY_IN_FLIGHT_EN,
	PROJECT_BUSY_EN,
	TRANSLATION_IN_FLIGHT_EN,
} from "./apiError";
import { setLocale } from "./i18n";

// parseApiError
assert.deepEqual(parseApiError("409: hello"), { status: 409, body: "hello" });
assert.deepEqual(parseApiError("no project open"), {
	status: null,
	body: "no project open",
});

// Spanish UI: known bodies must not stay English
setLocale("es");
assert.equal(
	localizeApiError(`409: ${TRANSLATION_IN_FLIGHT_EN}`),
	"Todavía hay una traducción en curso. Espere a que termine o cancele antes de abrir otro proyecto.",
);
assert.equal(
	localizeApiError(PATCH_APPLY_IN_FLIGHT_EN),
	"Ya se está aplicando un parche a esta carpeta de juego. Espere a que termine o cancele antes de iniciar otro.",
);
assert.equal(
	localizeApiError(`409: ${PROJECT_BUSY_EN}`),
	"Todavía hay una operación de proyecto en curso. Espere a que termine antes de abrir otro proyecto.",
);
assert.equal(localizeApiError("404: no project open"), "No hay ningún proyecto abierto.");
assert.equal(localizeApiError("path not found"), "No se encontró la ruta.");
assert.equal(
	localizeApiError("422: format not detected"),
	"No se pudo detectar el formato del juego.",
);
assert.equal(
	localizeApiError("format not found: wolf-rpg"),
	"Formato no encontrado: wolf-rpg",
);
assert.equal(
	localizeApiError("409: output file already exists: C:\\a.locust.db"),
	"El archivo de salida ya existe: C:\\a.locust.db",
);
assert.equal(
	localizeApiError(`400: ${INJECT_EMPTY_LANGUAGES_EN}`),
	"La inyección necesita al menos un idioma (por ejemplo es).",
);
assert.equal(
	localizeApiError("no strings in project — open a game and extract first"),
	"Este proyecto no tiene cadenas — abra un juego y extraiga primero.",
);
assert.equal(
	localizeApiError("no translated entries in C:\\x.locust.db to pivot from"),
	"No hay traducciones para pivotar. Traduzca o importe primero.",
);
assert.equal(localizeApiError("import file is empty"), "El archivo de importación está vacío.");
assert.equal(
	localizeApiError("unknown export format: csv"),
	"Formato de exportación desconocido: csv. Use po o xliff.",
);
assert.equal(
	localizeApiError("unknown import format: yaml"),
	"Formato de importación desconocido: yaml. Use po o xliff.",
);
assert.equal(
	localizeApiError("handle not found"),
	"Esa sesión de inicio de xAI caducó o no se encontró. Vuelva a iniciar sesión.",
);

// Unknown bodies keep the English detail but get a Spanish frame
assert.equal(
	localizeApiError("500: weird backend detail"),
	"Error del servidor 500: weird backend detail",
);

// Negative: English locale still returns English for the same body
setLocale("en");
assert.equal(
	localizeApiError(`409: ${TRANSLATION_IN_FLIGHT_EN}`),
	TRANSLATION_IN_FLIGHT_EN,
);
assert.equal(localizeApiError(PROJECT_BUSY_EN), PROJECT_BUSY_EN);
assert.equal(
	localizeApiError(INJECT_EMPTY_LANGUAGES_EN),
	"Inject needs at least one language (for example es).",
);
assert.equal(localizeApiError("no project open"), "No project is open.");
assert.equal(
	localizeApiError("unknown export format: csv"),
	"Unknown export format: csv. Use po or xliff.",
);
assert.equal(
	localizeApiError("handle not found"),
	"That xAI login session expired or was not found. Start login again.",
);

// Mirrors the owned Display/server messages, including the CLI-oriented pack suffix.
const recoveryErrors = [
	{
		name: "missing injection recording",
		body: 'patch error: no injection has been recorded in "C:\\fixtures\\project.locust.db". `locust patch` packs exactly ' +
			'the files a recorded injection wrote — never a list guessed from the database. Run ' +
			'`locust inject "C:\\fixtures\\game" -P "C:\\fixtures\\project.locust.db" --direct -l es` first, then re-run patch.',
		en: "No injection has been recorded for this project. Inject the translation first, then create the patch in Patch → Pack.",
		es: "No hay ninguna inyección registrada para este proyecto. Inyecte la traducción primero y después cree el parche en Parche → Empaquetar.",
	},
	{
		name: "interrupted patch application",
		body: "patch apply interrupted — run rollback first: run patch-rollback first",
		en: "Patch application was interrupted. Roll back the patch before applying it again.",
		es: "La aplicación del parche se interrumpió. Revierta el parche antes de volver a aplicarlo.",
	},
	{
		name: "missing patch backup",
		body: "patch backup incomplete: no backup found — factory pristine is unrecoverable",
		en: "No patch backup was found. Locust cannot restore the original game files.",
		es: "No se encontró ninguna copia de seguridad del parche. Locust no puede restaurar los archivos originales del juego.",
	},
	{
		name: "missing provider",
		body: "provider not found",
		en: "Provider not found. Choose another provider or configure its API key in Settings.",
		es: "No se encontró el proveedor. Seleccione otro proveedor o configure su clave API en Ajustes.",
	},
	{
		name: "multiple injection target languages",
		body: "One project database can inject only one target language. Use a separate pivot database for each target language.",
		en: "One project database can inject only one target language. Use a separate pivot database for each target language.",
		es: "Una base de datos de proyecto solo puede inyectar un idioma de destino. Use un proyecto intermedio con su propia base de datos para cada idioma de destino.",
	},
] as const;

for (const recovery of recoveryErrors) {
	for (const locale of ["es", "en"] as const) {
		test(`${recovery.name} shows desktop guidance for bare and HTTP errors in ${locale}`, () => {
			setLocale(locale);
			assert.equal(localizeApiError(recovery.body), recovery[locale]);
			for (const status of [400, 409, 500]) {
				assert.equal(localizeApiError(`${status}: ${recovery.body}`), recovery[locale]);
			}
		});
	}
}

const recoveryPrefixes = [
	{ prefix: 'patch error: no injection has been recorded in "', recovery: recoveryErrors[0] },
	{ prefix: "patch apply interrupted — run rollback first: ", recovery: recoveryErrors[1] },
];

test("recovery prefix guidance never interpolates opaque paths, CLI commands or other suffixes", () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const { prefix, recovery } of recoveryPrefixes) {
			for (const suffix of [String.raw`C:\fixtures\日本語\$&\{detail}" --direct`, "opaque\nsecond line", "$' $$ $`"]) {
				assert.equal(localizeApiError(`${prefix}${suffix}`), recovery[locale]);
				assert.equal(localizeApiError(`500: ${prefix}${suffix}`), recovery[locale]);
			}
		}
	}
});

test("near-miss recovery messages retain unknown-error fallback in both locales", () => {
	const nearMisses = [
		...recoveryErrors.map(({ body }) => `extra prefix: ${body}`),
		...recoveryErrors.slice(2).map(({ body }) => `${body} extra detail`),
		String.raw`patch error: no injection has been recorded in C:\fixtures\project.locust.db`,
		'patch error: no injection has been recorded for "project.locust.db"',
		'no injection has been recorded in "project.locust.db"',
		"patch apply interrupted - run rollback first: run patch-rollback first",
		"patch apply interrupted — run recovery first: run patch-rollback first",
		"patch apply interrupted — run rollback first",
		"patch apply interrupted — run rollback first:",
		"patch backup incomplete: interrupted apply has no valid backup/manifest.json — refusing",
		"patch backup incomplete: no backup found — pristine is unrecoverable",
		"provider not found: example",
		"provider not configured: example",
		"Provider not found",
		recoveryErrors[4].body.replace("only one", "multiple"),
		"weird backend detail",
	];
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const raw of nearMisses) {
			assert.equal(localizeApiError(raw), raw);
			assert.equal(localizeApiError(`400: ${raw}`),
				`${locale === "es" ? "Error del servidor" : "Server error"} 400: ${raw}`);
			assert.equal(new ApiError(raw).key, undefined);
		}
	}
});

const excerpts = ['"unterminated', String.raw`"$& C:\games\日本語\dialog.po`, '"café" extra', ""];
const poError = (line: string, excerpt: string) =>
	`parse error in po: line ${line}: malformed PO string: ${excerpt}`;
const spanishGuidance = (line: string) =>
	`Texto entre comillas no válido en el archivo PO, línea ${line}. Compruebe las comillas de apertura y cierre.`;
const unrelatedErrors = [
	"weird backend detail", "IO error: file unavailable",
	"parse error in xliff: line 2: malformed PO string: quote",
	"parse error in po2: line 2: malformed PO string: quote",
	"parse error in PO: line 2: malformed PO string: quote",
	"parse error in po: line 2: invalid msgstr: quote",
	...["0", "-1", "01", "+1"].map(line => poError(line, "quote")),
	`prefix ${poError("2", "quote")}`, ` ${poError("2", "quote")}`,
	poError("2", "first\nsecond"), poError("2", "first\rsecond"),
	`${poError("2", "quote")}\n`, `${poError("2", "quote")}\r\n`,
];

test("Spanish PO failures show the exact line and quote guidance without opaque excerpts", () => {
	setLocale("es");
	for (const line of ["1", "27", "9007199254740993"]) {
		for (const excerpt of excerpts) {
			const raw = poError(line, excerpt);
			assert.equal(localizeApiError(raw), spanishGuidance(line));
			assert.equal(localizeApiError(`400: ${raw}`), spanishGuidance(line));
		}
	}
});

test("English PO failures retain the existing bare text and HTTP frame byte-for-byte", () => {
	setLocale("en");
	for (const excerpt of excerpts) {
		const raw = poError("27", excerpt);
		assert.equal(localizeApiError(raw), raw.trim());
		assert.equal(localizeApiError(`400: ${raw}`), `Server error 400: ${raw.trim()}`);
	}
});

test("unowned parse/IO errors, prefixes, invalid lines and multiline diagnostics keep fallback behavior", () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const raw of unrelatedErrors) {
			assert.equal(localizeApiError(raw), raw.trim());
			assert.equal(localizeApiError(`400: ${raw}`),
				`${locale === "es" ? "Error del servidor" : "Server error"} 400: ${raw.trim()}`);
		}
	}
});

let api: typeof import("./api");
let rawFailure = "";
let rejectWithError = false;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
const originalFetch = globalThis.fetch;
const tauriCommands: string[] = [];
before(async () => {
	Object.defineProperty(globalThis, "window", { configurable: true, value: {
		__TAURI_INTERNALS__: { invoke: async (cmd: string) => {
			if (cmd === "get_server_port") return 7842;
			tauriCommands.push(cmd);
			throw rejectWithError ? new Error(rawFailure) : rawFailure;
		} },
	} });
	globalThis.fetch = async () => new Response(rawFailure, { status: 400 });
	api = await import("./api");
});
after(() => {
	globalThis.fetch = originalFetch;
	if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
	else Reflect.deleteProperty(globalThis, "window");
	useLogStore.getState().clear();
	setLocale("en");
});

test("real Tauri import localizes the message and logs the original string/Error detail exactly once", async () => {
	for (const locale of ["en", "es"] as const) {
		setLocale(locale);
		for (const asError of [false, true]) {
			rejectWithError = asError;
			for (const excerpt of excerpts) {
				rawFailure = poError("27", `${excerpt}   `);
				useLogStore.getState().clear();
				await assert.rejects(api.importTranslations("po", "fixture.po"), (error: unknown) => {
					assert.ok(error instanceof ApiError);
					assert.equal(error.message, locale === "es" ? spanishGuidance("27") : rawFailure.trim());
					return true;
				});
				assert.equal(tauriCommands[tauriCommands.length - 1], "import_translations");
				const entries = useLogStore.getState().entries;
				assert.equal(entries.length, 1);
				assert.deepEqual([entries[0].level, entries[0].message, entries[0].detail, entries[0].source],
					["error", locale === "es" ? "Error de Tauri: import_translations" : "Tauri error: import_translations", rawFailure, "api"]);
			}
		}
	}
});

test("real Tauri import does not add logs for unrelated failures", async () => {
	setLocale("es");
	for (const raw of unrelatedErrors) {
		rawFailure = raw;
		useLogStore.getState().clear();
		await assert.rejects(api.importTranslations("po", "fixture.po"), ApiError);
		assert.equal(useLogStore.getState().entries.length, 0);
	}
});

test("real HTTP PO import retains its raw body log without duplicate localization logging", async () => {
	setLocale("es");
	for (const excerpt of excerpts) {
		rawFailure = poError("27", excerpt);
		useLogStore.getState().clear();
		await assert.rejects(api.importPo("fixture content"), { message: spanishGuidance("27") });
		const entries = useLogStore.getState().entries;
		assert.equal(entries.length, 1);
		assert.equal(entries[0].detail, rawFailure);
		assert.equal(entries[0].message, "API 400: /import/po");
	}
});

test("Tauri string/Error rejections use recovery guidance without changing existing logging", async () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const asError of [false, true]) {
			rejectWithError = asError;
			for (const recovery of recoveryErrors) {
				rawFailure = `${recovery.body}   `;
				useLogStore.getState().clear();
				await assert.rejects(api.importTranslations("po", "fixture.po"), (error: unknown) => {
					assert.ok(error instanceof ApiError);
					assert.equal(error.message, recovery[locale]);
					return true;
				});
				assert.equal(useLogStore.getState().entries.length, 0);
			}
		}
	}
});

test("HTTP recovery failures localize guidance and preserve the complete raw activity-log detail", async () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const recovery of recoveryErrors) {
			rawFailure = `${recovery.body}   `;
			useLogStore.getState().clear();
			await assert.rejects(api.patchPack({ game_path: "fixture", output_path: "fixture.zip" }), (error: unknown) => {
				assert.ok(error instanceof ApiError);
				assert.equal(error.message, recovery[locale]);
				return true;
			});
			const entries = useLogStore.getState().entries;
			assert.equal(entries.length, 1);
			assert.deepEqual([entries[0].level, entries[0].message, entries[0].detail, entries[0].source],
				["error", "API 400: /patch/pack", rawFailure, "api"]);
		}
	}
});

test("detection failure recognition is unchanged in both locales", async () => {
	const { isDetectionFailure } = await import("./openProjectFlow");
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		assert.ok(isDetectionFailure(localizeApiError("format not detected")));
		assert.ok(isDetectionFailure(localizeApiError("422: Could not detect game format")));
		assert.equal(isDetectionFailure(localizeApiError(poError("27", "quote"))), false);
	}
});
