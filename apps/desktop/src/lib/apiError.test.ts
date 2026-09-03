import assert from "node:assert/strict";
import {
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

// Detection heuristic still matches the localized detect failure (Welcome picker)
import { isDetectionFailure } from "./openProjectFlow";
setLocale("es");
assert.ok(
	isDetectionFailure(localizeApiError("format not detected")),
	"Spanish detect failure must still trip isDetectionFailure",
);

console.log("apiError.test.ts: ok");
