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
import { getLocale, setLocale, t } from "./i18n";

// core/validation.rs's fixed refusal (no source/target prose) + core/error.rs Display.
const yurisEncodingRefusal = "injection error: translation cannot be encoded as Shift-JIS for the declared YU-RIS quoted literal; injection is refused before backup or output; the original game and saved translation remain unchanged";
test("inject language and output containment errors have EN/ES guidance", () => {
	const cases = [
		["invalid target language code: use 1 to 35 ASCII letters, digits, '-' or '_'", "api.error.invalidTargetLanguageCode"],
		["Replace destination must be a direct child of output_dir", "api.error.replaceOutputContainment"],
	] as const;
	for (const locale of ["en", "es"] as const) {
		setLocale(locale);
		for (const [reason, key] of cases) {
			for (const frame of ["", "500: "]) {
				const error = new ApiError(`${frame}injection error: ${reason}`);
				assert.equal(error.key, key);
				assert.equal(error.message, t(key));
			}
		}
	}
});
const yurisEncodingGuidance = {
	en: "The translation cannot be represented in Shift-JIS for the declared YU-RIS quoted literal. Injection was refused before backup or output creation; the original game and saved translation remain unchanged. Run Validate and review encoding issues. Use a supported target language; do not remove accents to force injection. This does not enable Unicode in the game.",
	es: "La traducción no se puede representar en Shift-JIS para el literal entre comillas de YU-RIS declarado. Se rechazó la inyección antes de crear copias de seguridad o archivos de salida; el juego original y la traducción guardada permanecen intactos. Ejecute Validar y revise los problemas de codificación. Use un idioma de destino compatible; no elimine los acentos para forzar la inyección. Esto no habilita Unicode en el juego.",
};

test("YU-RIS encoding refusal gives exact Spanish guidance for bare Tauri and HTTP messages", () => {
	setLocale("es");
	for (const frame of ["", "500: "]) {
		assert.equal(localizeApiError(frame + yurisEncodingRefusal), yurisEncodingGuidance.es);
		const error = new ApiError(frame + yurisEncodingRefusal);
		assert.equal(error.message, yurisEncodingGuidance.es);
		assert.equal(error.key, "api.error.yurisEncodingRefused");
	}
});

test("ApiError retains the original diagnostic additively across existing message and key semantics", () => {
	for (const locale of ["en", "es"] as const) {
		setLocale(locale);
		for (const raw of [yurisEncodingRefusal, `500: ${yurisEncodingRefusal}`, "404: no project open", "unknown diagnostic  \n", "500: unknown diagnostic  \n"]) {
			const error = new ApiError(raw);
			assert.equal(error.raw, raw);
			assert.equal(error.name, "ApiError");
			assert.equal(error.message, localizeApiError(raw));
			assert.equal(error.key, raw.includes(yurisEncodingRefusal)
				? "api.error.yurisEncodingRefused" : raw === "404: no project open" ? "api.error.noProjectOpen" : undefined);
		}
	}
});

test("YU-RIS encoding guidance retains English fallback and has matching EN/ES catalog copy", () => {
	for (const locale of ["en", "es"] as const) {
		setLocale(locale);
		// Lookup also checks copy parity without extending the English mapping policy.
		assert.equal(t("api.error.yurisEncodingRefused"), yurisEncodingGuidance[locale]);
	}
	setLocale("en");
	assert.equal(localizeApiError(yurisEncodingRefusal), yurisEncodingRefusal);
	assert.equal(localizeApiError(`500: ${yurisEncodingRefusal}`), `Server error 500: ${yurisEncodingRefusal}`);
});

test("YU-RIS encoding near-misses, whitespace and multiline messages preserve shared fallback without side effects", () => {
	const bodies = [
		yurisEncodingRefusal + " extra", "prefix: " + yurisEncodingRefusal,
		yurisEncodingRefusal.replace("injection error: ", ""),
		yurisEncodingRefusal.replace("Shift-JIS", "UTF-8"),
		yurisEncodingRefusal.replace("YU-RIS quoted literal", "YU-RIS caption"),
		yurisEncodingRefusal.replace("remain unchanged", "were changed"),
		"encoding error: unknown synthetic diagnostic",
		" " + yurisEncodingRefusal, yurisEncodingRefusal + " ",
		...["\n", "\r", "\r\n", "\u2028", "\u2029"].flatMap(line => [
			yurisEncodingRefusal + line, line + yurisEncodingRefusal, yurisEncodingRefusal + line + "opaque",
		]),
	];
	for (const locale of ["en", "es"] as const) {
		setLocale(locale);
		const state = useLogStore.getState();
		for (const body of bodies) {
			assert.equal(localizeApiError(body), body.trim());
			assert.equal(localizeApiError(`500: ${body}`), `${locale === "es" ? "Error del servidor" : "Server error"} 500: ${body.trim()}`);
			assert.equal(new ApiError(body).key, undefined);
			assert.equal(new ApiError(`500: ${body}`).key, undefined);
		}
		assert.equal(getLocale(), locale);
		assert.equal(useLogStore.getState(), state, "localization must not write activity state");
	}
});

const manifestlessOverlayError = "patch error: a Locust patch is already installed; roll it back before applying a patch without a manifest";
for (const locale of ["en", "es"] as const) {
	test(`manifestless overlay refusal gives Patch window rollback guidance in ${locale}`, () => {
		setLocale(locale);
		const expected = locale === "es"
			? "Ya hay un parche de Locust instalado. Use Revertir en la ventana Parche antes de aplicar un parche sin manifiesto."
			: "A Locust patch is already installed. Use Rollback in the Patch window before applying a patch without a manifest.";
		for (const raw of [manifestlessOverlayError, `400: ${manifestlessOverlayError}`]) {
			assert.equal(localizeApiError(raw), expected);
			const error = new ApiError(raw);
			assert.equal(error.message, expected);
			assert.equal(error.key, "api.error.patchManifestlessOverlay");
		}
	});

	test(`manifestless overlay mapping is exact in ${locale}`, () => {
		setLocale(locale);
		for (const raw of [
			`${manifestlessOverlayError} extra`,
			`extra: ${manifestlessOverlayError}`,
			manifestlessOverlayError.replace("without a manifest", "with a manifest"),
			"patch error: unrelated diagnostic",
		]) {
			assert.equal(localizeApiError(raw), raw);
			assert.equal(localizeApiError(`400: ${raw}`),
				`${locale === "es" ? "Error del servidor" : "Server error"} 400: ${raw}`);
			assert.equal(new ApiError(raw).key, undefined);
		}
	});
}

const capacityError = (count: string) =>
	`injection error: ${count} translation(s) exceed known binary slot capacity; Replace is refused before backup or output; run validation and resolve the capacity issues`;
for (const locale of ["en", "es"] as const) {
	test(`known capacity refusal maps only the exact count-only diagnostic in ${locale}`, () => {
		setLocale(locale);
		for (const count of ["1", "27", "9007199254740993"]) {
			const expected = locale === "en"
				? `${count} translation(s) exceed known binary slot capacity. The translated copy was refused before backup or output creation. Run Validate and resolve the capacity issues.`
				: `${count} traducción(es) superan la capacidad conocida de los espacios binarios. Se rechazó la copia traducida antes de crear copias de seguridad o archivos de salida. Ejecute Validar y resuelva los problemas de capacidad.`;
			for (const frame of ["", "422: "]) {
				assert.equal(localizeApiError(frame + capacityError(count)), expected);
				assert.equal(new ApiError(frame + capacityError(count)).message, expected);
			}
		}
	});

	test(`capacity near-misses and prose-bearing errors retain existing fallback in ${locale}`, () => {
		setLocale(locale);
		const raw = capacityError("2");
		for (const body of [
			...["0", "02", "-2", "+2", "2.5", "N", "private source / translated prose"].map(capacityError),
			`prefix: ${raw}`, raw.replace("Replace", "Add"), raw.replace("known binary", "binary"),
			`${raw}; source: private source; translation: private translation`,
			...['\n', '\r\n', '\u2028', '\u2029'].flatMap(line => [raw + line, raw + line + "opaque", line + raw]),
		]) {
			assert.equal(localizeApiError(body), body.trim());
			assert.equal(localizeApiError(`422: ${body}`),
				`${locale === "en" ? "Server error" : "Error del servidor"} 422: ${body.trim()}`);
		}
	});
}

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

// Verified against core error.rs and the corresponding construction sites.
const secondBatchErrors = [
	{
		name: "patch verification mismatch",
		body: `patch verification failed: data/Actors.json: expected ${"a".repeat(64)}, found ${"b".repeat(64)}`,
		en: "The selected game files do not match what the patch expects. The game or version may be different, or the files may already be modified. Check the game folder, or use Force only if you are sure.",
		es: "Los archivos del juego seleccionado no coinciden con los que espera el parche. Puede tratarse de otro juego o versión, o de archivos ya modificados. Compruebe la carpeta del juego o use Forzar solo si está seguro.",
	},
	{
		name: "unwritable game directory",
		body: String.raw`game directory not writable: C:\Program Files\Game (IO error: Access is denied. (os error 5))`,
		en: "Locust cannot write to the game folder. Close the game, check folder permissions, or move the game out of protected folders such as Program Files.",
		es: "Locust no puede escribir en la carpeta del juego. Cierre el juego, compruebe los permisos de la carpeta o mueva el juego fuera de carpetas protegidas como Program Files.",
	},
	{
		name: "damaged backup manifest",
		body: "backup error: backup 20261009_120000_fixture has no readable manifest.json: JSON error: EOF while parsing a value at line 1 column 0",
		en: "Backup 20261009_120000_fixture is damaged and cannot be restored. It is listed in Settings → Data.",
		es: "La copia de seguridad 20261009_120000_fixture está dañada y no se puede restaurar. Figura en Ajustes → Datos.",
	},
	{
		name: "protected export destination",
		body: "the export destination is the project database itself; choose a different file",
		en: "The chosen export file is the project database or one of its companion files. Choose a different file name.",
		es: "El archivo elegido para exportar es la base de datos del proyecto o uno de sus archivos auxiliares. Elija otro nombre de archivo.",
	},
	{
		name: "incompatible saved Direct project format",
		body: "injection error: saved Direct project format is incompatible",
		en: "The saved Direct project format is incompatible. Locust cannot resume this project with the selected format.",
		es: "El formato del proyecto guardado con inyección directa es incompatible. Locust no puede reanudar este proyecto con el formato seleccionado.",
	},
	{
		name: "patch already installed",
		body: "patch already applied: fixture@1.0.0",
		en: "This patch is already installed. Roll it back before reinstalling it, or use Force.",
		es: "Este parche ya está instalado. Reviértalo antes de volver a instalarlo o use Forzar.",
	},
	{
		name: "patch downgrade",
		body: "patch downgrade blocked: installed 2.0.0, incoming 1.0.0",
		en: "A newer patch version is installed (2.0.0); the selected version is 1.0.0. Roll back the installed patch before installing an older version.",
		es: "Hay una versión más reciente del parche instalada (2.0.0); la versión seleccionada es 1.0.0. Revierta el parche instalado antes de instalar una versión anterior.",
	},
] as const;

for (const recovery of secondBatchErrors) {
	for (const locale of ["es", "en"] as const) {
		test(`${recovery.name} shows second-batch guidance for bare and HTTP errors in ${locale}`, () => {
			setLocale(locale);
			assert.equal(localizeApiError(recovery.body), recovery[locale]);
			for (const status of [400, 409, 500]) {
				assert.equal(localizeApiError(`${status}: ${recovery.body}`), recovery[locale]);
			}
		});
	}
}

const secondBatchPrefixes = [
	{ prefix: "patch verification failed: ", recovery: secondBatchErrors[0] },
	{ prefix: "game directory not writable: ", recovery: secondBatchErrors[1] },
	{ prefix: "patch already applied: ", recovery: secondBatchErrors[5] },
];

const downgradeGuidance = {
	en: "A newer patch version is installed. Roll back the installed patch before installing an older version.",
	es: "Hay una versión más reciente del parche instalada. Revierta el parche instalado antes de instalar una versión anterior.",
};

test("second-batch prefixes hide opaque and long diagnostic suffixes in both locales", () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const { prefix, recovery } of secondBatchPrefixes) {
			for (const suffix of [String.raw`C:\fixtures\日本語\$&\{detail}`, "opaque; ".repeat(200), "$' $$ $`"]) {
				assert.equal(localizeApiError(`${prefix}${suffix}`), recovery[locale]);
				assert.equal(localizeApiError(`500: ${prefix}${suffix}`), recovery[locale]);
			}
		}
	}
});

test("damaged backup guidance preserves non-space IDs literally but hides manifest diagnostics", () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const id of ["fixture", "20261009_120000_abc-def", "$&$$$`$'{id}", "日本語"]) {
			const expected = locale === "es"
				? `La copia de seguridad ${id} está dañada y no se puede restaurar. Figura en Ajustes → Datos.`
				: `Backup ${id} is damaged and cannot be restored. It is listed in Settings → Data.`;
			for (const detail of [String.raw`IO error: C:\fixtures\$&\manifest.json`, "opaque".repeat(200)]) {
				const raw = `backup error: backup ${id} has no readable manifest.json: ${detail}`;
				assert.equal(localizeApiError(raw), expected);
				assert.equal(localizeApiError(`500: ${raw}`), expected);
			}
		}
	}
});

test("export database and all SQLite companion-file destinations show the same guidance", () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const target of ["the project database itself", ...["-wal", "-shm", "-journal"].map(suffix =>
			`the project database's SQLite ${suffix} sidecar`)]) {
			const raw = `the export destination is ${target}; choose a different file`;
			assert.equal(localizeApiError(raw), secondBatchErrors[3][locale]);
			assert.equal(localizeApiError(`400: ${raw}`), secondBatchErrors[3][locale]);
		}
		assert.equal(localizeApiError("saved Direct project format is incompatible"), secondBatchErrors[4][locale]);
		assert.equal(localizeApiError("409: saved Direct project format is incompatible"), secondBatchErrors[4][locale]);
	}
});

test("Direct-format exact rules retain error identity and reject added prefixes, suffixes or line breaks", () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const body of [secondBatchErrors[4].body, "saved Direct project format is incompatible"]) {
			for (const frame of ["", "409: "]) {
				const error = new ApiError(`${frame}${body}`);
				assert.equal(error.key, "api.error.directFormatIncompatible");
				assert.equal(error.message, secondBatchErrors[4][locale]);
				// Like every exact rule, surrounding whitespace is trimmed; other additions are not.
				for (const nearMiss of [`extra prefix: ${body}`, `${body}: opaque`, `${body}\nsecond line`]) {
					assert.equal(new ApiError(`${frame}${nearMiss}`).key, undefined);
				}
			}
		}
	}
});

test("downgrade guidance shows only reliably extracted short versions, including prerelease and build tokens", () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const [installed, incoming] of [["2.0.0", "1.0.0"], ["2.1.0-beta.2+build.7", "2.1.0-alpha.1+build.3"]]) {
			const raw = `patch downgrade blocked: installed ${installed}, incoming ${incoming}`;
			const expected = locale === "es"
				? `Hay una versión más reciente del parche instalada (${installed}); la versión seleccionada es ${incoming}. Revierta el parche instalado antes de instalar una versión anterior.`
				: `A newer patch version is installed (${installed}); the selected version is ${incoming}. Roll back the installed patch before installing an older version.`;
			assert.equal(localizeApiError(raw), expected);
			assert.equal(localizeApiError(`409: ${raw}`), expected);
		}
		for (const suffix of [
			"installed unknown, incoming opaque",
			"installed 2.0.0, incoming 1.0.0; opaque extra detail",
			"installed 2.0.0, incoming 1.0.0, incoming 0.9.0",
			"installed , incoming 1.0.0",
			String.raw`installed C:\fixtures\$&, incoming {incoming}`,
			`installed 2.0.0+${"a".repeat(100)}, incoming 1.0.0`,
			`installed 2.0.0, incoming 1.0.0+${"a".repeat(100)}`,
		]) {
			const raw = `patch downgrade blocked: ${suffix}`;
			assert.equal(localizeApiError(raw), downgradeGuidance[locale]);
			assert.equal(localizeApiError(`409: ${raw}`), downgradeGuidance[locale]);
		}
	}
});

test("second-batch extra prefixes, incomplete shapes and multiline diagnostics stay unmatched", () => {
	const nearMisses = [
		...secondBatchErrors.map(({ body }) => `extra prefix: ${body}`),
		// Exact rules trim surrounding whitespace (legacy behavior); only the line-owned families reject it.
		...secondBatchErrors.filter(({ body }) => body !== secondBatchErrors[4].body).map(({ body }) => ` ${body}`),
		...secondBatchPrefixes.flatMap(({ prefix }) => [prefix.trimEnd(), prefix.replace(": ", " - ") + "opaque"]),
		"patch downgrade blocked:installed 2.0.0, incoming 1.0.0",
		"patch downgrade blocked:",
		"backup error: backup fixture has no readable manifest.json",
		"backup error: backup fixture has no readable manifest.json:opaque",
		"backup error: backup fixture with spaces has no readable manifest.json: opaque",
		"backup error: backup  has no readable manifest.json: opaque",
		"backup error: backup fixture has no readable manifest.txt: opaque",
		"backup error: backup fixture has no readable manifest.json: ",
		"the export destination is the project database itself",
		"the export destination is ; choose a different file",
		"the export destination is the project database itself; choose another file",
		`${secondBatchErrors[3].body} extra detail`,
		`${secondBatchErrors[4].body}: extra detail`,
		"saved Direct project format is incompatible: extra detail",
		...secondBatchErrors.flatMap(({ body }) => [
			`${body}\nsecond line`, `${body}\rsecond line`,
			...(body === secondBatchErrors[4].body ? [] : [`${body}\n`, `${body}\r\n`]),
		]),
		"backup error: backup fixture has no readable manifest.json: first\nsecond",
		"the export destination is first\nsecond; choose a different file",
	];
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const raw of nearMisses) {
			assert.equal(localizeApiError(raw), raw.trim(), `${locale} bare: ${JSON.stringify(raw)}`);
			assert.equal(localizeApiError(`400: ${raw}`),
				`${locale === "es" ? "Error del servidor" : "Server error"} 400: ${raw.trim()}`,
				`${locale} HTTP: ${JSON.stringify(raw)}`);
			assert.equal(new ApiError(raw).key, undefined);
			assert.equal(new ApiError(`400: ${raw}`).key, undefined);
		}
	}
});

// Batch 3 keeps the existing English fallback and localizes only owned Spanish guidance.
const batch3Errors: readonly {
	name: string; body: string; es: string; nearMiss: string; prefix?: string;
}[] = [
	{
		name: "Tauri format detection failure",
		body: "unsupported format: format not detected",
		es: "No se pudo detectar el formato del juego. Compruebe la carpeta o el archivo seleccionado, o seleccione el formato manualmente.",
		nearMiss: "unsupported format: format not detected for this file",
	},
	{
		name: "existing pivot output",
		body: String.raw`IO error: refusing to overwrite existing file: C:\private\pivot.locust.db`,
		prefix: "IO error: refusing to overwrite existing file: ",
		es: "Ya existe un archivo en el destino del proyecto intermedio. Elija otro nombre o ubicación para guardar la nueva base de datos.",
		nearMiss: "IO error: refusing to overwrite existing directory: private",
	},
	{
		name: "invalid project database",
		body: String.raw`IO error: not a Locust project database: C:\private\other.locust.db`,
		prefix: "IO error: not a Locust project database: ",
		es: "El archivo seleccionado no es una base de datos de proyecto de Locust válida. Seleccione el archivo .locust.db del proyecto.",
		nearMiss: "IO error: not a Locust project database backup: private",
	},
	{
		name: "corrupt patch manifest",
		body: "patch error: corrupt locust-patch.json: expected value at line 1 column 1",
		prefix: "patch error: corrupt locust-patch.json: ",
		es: "El manifiesto del parche está dañado o no es válido. Vuelva a descargar el parche o solicite una copia válida a su autor.",
		nearMiss: "patch error: corrupt other.json: opaque",
	},
	{
		name: "unsafe patch entry",
		body: "unsafe zip entry: ../private/file.txt",
		prefix: "unsafe zip entry: ",
		es: "El parche contiene una ruta de archivo no segura y se ha rechazado. Solicite un parche válido a su autor.",
		nearMiss: "unsafe zip entries: ../private/file.txt",
	},
	{
		name: "pack backup selection mismatch",
		body: "patch error: selected backup does not match the recorded injection; reopen Pack to use its original backup",
		es: "La copia de seguridad seleccionada no corresponde a la inyección registrada. Cierre y vuelva a abrir Parche → Empaquetar para usar la copia de seguridad original de esa inyección.",
		nearMiss: "patch error: selected backup does not match the recorded injection; reopen Apply to use its original backup",
	},
	{
		name: "incomplete XLIFF document",
		body: "parse error in xliff: XLIFF parse error: empty or incomplete XLIFF document",
		es: "El archivo XLIFF está vacío o incompleto. Exporte un archivo XLIFF 1.2 completo y vuelva a importarlo.",
		nearMiss: "parse error in xliff: XLIFF parse error: empty or complete XLIFF document",
	},
	{
		name: "missing pristine files for packing",
		body: "patch error: pristine hashes required but no --pristine path and no valid .locust/backup found",
		es: "No se encontraron archivos originales válidos para calcular los hashes del parche. Seleccione una copia limpia del juego o una copia de seguridad válida en Parche → Empaquetar.",
		nearMiss: "patch error: pristine hashes required but no --pristine path and no valid .locust/backup found yet",
	},
	{
		name: "backup restore hash mismatch",
		body: "backup error: backup inventory/hash mismatch; restore refused",
		es: "Los archivos de la copia de seguridad no coinciden con su inventario o sus hashes. Se rechazó la restauración. Use otra copia de seguridad o una copia limpia del juego.",
		nearMiss: "backup error: backup inventory/hash mismatch; packing refused",
	},
	{
		name: "unsupported XLIFF version",
		body: "parse error in xliff: XLIFF parse error: only XLIFF 1.2 is supported",
		es: "La versión del archivo XLIFF no es compatible. Exporte el archivo como XLIFF 1.2 y vuelva a importarlo.",
		nearMiss: "parse error in xliff: XLIFF parse error: only XLIFF 2.0 is supported",
	},
	{
		name: "font extension mismatch",
		body: "patch error: source and existing target must have the same .ttf or .otf extension; web fonts, collections and Unity SDF assets cannot be replaced",
		es: "La fuente de reemplazo y la del juego deben tener la misma extensión .ttf u .otf. Seleccione una fuente compatible; no se admiten fuentes web, colecciones ni recursos SDF de Unity.",
		nearMiss: "patch error: source and existing target must have different .ttf or .otf extensions; web fonts, collections and Unity SDF assets cannot be replaced",
	},
	{
		name: "font patch output inside game",
		body: "output_path must be outside the selected game directory",
		es: "El destino del parche de fuentes está dentro de la carpeta del juego. Elija una ubicación fuera de esa carpeta para guardar el ZIP.",
		nearMiss: "output_path must be inside the selected game directory",
	},
];

for (const mapping of batch3Errors) {
	test(`batch 3 ${mapping.name}: Spanish guidance, unchanged English and near-miss fallback`, () => {
		for (const locale of ["es", "en"] as const) {
			setLocale(locale);
			for (const frame of ["", "400: ", "409: ", "500: "]) {
				const fallback = (body: string) => frame
					? `${locale === "es" ? "Error del servidor" : "Server error"} ${frame}${body}`
					: body;
				const raw = frame + mapping.body;
				const expected = locale === "es" ? mapping.es : fallback(mapping.body);
				assert.equal(localizeApiError(raw), expected);
				assert.equal(new ApiError(raw).message, expected);
				assert.equal(localizeApiError(frame + mapping.nearMiss), fallback(mapping.nearMiss));
				assert.equal(new ApiError(frame + mapping.nearMiss).key, undefined);
				if (!mapping.prefix) {
					assert.equal(localizeApiError(`${raw} extra`), fallback(`${mapping.body} extra`));
				}
			}
		}
	});
}

test("batch 3 prefixes hide opaque suffixes only in Spanish and reject every line separator", () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const mapping of batch3Errors) {
			if (!mapping.prefix) continue;
			for (const suffix of [String.raw`C:\private\日本語\$&\{detail}`, "hash; ".repeat(200), "$' $$ $`"]) {
				const body: string = mapping.prefix + suffix;
				assert.equal(localizeApiError(body), locale === "es" ? mapping.es : body.trim());
				assert.equal(localizeApiError(`400: ${body}`), locale === "es" ? mapping.es : `Server error 400: ${body.trim()}`);
			}
		}
		for (const mapping of batch3Errors) {
			for (const line of ["\n", "\r", "\r\n", "\u2028", "\u2029"]) {
				for (const body of [mapping.body + line, line + mapping.body, mapping.body + line + "opaque"]) {
					assert.equal(localizeApiError(body), body.trim());
					assert.equal(localizeApiError(`400: ${body}`),
						`${locale === "es" ? "Error del servidor" : "Server error"} 400: ${body.trim()}`);
				}
			}
		}
	}
});

// Verified Display templates: providers/{openai,claude,deepl,ollama,google,argos}.rs,
// xai_oauth.rs:346, core/error.rs:20 and core/translation.rs:527,1394-1419,1562,2333.
const providerGuidance = {
	credentials: {
		en: (name: string) => `${name} rejected the API key. Check it in Settings → Providers.`,
		es: (name: string) => `${name} rechazó la clave API. Revísela en Ajustes → Proveedores.`,
	},
	rate: {
		en: "The provider is limiting requests. Wait and retry, or lower the batch size in Settings → Translation Defaults.",
		es: "El proveedor está limitando las solicitudes. Espere y vuelva a intentarlo, o reduzca el tamaño del lote en Ajustes → Valores predeterminados de traducción.",
	},
	network: {
		en: "Could not reach the provider. Check your connection. For Ollama, Argos or LM Studio, make sure the local server is running.",
		es: "No se pudo conectar con el proveedor. Compruebe su conexión. Si usa Ollama, Argos o LM Studio, compruebe que el servidor local esté en funcionamiento.",
	},
	malformed: {
		en: "The provider returned an unusable answer. Retry, lower the batch size, or try another model or provider.",
		es: "El proveedor devolvió una respuesta que no se puede utilizar. Vuelva a intentarlo, reduzca el tamaño del lote o pruebe otro modelo o proveedor.",
	},
	budget: {
		en: "The cost limit cannot be enforced because cost information is unavailable. Remove the cost limit or choose a provider with cost estimates.",
		es: "No se puede aplicar el límite de coste porque falta información sobre los costes. Quite el límite de coste o elija un proveedor con estimaciones de coste.",
	},
};
const opaqueProviderDetails = ["", "opaque diagnostic", String.raw`<html>C:\日本語\$& {error} $' $$</html>`, "opaque; ".repeat(200)];
const malformedProviderPrefixes = [
	...["OpenAI", "Claude", "Ollama"].map(name => `provider error: ${name} malformed response: `),
	...["DeepL", "Argos"].map(name => `provider error: ${name} returned malformed response: `),
	"provider error: Google Translate returned invalid JSON: ",
	"provider error: could not parse JSON array from response: ",
];
const connectionProviderPrefixes = [
	...["OpenAI", "Claude", "DeepL", "Ollama", "Argos"].map(name => `provider error: ${name} connection failed: `),
	"provider error: Google Translate request failed: ",
];
const grokExpired = "provider error: xAI session expired — run `locust auth grok` to log in again";
const providerTimeout = "provider error: provider request timeout: retry deadline exceeded";
const costReasons = [
	"preceding calls have unknown cost", "this provider has no valid cost estimate",
	"this provider has no cost estimate", "observed calls have unknown cost",
];
const mismatchedIds = "provider returned mismatched or duplicate entry IDs";

for (const locale of ["en", "es"] as const) {
	const check = (raw: string, expected: string) => {
		assert.equal(localizeApiError(raw), expected, `${locale} bare: ${JSON.stringify(raw)}`);
		assert.equal(localizeApiError(`500: ${raw}`), expected, `${locale} HTTP: ${JSON.stringify(raw)}`);
	};
	test(`provider credentials reject only owned 401/403 forms and hide bodies in ${locale}`, () => {
		setLocale(locale);
		for (const name of ["OpenAI", "Claude", "DeepL"]) {
			for (const status of ["401", "403", "401 Unauthorized", "403 Forbidden"]) {
				for (const suffix of ["", ...opaqueProviderDetails.map(detail => `: ${detail}`)]) {
					check(`provider error: ${name} returned status ${status}${suffix}`, providerGuidance.credentials[locale](name));
				}
			}
		}
	});
	test(`Grok session expiry gives desktop sign-in guidance in ${locale}`, () => {
		setLocale(locale);
		check(grokExpired, locale === "es"
			? "La sesión de Grok caducó. Vuelva a iniciar sesión en Ajustes → Proveedores."
			: "Your Grok session expired. Sign in again in Settings → Providers.");
	});
	test(`provider rate limits hide bodies and suggest smaller batches in ${locale}`, () => {
		setLocale(locale);
		for (const name of ["OpenAI", "Claude", "DeepL", "Ollama", "Google Translate", "Argos"]) {
			for (const status of ["429", "429 Too Many Requests"]) {
				for (const suffix of ["", ...opaqueProviderDetails.map(detail => `: ${detail}`)]) {
					check(`provider error: ${name} returned status ${status}${suffix}`, providerGuidance.rate[locale]);
				}
			}
		}
	});
	test(`provider timeout and connections give local-server guidance in ${locale}`, () => {
		setLocale(locale);
		check(providerTimeout, providerGuidance.network[locale]);
		for (const prefix of connectionProviderPrefixes) {
			for (const detail of opaqueProviderDetails) check(prefix + detail, providerGuidance.network[locale]);
		}
	});
	test(`cost limits preserve amounts and recognize all four enforcement reasons in ${locale}`, () => {
		setLocale(locale);
		for (const [estimated, limit] of [["1.2345", "0.5000"], ["9007199254740993.2500", "0.0000"]]) {
			check(`cost limit exceeded: estimated $${estimated} exceeds limit $${limit}`, locale === "es"
				? `El coste estimado ($${estimated}) supera el límite de coste ($${limit}). Aumente o quite el límite de coste, o elija un proveedor con estimaciones de coste.`
				: `Estimated cost ($${estimated}) exceeds the cost limit ($${limit}). Raise or remove the cost limit, or choose a provider with cost estimates.`);
		}
		for (const reason of costReasons) {
			const expected = reason === "this provider has no cost estimate"
				? (locale === "es"
					? "Este proveedor no puede estimar el coste. Quite el límite de presupuesto para continuar o elija un proveedor con estimaciones de coste."
					: "This provider cannot estimate cost. Remove the budget limit to continue, or choose a provider with cost estimates.")
				: providerGuidance.budget[locale];
			check(`provider error: cannot enforce a cost limit: ${reason}`, expected);
		}
		assert.equal(new ApiError("provider error: cannot enforce a cost limit: this provider has no cost estimate").key, "api.error.unknownProviderCost");
	});
	test(`malformed provider output hides diagnostic bodies in ${locale}`, () => {
		setLocale(locale);
		for (const prefix of malformedProviderPrefixes) {
			for (const detail of opaqueProviderDetails) check(prefix + detail, providerGuidance.malformed[locale]);
		}
		check("provider error: translation count mismatch: sent 12 strings, got 0 back", providerGuidance.malformed[locale]);
		check(mismatchedIds, providerGuidance.malformed[locale]);
	});
}

const providerSamples = [
	"provider error: OpenAI returned status 401", "provider error: Claude returned status 403 Forbidden: opaque",
	"provider error: Argos returned status 429 Too Many Requests", grokExpired, providerTimeout,
	...connectionProviderPrefixes.map(prefix => prefix + "opaque"),
	...malformedProviderPrefixes.map(prefix => prefix + "opaque"),
	...costReasons.map(reason => `provider error: cannot enforce a cost limit: ${reason}`),
	"cost limit exceeded: estimated $1.2345 exceeds limit $0.5000",
	"provider error: translation count mismatch: sent 12 strings, got 0 back", mismatchedIds,
];
test("provider near misses and unknown diagnostics retain fallback in both locales", () => {
	const nearMisses = [
		...providerSamples.map(raw => `extra prefix: ${raw}`),
		...providerSamples.map(raw => ` ${raw}`),
		"provider error: Other returned status 401", "provider error: openai returned status 401",
		"provider error: Ollama returned status 401", "provider error: Google Translate returned status 403",
		"provider error: OpenAI returned status 4010", "provider error: Claude returned status 4030: opaque",
		"provider error: OpenAI returned status 401 Forbidden", "provider error: Claude returned status 401 Unauthorized extra",
		"provider error: OpenAI returned status 4290", "provider error: Other returned status 429",
		"provider error: OpenAI returned status 500: opaque", "provider error: OpenAI health check returned status 401",
		"provider error: OpenAI returned status 401:opaque", "provider error: OpenAI returned status 429 Nope",
		grokExpired + " extra", grokExpired.replace("—", "-"), providerTimeout + ": opaque",
		"provider error: Other connection failed: opaque", "provider error: OpenAI connection failed:opaque",
		"provider error: Google Translate request failed", "provider error: Other malformed response: opaque",
		"provider error: OpenAI malformed response:opaque", "provider error: Google returned invalid JSON: opaque",
		"provider error: cannot enforce a cost limit: unknown reason",
		...costReasons.map(reason => `provider error: cannot enforce a cost limit: ${reason} extra`),
		"cost limit exceeded: estimated $NaN exceeds limit $1.0000", "cost limit exceeded: estimated $-1.0000 exceeds limit $1.0000",
		"cost limit exceeded: estimated $1.0000 exceeds limit $0.0000 extra",
		"provider error: translation count mismatch: sent -1 strings, got 2 back",
		"provider error: translation count mismatch: sent N strings, got M back",
		"provider error: translation count mismatch: sent 1 strings, got 2 back extra", mismatchedIds + " extra",
		"provider error: something new", "unrelated diagnostic",
	];
	for (const locale of ["en", "es"] as const) {
		setLocale(locale);
		for (const raw of nearMisses) {
			assert.equal(localizeApiError(raw), raw.trim(), JSON.stringify(raw));
			assert.equal(localizeApiError(`500: ${raw}`), `${locale === "es" ? "Error del servidor" : "Server error"} 500: ${raw.trim()}`, JSON.stringify(raw));
		}
	}
});
test("provider rules reject embedded and final line breaks before legacy trimming", () => {
	for (const locale of ["en", "es"] as const) {
		setLocale(locale);
		for (const sample of providerSamples) {
			for (const newline of ["\n", "\r", "\r\n", "\u2028", "\u2029"]) {
				for (const raw of [sample + newline, sample + newline + "opaque", newline + sample]) {
					assert.equal(localizeApiError(raw), raw.trim(), JSON.stringify(raw));
					assert.equal(localizeApiError(`500: ${raw}`), `${locale === "es" ? "Error del servidor" : "Server error"} 500: ${raw.trim()}`, JSON.stringify(raw));
				}
			}
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
let httpFailureStatus = 400;
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
	globalThis.fetch = async () => new Response(rawFailure, { status: httpFailureStatus });
	api = await import("./api");
});
after(() => {
	globalThis.fetch = originalFetch;
	if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
	else Reflect.deleteProperty(globalThis, "window");
	useLogStore.getState().clear();
	setLocale("en");
});

test("YU-RIS encoding refusal through public Tauri inject rejects once without new logs or parameter mutation", async () => {
	for (const locale of ["en", "es"] as const) {
		setLocale(locale);
		for (const asError of [false, true]) {
			rejectWithError = asError;
			rawFailure = yurisEncodingRefusal;
			const params = { project_path: "synthetic-game", format_id: "yuris", languages: ["es"], mode: "replace" as const, output_dir: "synthetic-output" };
			const beforeParams = structuredClone(params);
			const commandsBefore = tauriCommands.length;
			useLogStore.getState().clear();
			await assert.rejects(api.inject(params), (error: unknown) => {
				assert.ok(error instanceof ApiError);
				assert.equal(error.message, locale === "es" ? yurisEncodingGuidance.es : rawFailure);
				return true;
			});
			assert.deepEqual(tauriCommands.slice(commandsBefore), ["run_inject"]);
			assert.deepEqual(params, beforeParams);
			assert.equal(useLogStore.getState().entries.length, 0, "preserve existing non-PO Tauri logging behavior");
		}
	}
});

test("YU-RIS encoding HTTP guidance preserves raw 500 activity detail and unmatched multiline fallback", async () => {
	// patchPack shares request() with HTTP inject, even when this test process uses Tauri.
	httpFailureStatus = 500;
	try {
		for (const locale of ["en", "es"] as const) {
			setLocale(locale);
			for (const body of [yurisEncodingRefusal, yurisEncodingRefusal + "\nopaque   "]) {
				rawFailure = body;
				useLogStore.getState().clear();
				await assert.rejects(api.patchPack({ game_path: "synthetic-game", output_path: "synthetic.zip" }), {
					message: locale === "es" && body === yurisEncodingRefusal
						? yurisEncodingGuidance.es
						: `${locale === "es" ? "Error del servidor" : "Server error"} 500: ${body.trim()}`,
				});
				const entries = useLogStore.getState().entries;
				assert.equal(entries.length, 1);
				assert.deepEqual([entries[0].level, entries[0].message, entries[0].detail, entries[0].source],
					["error", "API 500: /patch/pack", body, "api"]);
			}
		}
	} finally { httpFailureStatus = 400; }
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

test("Tauri second-batch string/Error rejections keep existing logging behavior", async () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const asError of [false, true]) {
			rejectWithError = asError;
			for (const recovery of secondBatchErrors) {
				rawFailure = recovery.body;
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

test("HTTP second-batch guidance preserves raw diagnostic activity-log details without duplicate logs", async () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const recovery of secondBatchErrors) {
			rawFailure = recovery.body;
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
		// Opaque long/multiline bodies must still reach the log verbatim, even when unmatched.
		for (const raw of [
			`patch verification failed: ${"opaque; ".repeat(200)}`,
			`patch verification failed: first\nsecond\r\n  `,
		]) {
			rawFailure = raw;
			useLogStore.getState().clear();
			await assert.rejects(api.patchPack({ game_path: "fixture", output_path: "fixture.zip" }), ApiError);
			assert.equal(useLogStore.getState().entries.length, 1);
			assert.equal(useLogStore.getState().entries[0].detail, raw);
		}
	}
});

test("HTTP batch 3 guidance preserves raw activity-log detail and English fallback", async () => {
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const mapping of batch3Errors) {
			rawFailure = mapping.body;
			useLogStore.getState().clear();
			await assert.rejects(api.patchPack({ game_path: "fixture", output_path: "fixture.zip" }), {
				message: locale === "es" ? mapping.es : `Server error 400: ${mapping.body}`,
			});
			const entries = useLogStore.getState().entries;
			assert.equal(entries.length, 1);
			assert.deepEqual([entries[0].level, entries[0].message, entries[0].detail, entries[0].source],
				["error", "API 400: /patch/pack", mapping.body, "api"]);
		}
	}
});

test("batch 3 leaves unstable sentences and unowned diagnostics unchanged", () => {
	const bodies = [
		"game busy: another patch operation is using C:\\private; wait for it to finish and retry",
		"patch error: game busy: another patch operation is using C:\\private; wait for it to finish and retry",
		"incomplete pivot: entry 'private-id' has a missing or blank translation",
		"stale translation for entry 'private-id': source changed after translation; translate again or explicitly mark Reviewed/Approved after checking the current source",
		"backup error: backup private-id source no longer exists or has wrong type (C:\\private)",
		"backup private-id is needed to pack the injection recorded for es; inject again or open another project before deleting it",
		"patch error: replacement lacks 3 required character(s): ['a', 'b', 'c']",
		"parse error in C:\\private: invalid language code: private",
		"database error: database is locked",
		"IO error: Access is denied. (os error 5)",
	];
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		for (const body of bodies) {
			assert.equal(localizeApiError(body), body);
			assert.equal(localizeApiError(`400: ${body}`),
				`${locale === "es" ? "Error del servidor" : "Server error"} 400: ${body}`);
			assert.equal(new ApiError(body).key, undefined);
		}
	}
});

test("detection failure recognition is unchanged in both locales", async () => {
	const { isDetectionFailure } = await import("./openProjectFlow");
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		assert.ok(isDetectionFailure(localizeApiError("format not detected")));
		assert.ok(isDetectionFailure(localizeApiError("422: Could not detect game format")));
		assert.ok(isDetectionFailure(localizeApiError("unsupported format: format not detected")));
		assert.equal(isDetectionFailure(localizeApiError(poError("27", "quote"))), false);
	}
});
