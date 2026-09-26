# Mejora continua hasta las 14:00 — 19 de septiembre de 2026

Goal activo por solicitud explícita del usuario, con plazo **20:00 UTC / 14:00 America/Mexico_City**, o agotamiento de la cuota disponible. Sin compras, resets ni publicación. Estado en `tmp/goal-20260919/STATE.json`. La tanda anterior está cerrada con evidencia en `tmp/goal-20260917/RESULT.json`.

## Implementado y comprobado

- Recuperación Direct/Add desde Inicio, incluso cuando una inserción pendiente impide abrir el proyecto. Diagnóstico, restauración y copias de conflictos con consentimiento específico. El backend verifica bajo bloqueo el ID de la operación revisada y rechaza una operación sustituida. La confirmación se descarta al cambiar de ruta, cerrar o fallar.
- Reproducido y corregido un fallo de borradores: abrir desde la interfaz y recargar perdía la identidad de la base porque `/project/current` no devolvía `database_path`. El servidor conserva ahora esa identidad y los modos del formato, también al abrir bases pivotadas desde Tauri.
- HTTP y Tauri guardan los proyectos recientes mediante el mismo helper. Un fallo al escribir la lista se muestra sin presentar como fallida una apertura que sí ocurrió. Cada estado de pruebas tiene su propia ruta de configuración.
- Ajustes compartidos: escritura de una generación completa antes de reemplazar el archivo; fallo de persistencia conserva la configuración activa y los bytes anteriores. Las respuestas ocultan las claves y el marcador `***` no sustituye una clave existente al cambiar de modelo. Los cambios de proveedores y apariencia envían solo los campos editados; se procesan en orden y muestran errores de guardado.

## Evidencia de esta tanda

- `recovery-tests.log`: 45 pruebas focales pasan, una ignorada (helper); frontend 31 archivos y build pasan.
- `persistence-tests.log`: **608 pruebas pasan, 2 ignoradas, 27 suites** de core/server/escritorio. Incluye publicación de configuración, lectura concurrente, claves ocultas, fallos de escritura y apertura de bases pivotadas.
- `draft-reload-baseline.log`: reproduce el borrador vacío al recargar antes de la corrección.
- `recovery-ui-1789839557085/result.json`: **7 recorridos** sobre frontend de producción y backend real pasan: recuperación limpia/interrumpida/con conflictos/ID obsoleto, borrador tras apertura real, recientes persistidos y fallo/reintento de ajustes. Capturas y limpieza de procesos en el mismo directorio.
- `unity-release-1789839466170846100/RESULT.json`: CLI release congelado de la tanda anterior, SHA `9d54ba1adcee89231bfd1ce3fa89f1ca438b2d9db7ad9b28c40bb96a42be4bb7`. Recurso Unity real de 261 MB en copia independiente: extracción 27,507 s, inserción de una cadena 12,281 s, reextracción 27,041 s. Se conservan las 4.374 identidades y solo cambia la cadena elegida. DLL y entrada prístina intactas. Cero API. Es un cambio ASCII de igual longitud; no certifica arranque del juego, fuentes o calidad lingüística.

## Importación y ajustes: siguiente checkpoint

- Corregido el importador XLIFF 1.2: admite namespaces por defecto/prefijados, entidades, CDATA y conserva espacios. Rechaza documentos incompletos, IDs duplicados y marcado inline no compatible antes de tocar la base. Es un importador de texto; no transforma `<ph>`/`<mrk>`.
- Reproducida la sobrescritura de una traducción vigente con un PO cuyo texto de origen era antiguo. PO/XLIFF ahora comparan el origen dentro de la transacción; omiten filas obsoletas/desconocidas e informan los conteos en CLI, HTTP y Tauri. IDs duplicados abortan todo el lote.
- `import-tests.log`: 673 pruebas pasan, 2 ignoradas, 34 suites. `import-http-tests.log`: integridad HTTP pasa. `stale-import-fixed.log`: 0 importadas, 1 origen obsoleto; traducción vigente intacta.
- `recovery-ui-1789840781725/result.json`: 11 recorridos con backend real pasan. Incluyen PO obsoleto, XLIFF prefijado/CDATA, escritura al confirmar campos, validación de vacíos, slider con una escritura y respuestas lentas A→B→A sin perder la última intención. Capturas en ese directorio.

## En curso

El usuario pidió levantar la app y revisar capturas. Compilación nativa en `visual-desktop-build.log`. Las capturas detectaron nombres de motor truncados por las insignias y campos de idiomas desalineados; corrección visual en curso. También se identificó que la carga con fallback de una configuración inválida podría sobrescribirla después: pendiente de proteger y validar. Falta validación integrada/final de los binarios nuevos y replay completo.

Las capturas anteriores a esta revisión nativa proceden de navegador headless con backend real. No se modificaron juegos originales ni se publicaron cambios. Cero llamadas API en esta tanda.

## Revisión visual y protección de borradores (18:25 UTC aprox.)

- App nativa abierta como `tmp/goal-20260919/native-visual/locust-visual-20260919.exe`, perfil aislado y cinco cadenas japonesas de fixture HTML. Capturas nativas de Inicio, Proveedores, Apariencia y Editor. El candidato permite reproducir el tema oscuro sin efecto; está pendiente sustituirlo por el siguiente release corregido. Helper de pantalla intermitente: una captura mostró oclusión de otra ventana y algunos IDs caducaron; se recuperó selección sin actuar sobre la ventana ajena. Las capturas de layout detalladas son headless con backend real.
- Corregidos nombres de motores cortados, alineación de campos, dark variant de Tailwind, fondo/texto base del tema, contraste de ayudas, botones del editor/filtros adaptables y estados localizados. La guía de flujo también se ajustó tras detectar superposición a 900px/18px.
- `recovery-ui-1789841425849`: 12 capturas a 1280/900, claro/oscuro, 14/18px; colores reales verificados y sin desbordamiento horizontal de documento.
- Configuración inicial inválida: fallback protegido, aviso visible en todas las pantallas, escritura bloqueada hasta reparar/reiniciar. `config-protection-tests.log`: prueba core y cuatro de persistencia servidor pasan. `recovery-ui-1789842210593`: UI confirma bytes inválidos intactos al abrir proyecto y al intentar cambiar ajustes; credencial fixture no aparece en respuesta.
- `recovery-ui-1789841992228`: antes de corregir edición inline, un error de guardado perdía texto y causaba una excepción no manejada. `recovery-ui-1789842236753`: cinco recorridos pasan después: borrador visible, recarga, reintento exacto, Escape sin escritura y todos los botones visibles en 900px. La tabla comparte ahora persistencia/identidad de borrador con el panel lateral.
- `frontend-unit.log`: 31 archivos de pruebas pasan. Suite Rust integrada en curso mediante `integrated-validation.ps1`; artefactos finales/replay pendientes. No se llamó API de traducción.

## Checkpoint integrado y nuevo fallo de empaquetado (18:43 UTC)

- Suite integrada: **1.353 pasan, 16 ignoradas, 57 suites**; fmt/Clippy correctos. `final-buffered-ui.log`, `final-inline-ui.log`, `final-config-ui.log` suman 17 recorridos, más 12 capturas de layout; sin excepciones de página. Release Locust compilado y abierto (PID 29392, `native-visual/updated-artifact.json`); captura `native-visual/updated-home.jpg` muestra tema oscuro nativo funcionando.
- `final-replay/REPORT.md`: CLI `f36a421219a70e42252010e05c8b750fb344d37d16a13acf5eddacf329589903`, 25/25 casos y 255/255 filas, entradas congeladas intactas, API cero.
- `unity-release-1789842853441554200/RESULT.json`: nuevo release, XLIFF prefijado; 4.374 identidades, una fila cambiada; 27,112s extracción, 11,224s inserción, 27,296s reextracción. Mismos límites ASCII de igual longitud y sin arranque del juego.
- Rule95: 14 pruebas pasan y build release correcto; harness de artefactos aún pendiente.
- El recorrido GUI completo llegó a insertar correctamente, pero empaquetar con hashes prístinos falló: la copia de BackupManager creada por Direct no es la copia `.locust/backup` que buscaba pack. El selector y los bytes originales están intactos; no se presenta como éxito. Se está conectando el ID exacto del backup de esa inyección a pack, verificando origen/inventario/hashes y rechazando copias ajenas o dañadas. Para proyectos abiertos como un archivo se normaliza su árbol al nombre original y el registro al directorio contenedor.
- Esta corrección Round 2 modifica fuentes después del checkpoint integrado. La lista anterior se conserva en `checkpoint-1830-source-sha256.json`; falta regenerar el manifiesto y rebuild final tras validar la conexión de backups.


## Inyección → ZIP → aplicación → reversión (18:54 UTC)

- Corregida la asociación al respaldo exacto de Direct. `with_pristine_tree` verifica origen/tipo/inventario y prepara un árbol temporal con el nombre original cuando la selección era un archivo individual. No restaura el juego para empaquetar. UI transmite el ID solo para ese juego, activa hashes prístinos y permite elegir otra copia explícita.
- Dos pruebas core y una prueba HTTP que cubre carpeta/archivo pasan. Un respaldo alterado devuelve 400 y conserva un ZIP existente.
- `recovery-ui-1789843773598/result.json`: cuatro etapas completas pasan con backend real y WebSocket real. Edición/inyección, ZIP estricto, verificación sin escritura, aplicación y reversión exacta. Capturas `ui-injected.png`, `ui-packed.png`, `ui-applied.png`, `ui-reverted.png`. Sin errores de página ni API de traducción.
- `originals-check.json`: los 9 originales y sus 9 copias prístinas conservan los hashes del manifiesto y los metadatos observados durante la lectura.
- Se están repitiendo checks integrados antes del siguiente release. El candidato nativo abierto todavía corresponde al checkpoint anterior.


## Checkpoint verificado de escritorio y parches (19:13 UTC)

- Round 2: **1.356 Rust pasan, 16 ignoradas, 58 suites**, fmt y Clippy correctos. 31 archivos frontend pasan; tras la corrección visual final se repitieron `injectOutcome` e i18n y el build. La ausencia de un `tsx.cmd` local hizo fallar una invocación auxiliar; el chequeo se ejecutó correctamente con `npx --offline tsx`.
- CLI release SHA `2940f7eee284cec78c8dbe155cdee87cef73e9bda1c7b8640da21da116278074`. `round2-replay/REPORT.md`: 25/25 casos, 255/255 filas, cinco motores por cinco idiomas, inputs congelados intactos, cero API.
- Rule95 release `apply_once.exe` SHA `b2e265c3eee80ff28c61cefb20136638726d11b551848763e6b5ab4354f20507`. `rule95-final/artifact-qa`: 25/25 casos; `qa-exit-codes.json` conserva los 38 hashes de fixtures y salida de cada proceso. Build release correcto.
- `recovery-ui-1789844242591/engine-ui.json`: apertura por GUI con detección automática de Ren'Py/RPG Maker/Unity/Unreal/HTML, cadenas japonesas, modos permitidos, identidad de DB al recargar y bytes de fixtures intactos; diez capturas. Fue una revisión de extracción/controles, no arranque de los juegos.
- `recovery-ui-1789844454961/pivot-workflow.json`: GUI JA → EN → ES/FR/zh-CN con `{name}`, bases independientes abiertas/recargadas, procedencia japonesa conservada y copia Reemplazar exacta. Textos de prueba manuales, cero API; no es una nueva evaluación lingüística de Grok.
- `native-visual/native-pack-audit.json`: en la aplicación nativa se editó `はい` → `Sí`, guardó por Tauri, insertó Direct por Tauri y empaquetó por el backend embebido compartido. ZIP estricto con hash original de la copia exacta y bytes traducidos verificados. Candidato abierto PID 4720, SHA `c30d6b3688e6fc096b6f2d016e831ecf0a668802a7d6a7d52e99912076962aa1`. Capturas nativas observadas; algunas tienen una notificación ajena superpuesta, por lo que la evidencia visual limpia adicional procede de los recorridos headless.
- La revisión nativa detectó que el diagnóstico de conservación de originales se duplicaba en Advertencias. Se deduplican avisos repetidos globales/por idioma y se presenta ese diagnóstico exacto como información de recuperación. Otros mensajes y fallos siguen como advertencias; pruebas específicas y recorrido GUI pasan. Esta última corrección de frontend está recompilando el escritorio; aún no está en el candidato PID 4720.

## Unity real con Unicode y crecimiento estructural

`unity-unicode-1789844608991219900/VERIFIED-ROUNDTRIP.json` verifica el CLI final sobre copia independiente del recurso real de 261 MB: una frase MonoBehaviour traducida al español dentro de su capacidad UTF-8 y un campo de prueba con acentos/japonés/chino añadido a un JSON TextAsset. Extracción 30,202s, inserción 15,561s, reextracción 20,995s. Se conservan 4.374 entradas semánticas y solo cambian las dos seleccionadas. DLL, original y copia de referencia intactos.

La primera comprobación (`RESULT.json`, conservada) falló porque exigía IDs por posición invariantes y texto sin relleno. El crecimiento reubica tres entradas heurísticas 48 bytes; la escritura en slot rellena la frase con 9 espacios. La verificación posterior compara todas las fuentes semánticas, los desplazamientos explícitos y el relleno exacto. **No se afirma estabilidad de todos los IDs tras crecimiento estructural**, arranque del juego ni disponibilidad de glifos. El campo JSON es una prueba de estructura, no contenido traducible del juego.


## Round 3 en curso: controles de Unity (19:21 UTC)

La vista nativa del recurso real cargó 4.374 cadenas en 28,307s mediante el backend embebido. Mostró nombres de controles (`left ctrl`, `joystick button 2`) y ejes de InputManager entre los candidatos. El traductor no los excluye después. Se está corrigiendo el filtrado por clase serializada, sin blacklist de palabras de botones: InputManager es clase 13 según la [referencia oficial de Unity](https://docs.unity3d.com/2021.3/Documentation/Manual/ClassIDReference.html); sus nombres de teclas/ejes son configuración según [Input Manager](https://docs.unity3d.com/2021.3/Documentation/Manual/class-InputManager.html).

La protección también debe cubrir proyectos antiguos que ya guardaron esos candidatos: rechazar escrituras heurísticas dentro de objetos técnicos reconocidos, con motivo visible, preservando sus bytes. Dos regresiones se están ejecutando contra el comportamiento anterior. No se cambia el contrato de GameObject ni se elimina globalmente la extracción heurística.

Los binarios y la evidencia Round 2 siguen siendo un checkpoint válido, pero no incluyen esta corrección todavía. La compilación del último ajuste visual terminó; se pospone sustituir el candidato abierto hasta integrar Round 3. El manifiesto de fuentes necesita regenerarse al cerrar los nuevos cambios.

## Ronda final: publicación segura del ZIP y configuración Unity

Se reprodujo que `pack` podía reemplazar `game/script.rpy` con un ZIP si se elegía ese destino. La regresión fallida se conserva en `pack-output-baseline.log`. El núcleo ahora valida el destino antes de crear carpetas/temporales: fuera del juego y copia prístina, sin colisión con la base real ni sus sidecars; HTTP protege también el almacén completo de respaldos. `pack-output-fixed.log` y `pack-output-http.log` pasan. La cobertura final añade la DB real aunque `PackOptions.project` tenga una etiqueta distinta.

El primer smoke InputManager detectó 11 bajas y una entrada nueva, no las 8 bajas previstas. Se conserva `unity-input-1789846560737835700/RESULT.json` como fallo de diagnóstico. Un lector de solo lectura del contenedor (`unity-object-audit.rs`, `.log`) atribuyó los offsets a InputManager 13, TagManager 78 y ShaderNameRegistry 94. Se amplió la exclusión de extracción y escritura histórica a esas tres clases, con casos para offsets explícitos y búsqueda heredada; se mantiene TextAsset válido. Identificadores verificados con la [referencia oficial Unity](https://docs.unity3d.com/2021.3/Documentation/Manual/ClassIDReference.html). El nuevo smoke `unity-runtime-real.py` exige las 11 bajas exactas, cero nuevas, cero otras fuentes cambiadas y referencias intactas. Pendiente su ejecución sobre el binario final.

Ronda 4 integrada y compilaciones Locust/Rule95 iniciadas. Los manifiestos `round4-source-sha256.json` y `final-source-sha256.json` identifican el código candidato; deben contrastarse al cierre.

## Candidato final verificado

- Ronda 4: 1.360 pruebas Rust aprobadas, 16 ignoradas, 58 suites; fmt y Clippy sin incidencias. Compilación release Locust 4m03s y Rule95 2m24s; 14 pruebas Rule95 y 25/25 artefactos.
- CLI final `a45e40b1b6c97410268dddf96e2b2c762802f92bf20aaea1a146fee4bf0deed8`; escritorio `4d96ffab2083e01d6d8a5683bf50af02c0b76368e8f760cb7db5b6451650ec1f`; Rule95 `c82e7e1ada8ae0ce97846e04a2b01dc87de88ef110fe5b37a30b6204fdb6ab1c`.
- `round4-replay/REPORT.md`: 25/25 casos, 255/255 filas, cero API, fuentes intactas. `rule95-round4/qa-exit-codes.json`: 38 entradas intactas.
- Unity final: `unity-input-1789847316417327700/RESULT.json`, 21,174s de extracción, 4.374→4.363 con exactamente 11 bajas técnicas. La base histórica protegida queda en `unity-old-database-1789847564444791200/VERIFIED.json`: 11 omisiones, 0 escrituras, bytes originales iguales.
- GUI final `recovery-ui-1789847367794`: cinco comprobaciones completas incluyendo destino destructivo rechazado y reintento válido. Se conservó el primer fallo del selector de pruebas, que encontraba el mismo mensaje tanto en modal como toast; se limitó al modal.
- App final abierta en perfil privado: PID34884, puerto57207; `native-visual/final-artifact.json`. Inicio y editor Unity nativos capturados sin notificación ajena superpuesta; `final-native-unity-open.json` confirma 4.363 entradas.
- `final-originals-check.json`: 18/18 originales/copia prístina iguales. `final-source-audit.json`: 261 fuentes sin cambios. `final-diff-check.log`: limpio respetando CRLF de Windows.

No llamar completada toda compatibilidad de juegos: ver límites en `QA-2026-09-19.md`. Próxima tarea sustantiva: arranque de copias de juegos, validación de glifos/TMP/RTL y semántica de filas heurísticas; esta tanda no produjo nuevas traducciones con proveedor. No hubo publicación ni compra/canje de cuota.

### Cierre definitivo — 14:00 México

Goal completado al terminar la ventana solicitada. Todas las correcciones abordadas están validadas; la aplicación nativa sigue abierta y su backend conserva la misma base sin advertencias de persistencia. `tmp/goal-20260919/RESULT.json` y `STATE.json` contienen el cierre. Las comprobaciones futuras de compatibilidad se describen como límites, no como trabajo ejecutándose.
