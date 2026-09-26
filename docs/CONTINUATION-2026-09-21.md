# Locust — continuación del 21 de septiembre de 2026

Goal activo por petición del usuario: mejoras útiles de Locust/Rule95 hasta el límite de cuota, sin hora final ni presupuesto explícito. Solo procesos ocultos y capturas headless. No cambios de foco, ratón, teclado ni ventanas visibles; no compras, resets o publicación.

## Cambios y evidencia

- Una inserción Direct sin cambios creaba un respaldo del juego ya traducido y lo entregaba al empaquetador como original. Reproducción fuerte: `tmp/goal-20260921/noop-backup-baseline-strengthened.log` (hash original incorrecto y verificación Mismatch). El primer ensayo comprobaba únicamente `conflicts` y no detectaba el fallo; no se usa como prueba de corrección.
- El registro SQLite guarda ahora el ID y origen del respaldo en la misma transacción que los hashes de los archivos insertados. Los registros antiguos se migran sin inventar respaldo; las inserciones sin cambios preservan el anterior. El empaquetado verifica el inventario del respaldo exacto y la generación del registro bajo bloqueo. Un respaldo ausente, alterado o incorrecto provoca rechazo; no se sustituye por el más reciente.
- Se quitó la eliminación global automática de todos salvo tres respaldos desde Direct: podía borrar originales todavía referenciados por otros proyectos. Esto puede aumentar el uso de disco; la gestión manual sigue disponible.
- La interfaz usa el ID original asociado y muestra la asociación al reabrir el diálogo. El diálogo Inyectar limpia resultados terminados al cerrarse; mantiene las operaciones en curso. El botón de cerrar Parche tiene nombre accesible.
- La prueba en frío detectó que abrir un proyecto reciente reextraía el juego ya traducido y reiniciaba las traducciones por cambio de origen. Las aperturas nuevas ahora guardan la ruta de su DB en recientes (HTTP y Tauri), de modo que reabrirlas conserva origen/traducción. Abrir una carpeta explícitamente sigue extrayendo. Los recientes antiguos sin ruta de DB requieren abrir su `.locust.db` para evitar reextracción.
- Capturas y prueba real con backend aislado: `tmp/goal-20260921/backup-restart-ui-1790025439909/`. Se editó una traducción, insertó, empaquetó, verificó Clean, aplicó y revirtió comprobando bytes. Después se repitió la inserción sin cambios, se reiniciaron servidor y navegador, se abrió el reciente y se empaquetó con respaldo recuperado sin ID ni carpeta aportados por el cliente. Captura `ui-cold-reopened-pack.png`. Cero llamadas a proveedores.

## Validación y alcance

Primera integración: 1.365 tests Rust correctos, 0 fallos, 16 ignorados, 59 suites (`workspace-tests.log`); Clippy estricto correcto. Frontend: 31 archivos de tests y TypeScript/Vite correctos. Los cambios posteriores de recientes y `--astro` tienen comprobación específica e integración final pendiente: consultar STATE, no atribuirles automáticamente estos totales. Debug CLI construido para la prueba visual; release y Rule95 todavía no se recompilaron en esta continuación.

CLI `--astro`: corregida sobrescritura de archivos del juego, base de datos, notas previas y el propio ZIP. Reproducción previa: `tmp/goal-20260921/astro-baseline.log`; regresión ejecutable posterior: `astro-fixed.log` (cuatro rechazos sin cambios y un destino nuevo válido). La validación ocurre antes de empaquetar; publicación temporal completa con `persist_noclobber` impide truncado también si aparece un destino entre comprobación y escritura. El idioma del documento sale del registro seleccionado, incluyendo selección automática sin `-l`.

Validación posterior de CLI/servidor/escritorio: 184 tests correctos y Clippy/fmt correctos. Rule95: 14 tests correctos. Pruebas originales: 18 hashes iguales al cierre del 19. Compilaciones release y matrices siguen en curso; consultar `STATE.json`.

Limitaciones: respaldo asociado implementado para Direct; CLI `patch` aún no recupera automáticamente el respaldo central registrado y Add/Replace no guardan esa procedencia. Una reinserción que sí cambia archivos crea un parche basado en el estado previo de esa generación, que puede estar ya traducido; no equivale necesariamente a la copia original de distribución. No se certifica arranque de juegos reales, tipografía en runtime, TMP o RTL. No se modificaron juegos del usuario ni credenciales.

## Cierre de tanda 1 (goal continúa)

Release CLI y escritorio construidos. CLI: 25/25 combinaciones, 255 textos, matriz intacta. Rule95: 25/25 artefactos, 38 archivos de fixtures intactos. UI release con reapertura en frío, verificación Clean, aplicación y reversión por bytes; capturas 1280×900 y 1024×768 en `backup-restart-ui-1790026029756`. Fuentes compiladas: 262 hashes sin cambios. Resultado y hashes exactos: `tmp/goal-20260921/BATCH1.json`.

Tanda 2 inicia con reproducción CLI sin `--pristine`: falta original_sha256 aun existiendo el respaldo, tanto con almacén igual como distinto. Evidencia `cli-backup-location-baseline.log`.

## Tanda 2: ubicación del respaldo compartida

Implementado almacén absoluto opcional en procedencia, conservando lectura de registros anteriores. API y CLI usan un resolvedor común que valida origen/inventario/generación y protege el almacén original frente a salidas ZIP o Astro. El CLI recupera el respaldo de Direct automáticamente al omitir `--pristine`; admite también archivo individual relativo. Las pruebas pasan con almacén igual, distinto y ruta relativa (`cli-backup-location-fixed.log`). Core: 5 tests; HTTP: 2 tests incluyendo reinicio con otro almacén. UI real en `backup-restart-ui-1790026589989` comprueba además DB creada en la app y empaquetada con CLI desde otro perfil. Full workspace/Clippy y nueva entrega release pendientes; la entrega de tanda 1 sigue registrada por separado.

La primera regresión completa de tanda 2 falló en tres tests CLI: otra ruta `cmd_inject` (Replace/Add) todavía borraba globalmente respaldos salvo los tres más recientes. Las ejecuciones paralelas eliminaban la copia exacta de otro proyecto antes de empaquetar. Se retiró esa poda y se añadió un test de cinco Reemplazar en otro proyecto seguido de empaquetado Direct anterior, con almacén aislado. Log fallido preservado: `batch2-workspace-tests.log`; comprobación posterior pendiente. Las pruebas por comandos posteriores fijan `LOCUST_BACKUP_ROOT` dentro del directorio de esta sesión.


## Native visual pass (latest user authorization)

User explicitly authorized native screens, launching apps and UI interaction. Launched isolated Locust release (PID 35268) and Rule95 QA release (PID 10260). Native Locust home/recent DB/editor inspected; fixture HTML opens through shared embedded server and recent DB button. Native Rule95 initial layout placed Apply below viewport; implemented two-column desktop layout, localized accessibility names and consistent Spanish wording. Preview checks pass at 900x640,720x520,1280x800 in ES/EN, with Apply visible and no horizontal overflow. Preview uses mocked Tauri calls and is not engine validation; real native improved startup was captured separately. Current Rule95 UI tests 3/3 pass, release rebuilt. Screenshots: tmp/goal-20260921/native-visual/locust-editor.jpg and rule95-improved.jpg; preview evidence: rule95-visual/result.json. Native file-dialog automation is intermittent (stale accessibility cache); do not confuse it with an app failure. BATCH2.json records the independently verified release engine/HTTP/CLI tests.

### Cursor-only checkpoint: legacy recents review correction

- Initial Cursor writer completed exit 0. Report: `tmp/goal-20260921/cursor-recents/REPORT.md`. Directed tests pass (core project 25 includes the 8 open_recent cases; server 3 prefer_saved + 1 persistence; Tauri 1; frontend flow args). No current full release/UI claim.
- Exactly the eight intended source files changed relative to batch2 source inventory; dependency manifests and locks unchanged.
- Independent Cursor review (`cursor-recents-review/REVIEW.md`) found metadata errors suppressed by is_file(), plus unused helper cleanup. SAME writer chat f5301f42-6719-4707-8e2a-85cf560763c4 resumed as unified exec25163. Route reverified in followup.jsonl: Grok 4.6 High / login. No Grok Build/direct xAI.
- Wait SAME exec25163 until terminal, inspect followup report/diff, then run coordinator `tmp/goal-20260921/cursor-recents/verify.ps1` in no-profile PowerShell. This runs scoped Rust tests, fmt/strictClippy/debug CLI/frontend build and real legacy-recent headless restart + exact-hash pack/apply/rollback/CLI bridge. Never run while writer still owns source.
- New process regression `verify-pristine-io.py --binary target/release/locust.exe --expect baseline` passed as a bug reproduction: locked original silently accepted; prior private ZIP replaced; original hash unchanged. Evidence `pristine-io-existing-output-baseline.log`. Future fixed assertion uses --expect fixed and fresh debug binary. Preserve baseline logs.
- Next bounded Cursor order queued at `cursor-pack-io-ORDER-QUEUED.txt`, then Replace provenance order at `cursor-replace/ORDER-QUEUED.txt`. Neither dispatched. Goal remains active.

### Tanda 3 validada (debug + HTTP real)

- Cursor-only implementation and two review/integration corrections finished. 27 core +3 HTTP +1 Tauri directed tests passed; frontend args unit, fmt, strict Clippy on core/server/desktop, debug CLI and frontend production build passed. Initial compile/assertion/fmt failures preserved; final result BATCH3.json.
- Real existing harness unchanged: LOCUST_QA_LEGACY_RECENT=1 and LOCUST_QA_CLI_BRIDGE=1 passed. Run backup-restart-ui-1790033529045: inline translation, Direct injection, protected outputs, app->CLI other-profile/store original hashes, exact apply/rollback, no-op, cold backend+browser reopen of legacy recent without re-extraction, strict pack verified Clean. Screenshot ui-cold-pack-1024.png visually inspected.
- Release executable and open native private app remain Batch2. Defer release rebuild until next correctness fixes; do not claim installed binary already includes this source.
- Replace design review finished: cursor-replace-design/REVIEW.md and FOLLOWUP-REVIEW.md, corrected by ADJUDICATION.md. Initial no-dest-writer claim was wrong: Direct/apply on copied destination can cooperate on a different lock. Acquire destination before copy bytes, hold through record; validate with tests before claiming runtime concurrency proof. Source/backup provenance fix stays queued after pristine read-I/O correction.
- Next writer cursor-pack-io owns only core patch/pack.rs and its tests, exact before/BASE saved. No shell/tests from writer; coordinator verifies. Native UI and data originals preserved. Goal active.

### Tanda 4 validada y nueva reproducción de revisión de traducciones

BATCH4.json: Cursor corrigió pack.rs (solo NotFound permite hash original ausente; errores de lectura devuelven PatchError con ruta). Coordinador solo aplicó rustfmt a ese archivo.11 pruebas de pack, fmt, Clippy estricto core/all-targets y debugCLI pasan. CLI real con archivo original bloqueado: exit1, ZIP anterior/original intactos, sin temporales. HTTP real:400 con ruta, mismas garantías. HTTP harness inicialmente asumió JSON; ApiError es texto, comprobador corregido y endurecido con mensaje/ruta sin relajar estatus/integridad. Fallos previos guardados. COORDINATOR-REVIEW.md y BATCH4-SOURCE/AUDIT guardados. Binario5747d259c4dbba08e8ffe7a5395d55e90318da14d7469e9ff3aeaa6cf638e680. Release sigue tanda2.

Nuevo repro `repro-direct-revision.py` con debug de tanda3 copiado en su ejecución UI: primera inserción HTML A correcta; cambiar traducción DB a B y volver a Direct devuelve0, files_written0/source_changed1 y mantiene A. Segundo patch es A, conserva hash original correcto y dry-run permite instalarlo en juego fresco. Evidencia `direct-revision-baseline.log`, run direct-revision-1790034469523665400. El primer repro abortó al detectar B ausente; preservado como repro-direct-revision-first.py y direct-revision-first-failure.json. NO afirmar pérdida de original hash: la segunda escritura nunca ocurrió. No hay translations.json en ZIP, solo payload/manifest/README; no inventar inconsistencia de exportación de filas.

Preocupación por explorar: si una revisión logra escribir solo uno de varios archivos, Database::record_injection_with_backup reemplaza la lista completa; preservar una traducción acumulada puede requerir conservar outputs previos verificados. Esto aún es inferencia, no reproducción. Revisor Cursor read-only exec73102/chat6fe8e58f-b224-4aa0-a164-39597d846594 trabaja snapshot10archivos en cursor-direct-revision-review. Protecciones source_changed no se deben desactivar globalmente.

Escritor exclusivo actual: Cursor Replace exec23819/chat88f58cad-0758-4874-ad0b-96da569670e7, owns extraction.rs, orden cursor-replace/ORDER.txt, snapshot before/BASE. Asociar respaldo original exacto y tomar lock de copia antes de sus bytes, registrar bajo lock. Direct revision NO forma parte de este encargo; revisión paralela no modifica checkout.

No-op reviewer3489 terminó: cursor-noop-design/REVIEW.md. Propone mantener backup previo y, con prueba de árbol vivo intacto, retirar solo copia nueva sin referencias tras no-op. No implementar todavía: primero fallos de revisión/Replace y demostrar contratos de recuperación/reporte; no poda global ni retrasar backup a ciegas.


### Tanda 5: Replace y detección HTML validados en debug

Cursor Replace23819 y SugarCube aislado65284 terminaron. Integración SugarCube comprobó SHA de los cuatro archivos base antes de copiar solo sugarcube.rs. Coordinador aplicó rustfmt acotado. 50 pruebas extracción,22 transacción (1 ignorada),24 SugarCube,20 HTML, Clippy estricto y debugCLI correctos. Procesos reales verifican Replace carpeta/archivo individual tanto HTML como SugarCube, origen intacto, SHA original exacto, DB reabierta y perfil/almacén distinto, HTTP, aplicación/reversión byte a byte; rechazos de respaldo ausente/corrupto/erróneo conservan ZIP previo y explícito pristine funciona. BATCH5.json detalla binario y ejecuciones. Release/app nativa siguen tanda2.

Revisión independiente Cursor8060/chat341c0d48-8873-469e-80f4-49aabb72d81a activa en snapshot congelado cursor-replace-review. Diseño Direct segundo followup48640 continúa chat6fe8e58f-b224-4aa0-a164-39597d846594; ADJUDICATION rechaza identity-only sin validar original y exige resolver drift antes de escribir. No escritor de fuente activo.

Nueva prueba real verify-direct-cumulative.py, direct-cumulative-baseline.log, run direct-cumulative-1790036555974455600: dos HTML traducidos en tandas sucesivas quedan ambos traducidos en disco, pero ZIP final solo contiene two.html. La preocupación por pérdida de inventario acumulado ya es reproducción, no mera inferencia. Junto con A->B omitido es la siguiente corrección. Preservar original O para distribución, backup de A para recuperación, y guardas source_changed.


### Siguiente escritor activo: registro Direct acumulado

Cursor exec10553/chatc049bfc7-50bc-4e49-ac5e-89f528f5b451 owns solo extraction.rs, nuevo before/BASE en cursor-cumulative. Guarda unión de archivos previos verificados y nuevos, conserva respaldo originalO y separa backup recuperaciónA; detecta drift/falta/error de backup antes de escribir. No implementa rebase A->B, identidad ni cambios Add/Replace. Revisión congelada Replace8060 sigue activa. Verificador cursor-cumulative/verify.ps1 preparado, no ejecutado mientras escritor viva.

Tanda5 también pasa UI real en backup-restart-ui-1790036629073; captura ui-cold-pack-1024.png inspeccionada. Reproducción Direct ampliada a HTML y RenPy loose: A y repeatA correctos, B omitido, O hash exacto. Cuatro escenarios de preflight adverso reproducidos sobre copias nuevas; verify-direct-refusal.py exigirá rechazo antes de modificar árbol/registro. Evidencias y handles actuales en NEXT.md/STATE.json. Cuota última lectura46%usado/54%restante, sin canjes. Goal continúa activo.


### Regresión detectada al ampliar Replace a cinco motores

La validación general sigue incompleta: batch5-replace-matrix da20/25. HTML/RPGMaker/RenPy/Unity pasan cinco destinos, pero Unreal rechaza con gamebusy al tomar el mismo lock ya retenido por core. Se reprodujo también Tyrano loose. No cambian originales. Revisión independiente8060 terminó sin bloqueantes en su snapshot limitado, que no incluía esos plugins; el resultado de proceso tiene prioridad. BATCH5 corregido a scoped-pass/broader-fail.

Cursor aislado55773/chat9bab4ff1-56de-4808-8beb-1f1b3924231b implementa delegación explícita de lock para Unreal/Tyrano/Yuris y trait/callReplace. Root cumulative10553 sigue activo; nunca copiar extracción aislada completa por encima de su trabajo. Integración selectiva y serial después de cierre y checks de ambos. Goal permanece activo.


### Registro acumulado: primera validación y corrección

Cursor10553 terminó solo extraction.rs. Formato correcto; 55 tests extracción pasan,2 fallan. Una prueba nueva tenía un fixture que se detenía en la validación de entradas anterior. La otra es una regresión real en un test existente: Direct sobre una copia Replace debe aceptar respaldo cuyo origen es la carpeta anterior, no exigir que sea la copia. Se pidió validar el source_path registrado contra el manifest del respaldo, además de probar source_path alterado. Logs fallidos conservados.

Misma sesión de implementación reanudada exec69538, revisión congelada59410 y escritor aislado55773 siguen separados. Tres lanes temporalmente, memoria comprobada24GBlibres; sin otras compilaciones. docs/QA-2026-09-21.md ofrece resumen legible; NEXT/STATE tienen handles. Última lectura cuota52%usado. Goal activo, no limitación externa impeditiva.

## Cierre Codex directo: tandas 7 y 8

Usuario aclaró: suscripción ChatGPT/Codex, NO Cursor. Se resolvió la pregunta pendiente; todos los encargos Cursor quedaron terminales. Corrección directa de bloqueo anidado Replace y de A→B HTML/RenPy suelto mediante retarget de entradas verificadas, sin superponer originales. Ver docs/QA-2026-09-21.md y tmp/goal-20260921/BATCH8.json.

1.413RustPASS/0FAIL/16ignored,59suites;fmt/Clippy/releasePASS.25Direct/25ReplacePASS; Tyrano/Yuris y CLIcorrection/identity/cumulative/legacy/refusalsPASS. UIbaseline y A→B/reinicio/packPASS,capturas vistas. Rule95 25ZIPnuevos/79invocacionesPASS,fuentesintactas. CLI SHA7785ee119f4ab23d12152b3c743819204b8acf6a780c0a4e2c254009a14a8c2b;desktopSHA4234d4d129878abdba26e82e38c9bd673d3de73ffa787b63505369e8cb3cdee4. Nativewindowsanteriores noactualizadas; actualQAheadless.

Fallosantiguos preservados: consejo reinjecttrasdrift corregido; HTTPautoprovenance Replace ahora200conSHAverificado,wrongbackup400; QA Direct incluyódiarioprivado y se corrigióvalidandosu identidadantesdeexcluir; prefijoWindowsresueltoconsamefile. No debilitarassertsdebytes. No llamar a scripts checkpoint.py otra vez tras finalize: representan progreso intermedio. BATCH8-SOURCE estable262archivos.

Todos los procesos de esta tanda terminales, exec18855exit0 fue último. Goalactivo; próximosdeduplicación/índices,noopretention segura,packraces,fuentes/compatibilidad. NoCursor/GrokBuild/reset/compra/publicación/commits.

## Cierre de tanda 9: búsquedas e identidades

Implementación directa con Codex; sin Cursor ni APIs externas. Unión de rutas con identidad normalizada por elemento, agrupación de entradas por archivo e índices HTML/RenPy. Conserva fuentes, locadores, alias y rechazo de ambigüedades. Cuatro fuentes cambiadas respecto de BATCH8; copias y diff en codex-performance; BATCH9-SOURCE comprueba 262 archivos sin deriva.

1.414 tests normales PASS,0FAIL,17ignored; benchmark explícito adicional PASS;fmt/Clippy/releaseCLI+desktopPASS.25Direct+25Replace producen mismos bytes y hashes de manifiesto que BATCH8. CLIextended/legacy/refusal y UIA→B/restart/packPASS. Exec32213 terminal0. Captura nueva1024×768 inspeccionada, botónEmpaquetar visible.

Medición release: segunda inserción250archivos4.205→1.741s;500archivos14.567→3.613s. Corrección6000HTML1.475→1.293s;RenPy4.897→2.691s. Primera inserción500sinmejora significativa7.140→7.335s. Un solo par de mediciones por tamaño en Windows compartido, no garantía. Microbenchmarkdebug deunion500rutas11.657s→23.965ms, separado del flujo total.

CLI SHAc36225b3df27caf0324ab959e006671ab47269603c23976f353a5918a2500cfb;desktopSHA16ec636607631405c224f73f6d45fefebfaea5c529eaff407c698e6747dd2847. UIC:\Projects\Locust\tmp\goal-20260921\backup-restart-ui-1790042204938. Rule95 no repetido en esta tanda; comparación byte/hash conserva payloads ya verificados enBATCH8. Nativewindowsanteriores no actualizadas. No más procesos vivos de esta tanda.

Goalactivo. Próximo: repro determinista de carreraNone→Some en selección de respaldo de pack; leer codex-performance/PACK-RACE-NEXT.md. Retención segura no-op y mutación de payload trasvalidación siguen pendientes; no aplicar podaglobal. No volver a ejecutar checkpoint.py tras este cierre porque reescribiría estado intermedio.


## BATCH10 — selección de respaldo durante empaquetado (2026-09-22T02:13:42.649763+00:00)

Trabajo realizado directamente con Codex/ChatGPT; Cursor y Grok Build deshabilitados por petición explícita. Solo cambia `crates/core/src/patch/pack.rs` respecto a BATCH9. Se reprodujeron dos fallos deterministas: una primera inserción aparecida después de seleccionar respaldo producía ZIP structural sin hashes originales y sustituía el archivo anterior. El selector de idioma/root ahora se comparte y devuelve una generación concreta antes de resolver respaldo; el empaquetado conserva esa generación y vuelve a comprobarla bajo GameLock. Mantiene diagnóstico sin traducciones, idiomas ambiguos, root incorrecto, override pristine y procedencia legacy.

Cuatro intercalados prueban ausencia→registro y cambio de registro, con idioma explícito/automático: rechazo sin tocar ZIP previo, bytes de la inserción concurrente intactos, reintento strict verificado contra el original. Suite completa: 1418 PASS / 0 fallos / 17 ignoradas en 59 suites. fmt/Clippy y release CLI+desktop pasan. 25Direct+25Replace (Unity,Unreal,RPGMakerMV,HTML,RenPy × ES/FR/DE/PT-BR/ZH-CN) mantienen hashes de juegos/manifest de BATCH9. NuevosZIPs ejecutados por consumidor Rule95 independiente (79 llamadas); interfaz/backendrelease headless con reinicio,pack,verify,apply yrollback pasan.

CLI SHA256 `a649f5dbea287914523b8c88dc82e57f3e0b2799b65b5edc597e44ee8073331a`. Desktop SHA256 `b5776a99aa275c7dd99de11800f0907ca6be0bcda4bfd3a7fe72948d9e359431`. Evidencia: `tmp/goal-20260921/BATCH10.json`, `BATCH10-SOURCE.json`, `codex-pack-selection/before-regression.log`, `CHANGE.diff` y `verification/`. No certifica juegos comerciales completos, todos los contenedores, fuentes/TMP/RTL ni ventanas nativas actualizadas. Pendientes: carrera del payload del backup (nota separada),retención no-op y cobertura runtime. Goal sigue activo.


## BATCH11 — huellas originales estables al empaquetar (2026-09-22T02:27:08.955444+00:00)

Trabajo directo con Codex/ChatGPT, sin Cursor ni Grok Build. Se reprodujeron tres fallos: cambiar o borrar un original, o añadir su contraparte después de validar el respaldo, producía un ZIP con huellas originales incorrectas y sobrescribía el archivo previo. `VerifiedPristine` conserva el inventario validado; el empaquetador contrasta cada huella o ausencia antes de publicar. Para un solo archivo se mantiene la copia temporal verificada, el nombre Unicode y la limpieza incluso si falla el consumidor. Los alias de mayúsculas utilizan un índice del inventario construido cuando hace falta y rechazan ambigüedades. No se añade otra copia ni otro recorrido del respaldo. La API pública `with_pristine_tree` sigue siendo compatible con los demás consumidores.

Los tres intercalados se rechazan y conservan el ZIP, la DB y los archivos traducidos; el reintento con el respaldo restaurado verifica Clean. Las pruebas adicionales cubren limpieza de temporales con nombres Unicode y alias de mayúsculas cuando desaparece el original. Total: 1423 PASS / 0 fallos / 17 ignoradas en 59 suites; fmt, Clippy y compilación release pasan. Los 25 casos Direct y 25 Replace (Unity, Unreal, RPG Maker MV, HTML y Ren'Py × ES/FR/DE/PT-BR/ZH-CN) conservan los hashes de BATCH10. Rule95 consume los nuevos ZIPs (79 llamadas). La interfaz y el backend release pasan las pruebas de reinicio, empaquetado, verificación, aplicación y reversión en segundo plano.

CLI SHA256 `a18d212897ab275582c3d85d2775dff141e08000f489034e2b1667763e233d92`; desktop `452a35009207b09eb475d9e71a54698ef2d01e150783352a8a88b27437a94036`. Evidencia: `tmp/goal-20260921/BATCH11.json`, `BATCH11-SOURCE.json`, `codex-pristine-integrity/before-regression.log`, `CHANGE.diff` y `verification/`. Límites: las carpetas prístinas manuales no tienen inventario histórico; los demás consumidores de rutas de respaldo y la copia de bytes traducidos al ZIP son asuntos separados. No se certifica la ejecución de todos los juegos comerciales ni la actualización de las ventanas nativas. El goal sigue activo.


## BATCH12 — integridad de los bytes escritos al ZIP (2026-09-22T02:40:58.468205+00:00)

Trabajo directo con Codex/ChatGPT, sin Cursor/Grok Build. Se reprodujeron tres fallos: reemplazar bytes manteniendo tamaño, alargar o recortar el archivo después del hash previo podía publicar un ZIP inconsistente y sobrescribir el anterior. El empaquetador abre cada origen una sola vez, conserva el control rápido de tamaño y calcula SHA-256 sobre los bloques que escribe. Solo publica si tamaño y huella coinciden con la inserción registrada. Se comparte el bucle acotado con verificación/aplicación sin cambiar sus mensajes; los errores de lectura y escritura no se confunden con crecimiento del origen.

Pruebas nuevas: tres intercalados con archivos de más de dos bloques, preservación de ZIP/DB/ediciones externas, limpieza del temporal y reintento verify/apply/rollback exacto. Además, entrada vacía, escrituras parciales y errores de lectura/escritura. Total: 1429 PASS / 0 fallos / 17 ignoradas en 59 suites. fmt, Clippy y release CLI+desktop pasan. Los 25 Direct y 25 Replace conservan todos los hashes de juegos y archivos del manifest de BATCH11; Rule95 consume los nuevos ZIPs (79 llamadas). La interfaz y el backend release pasan reinicio, pack, verify, apply y rollback en segundo plano.

Se elimina una lectura completa del origen por construcción y se mantiene el búfer de 1 MiB; no se afirma una mejora de tiempo medida. CLI SHA256 `2fb61f4db2daae9392c8aa4f5b40890ecc462fd79d40ac2e8e5b9f248eb91948`; desktop `7e1d8cac4d1b187dd6a8748a5632ac2069db1feff36b297c2b4493a7aacda52c`. Evidencia: `tmp/goal-20260921/BATCH12.json`, `BATCH12-SOURCE.json`, `codex-pack-stream/before-regression.log`, `CHANGE.diff` y `verification/`. Pendientes: retención de respaldos de operaciones sin cambios, lectores de respaldo en Direct y cobertura de fuentes/juegos reales. Goal activo; ventanas nativas antiguas no actualizadas.


## BATCH13 — retención de respaldos en reinserciones idénticas (2026-09-22T03:09:22.583879+00:00)

Trabajo directo con Codex/ChatGPT, sin Cursor/Grok Build. Reproducido con CLI real en HTML y Ren'Py: tres ejecuciones idénticas retenían 1→2→3 respaldos; una primera traducción igual al original retenía uno sin registrar archivos. Ahora retienen 1→1→1 y cero, respectivamente. Se mantiene el respaldo obligatorio previo a cualquier plugin/metadato de transacción; solo se elimina el duplicado nuevo tras completar, con GameLock, plan vacío, inventario vivo intacto, outcomes sin escritura, ID no referenciado y validación de generación/inventarios del respaldo y origen. Nunca se podan generaciones anteriores.

El informe de una reinserción conserva el ID/ruta del original existente, incluso desde otro perfil, o devuelve ID vacío/ruta null cuando no hay original registrado. Si el borrado falla parcialmente, avisa sin presentar la copia dañada como respaldo utilizable. Se añadieron pruebas para cambios externos en un plan vacío, fase terminal y bloqueo, directorios creados, cambios tardíos/fallo de registro, referencia por otro idioma, recibos .locust, cambio de generación, corrupción y archivo bloqueado en Windows. La API prueba cinco reinserciones, carpeta/archivo individual, primer noop y empaquetado tras reiniciar.

Validación: 1439 PASS / 0 fallos / 17 ignoradas en 59 suites; fmt/Clippy y release CLI+desktop. Los 25 Direct y 25 Replace conservan hashes de BATCH12; Rule95 consume nuevos ZIPs (79 llamadas). La interfaz real en navegador oculto prueba A→B, noop sin tercera copia, original conservado, primer noop sin ruta de backup ni CTA de pack, reinicio y verify/apply/rollback. Capturas y JSON en `C:\Projects\Locust\tmp\goal-20260921\backup-restart-ui-1790046542812`.

CLI SHA256 `2d3ddef26131257ca46824c9015f6fd36c235ae3780f66e7a7861bc9d5ab0880`; desktop `69d0dfc4f4276cc55d5f0e2f6fbc322432b3940cdd3d358fae6fa26afda5bba4`. Evidencia: `tmp/goal-20260921/BATCH13.json`, `BATCH13-SOURCE.json`, `codex-noop-retention/before-cli-verified.log`, `before-regression.log`, `CHANGE.diff` y `verification/`. El primer intento del harness confundía LF con CRLF en Ren'Py; se corrigió comparando los bytes originales reales, sin cambiar producto por ese fallo. No se promete menor latencia: siguen existiendo la copia previa y la de trabajo, y se verifica el duplicado antes de eliminarlo. Permanecen diarios de transacción, retención de Add/Replace, lectores de respaldos y cobertura de fuentes/juegos reales. Goal activo.


## BATCH14 — distinguir texto idéntico de fallos (2026-09-22T03:18:37.641403+00:00)

BATCH14 validated: unchanged injections show a localized informational result only with complete all-unchanged counts, zero writes/files, no failed/missing languages and no actionable warnings. Unknown/failed cases remain actionable. No Pack CTA or language-registration side effect for zero writes. Backup cleanup exact notice is localized; recording status neutral for confirmed no-op. 31 frontend test files, final targeted regression, production frontend/desktop release and 11 real UI/backend checks PASS; source drift remains red and preserves user edits. Direct Codex only; goal ACTIVE.

Reproducido antes de editar: `codex-noop-ui/before-regression.log` falla porque un resultado completamente idéntico era `empty`. Añadido estado `unchanged` informativo con evidencia estricta por idioma, cuentas coherentes, archivos intactos y advertencias conocidas exactas; skips desconocidos, falta de cobertura, fallos de idioma, registros contradictorios y problemas de limpieza no entran en ese estado. Se preservó la guarda anterior de pack para resultados sin idiomas procesados. El registro y el aviso son informativos; no se dispara registro automático de idioma con cero escrituras. Inglés/español tienen las mismas claves.

Los 31 archivos unitarios pasaron; después de reforzar la guarda de pack se repitió su suite, build y QA visual. Backend CLI sin cambios (SHA `2d3ddef26131257ca46824c9015f6fd36c235ae3780f66e7a7861bc9d5ab0880`); desktop recompilado `2107dcbdcdfecac6ba6389008997c80d2487cd13045b4f59b2e821d1043e661a`. UI real con perfiles privados en `C:\Projects\Locust\tmp\goal-20260921\backup-restart-ui-1790046933346`: inserción A→B, repetición sin tercera copia, identidad inicial sin backup, pack estricto tras reinicio, verify/apply/rollback, y modificación externa que sigue roja con `source_changed` sin alterar bytes. Capturas inspeccionadas. No se movieron ventanas ni el cursor. Referencia de fuentes: `BATCH14-SOURCE.json`; cambios: `codex-noop-ui/CHANGE.diff`.

Rust y matriz no se repitieron: no hubo cambios del motor desde BATCH13. Pendientes: lectores de respaldos en prepare_direct_revision, cobertura real de runtime/fuentes, retención de diarios y Add/Replace. Los diarios vacíos todavía muestran información de originales de recuperación; revisar ese detalle sin esconder recuperación de escrituras reales. CLAUDE.md ya recoge la ruta autorizada Codex/ChatGPT sin Cursor/Grok Build.


## BATCH15 — originales verificados durante la revisión Direct (2026-09-22T03:33:10.714950+00:00)

BATCH15 validated: 1448 Rust PASS,0fail,17ignored,59suites; fmt/Clippy/release CLI+desktop,50 hash-equivalent matrix cases,Rule95 79 calls,HTML/RenPy extended revision/noop CLI and11 actual UI checks PASS. Checked original readers hash the exact consumed bytes; missing originals cannot masquerade as generated output, RenPy parses loaded text without reopening. Five reproduced before-fix failures now refuse safely;9 new tests. Goal ACTIVE, Codex/ChatGPT only.

Cinco pruebas antes del arreglo demostraron aceptación de un respaldo modificado después de verificar su inventario: mismo tamaño, crecimiento, truncado, borrado y árbol normalizado de archivo individual. Se conservan en `codex-revision-originals/before-regression.log`. `RevisionOriginal` mantiene hash/tamaño del inventario inmutable y solo entrega texto cuyos bytes consumidos coinciden; no expone una ruta sin comprobar al plugin. El límite de lectura evita retener crecimiento por encima del tamaño original. La existencia se decide desde el inventario, así que borrar un archivo no hace pasar su ausencia por un overlay generado. HTML lee su original una vez; RenPy extrae filas de los mismos strings original/actual ya leídos, eliminando reaperturas durante el análisis.

Las pruebas nuevas verifican conservación del juego/registro/respaldo de recuperación, rechazo de bytes corruptos aunque la ruta vuelva a contener el original antes de un hipotético rehash, entradas ilimitadas/truncadas, UTF-8 inválido, y retargeting real HTML/RenPy que falla sin cambiar el juego y permite reintentar al restaurar el original. Se conserva el comportamiento A→B, omisiones parciales, identidad/reextracción, procedencia original y apply/rollback exacto con CLI release. Los 50 casos de matrices conservan cada hash de juego y manifiesto de BATCH13. Rule95 lee los ZIPs nuevos sin modificar el consumidor. La UI mantiene el estado informativo de identidad y el error real de fuente cambiada; capturas en `C:\Projects\Locust\tmp\goal-20260921\backup-restart-ui-1790047967520`.

CLI `0c705df539151707502002d6abeaaee4cdfb584adf543deaf522d9c24d665e32`; desktop `34ad6025e866100ee4d3c2d9635cfd0cf1ca723c4fbbd245b0e78777ce4c0e9b`. Evidencia: `BATCH15.json`, `BATCH15-SOURCE.json`, `codex-revision-originals/CHANGE.diff` y `verification/`. No hubo cambios de frontend desde BATCH14, ni nuevos tests de runtime de juegos/fuentes. Cambio de API Rust: implementaciones externas de `FormatPlugin::prepare_revision_entries` deben aceptar `HashMap<PathBuf, RevisionOriginal>` y usar `read_text()`. Se mantienen los contratos CLI/HTTP y formatos de parches. No se añade otra copia completa del juego; permanece buffering por archivo y no se afirma mejora de latencia medida. `with_pristine_tree` sigue público para compatibilidad; los lectores de revisión y pack usan inventario verificado. Goal activo.


## BATCH16 — fuentes activas y texto visible (2026-09-22T03:46:58.441707+00:00)

BATCH16 validated: 1453 Rust PASS,0fail,17ignored,59suites; fmt/Clippy/release CLI+desktop,localization/production frontend,real font CLI/UI and11 revision UI checks PASS. Recovery copies excluded; explicit recovery audits and similar user names retained. Font audit covers untranslated physical text through pivots and surfaces stale/malformed inputs. BATCH15 patch matrix/Rule95 evidence retained; no engine-change reruns. Goal ACTIVE, Codex/ChatGPT only.

Baseline real CLI en `font-recovery-1790047869495602800`: una fuente activa y una copia de recuperación producían2 fuentes/1 fallo por un archivo inválido dentro de .locust-injections. El nuevo CLI da1/0; seleccionar explícitamente la carpeta de recuperación sigue dando1/1. Descubrimiento y auditoría comparten exclusión de directorios internos, sin ocultar .locust-injections-old ni una raíz seleccionada explícitamente. Cinco pruebas nuevas cubren conteos, carpetas/archivo individual, mayúsculas, fuentes ilegibles reales, texto pendiente normal/pivotado/vacío, metadatos inválidos y traducciones obsoletas. Tres pruebas de servidor fallaron antes del cambio (`codex-font-recovery/before-server.log`).

El helper común HTTP/Tauri audita traducción no vacía o `injection_source()` físico, con validación de vigencia, igual que el generador de parches de fuente. Los catálogos EN/ES explican el corpus ampliado. Capturas de UI real en `C:\Projects\Locust\tmp\goal-20260921\font-audit-ui-1790048800499` muestran3 glifos japoneses ausentes mientras esa fila está pendiente, y0 al traducirla al inglés; se analiza solo la fuente activa, sin modificar el juego. La regresión de inserción A→B, reinicio, pack/apply/rollback y fuentes cambiadas también pasó en `C:\Projects\Locust\tmp\goal-20260921\backup-restart-ui-1790048802573`.

CLI `61384bbee73c8aa718bb539243cfd09292c8437185e6578f20878f33d8280363`; desktop `1ee3b7a94e7bb9a45f8b97648ec540bee94605b2bb9298e79e717f7e92179aca`. Evidencia: BATCH16.json, BATCH16-SOURCE.json, codex-font-recovery/CHANGE.diff y verification. No se repitieron50 casos de matrices/Rule95 porque no hubo cambios de inserción/empaquetado; su evidencia corresponde a BATCH15. La fuente sintética solo prueba cmap, no glifos dibujados. Persisten límites de selección de fuentes, shaping/RTL, atlas TMP y ejecución real de juegos. No se abrieron ventanas nativas ni se movió el cursor. Goal activo.
