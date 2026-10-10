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

test("detection failure recognition is unchanged in both locales", async () => {
	const { isDetectionFailure } = await import("./openProjectFlow");
	for (const locale of ["es", "en"] as const) {
		setLocale(locale);
		assert.ok(isDetectionFailure(localizeApiError("format not detected")));
		assert.ok(isDetectionFailure(localizeApiError("422: Could not detect game format")));
		assert.equal(isDetectionFailure(localizeApiError(poError("27", "quote"))), false);
	}
});
