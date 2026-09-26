# Continuación: motor, parches y medición de Grok

Fecha: 2026-09-11. Trabajo sobre Locust y Rule95 Patcher. Complementa las revisiones anteriores; no se publicó Rule95 web ni se modificaron los juegos originales.

Actualización posterior: [matriz multilingüe, nuevas correcciones y ejecutables de Locust](C:/Projects/Locust/docs/ENGINE-MATRIX-2026-09-11.md). Los recuentos y binarios de este documento corresponden a la fase anterior.

## Cambios integrados

- HTML se extrae e inyecta por ubicación y contenido esperado. Dos textos iguales pueden tener traducciones distintas. Se conservan scripts, estilos, comentarios, atributos no traducibles y marcado interior; se escapan las traducciones según su contexto. Bases antiguas sin ubicación solo se aceptan si la coincidencia es única. Entidades HTML completas y numéricas, incluyendo C1, tienen pruebas.
- Los informes detallan motivos de omisión y la interfaz distingue resultados parciales. Unity verifica destinos y texto original antes de contabilizar escrituras, conserva filas de tablas no seleccionadas y rechaza traducciones vacías. Otros motores conservan sus contadores y usan `unclassified` cuando aún no desglosan el motivo.
- La inyección directa y Add también validan las rutas antes de crear el respaldo o escribir. Rechazan bases que apunten fuera de la selección, traversal, enlaces interiores y el directorio de recuperación `.locust`. Se permite la junction raíz elegida y el sufijo virtual de un bundle UnityFS. No se detectan hardlinks preexistentes ni se afirma protección ante cambios concurrentes del árbol.
- El modo copia remapea las rutas de las entradas antes de entregar el trabajo al motor. Crea una carpeta nueva, rechaza enlaces interiores y copia archivos independientes, incluyendo medios. Una salida ya existente se rechaza. Se conserva el respaldo de recuperación. No se afirma exclusión frente a otro proceso que cambie enlaces durante una escritura.
- Por defecto no se recortan ni eliminan acentos de una traducción para encajarla en un slot binario. Una traducción demasiado larga se conserva para revisión y no entra en memoria. El ajuste mecánico requiere `--allow-lossy-binary-fit`.
- Los lotes tienen un presupuesto aproximado de entrada de 6.000 tokens, además del número máximo de cadenas. Cuenta texto y contexto por bytes; no es un tokenizador exacto ni una garantía de límite de contexto. Una cadena individual demasiado grande se envía sola.
- Grok tiene un plazo de respuesta de 180 segundos manteniendo cancelación inmediata. CLI usa lotes de 10 y 10 peticiones simultáneas para Grok, salvo flags explícitos. El diálogo ofrece un botón para aplicar ese ajuste sin sustituir preferencias guardadas.
- El coste conserva un indicador de integridad desde SQLite hasta CLI, eventos e interfaz. Una suma parcial nunca se presenta como total y una ruta OAuth sin precio comunicado muestra coste desconocido. La contabilidad de lotes respeta proveedores que informan el coste agregado solo en la primera entrada.
- Un fallo recuperable de lote usa `BatchFailed`; no cierra prematuramente el trabajo ni impide continuar con los proveedores de respaldo. Los errores terminales siguen usando `Failed`.

Los reintentos binarios también comprueban el presupuesto antes de enviar una corrección. No aceptan IDs ajenos/duplicados ni una respuesta vacía o con variables dañadas; conservan el intento válido anterior y registran el consumo recibido. Memoria y glosario pasan las mismas guardas de integridad. Opciones inválidas se rechazan antes de iniciar la cadena de proveedores. La interfaz también rechaza presupuestos negativos o mal formados; un cero conserva un límite cero, en vez de convertirse silenciosamente en gasto ilimitado.

## Correcciones posteriores de Cursor Sol High

Cursor CLI revisó una captura de 19 archivos y señaló tres fallos confirmados. Una segunda sesión escribió las correcciones en cinco archivos; el coordinador revisó el diff, aplicó formato y ejecutó las pruebas integradas.

- Unity heurístico conserva `binary_offset` y endianness, valida fuente/rango y escribe en esa posición. Una posición inválida no deriva a búsqueda global. Las bases antiguas solo se aceptan cuando hay coincidencia única; los duplicados se omiten como `ambiguous_target`. Los destinos se resuelven antes de las escrituras heurísticas para que una modificación no desplace la siguiente.
- El presupuesto se verifica también después del coste real recibido, incluido el último lote, IDs inválidos y correcciones binarias. Se conservan traducciones válidas y consumo, pero el resultado comunica exceso o coste no verificable y detiene los siguientes despachos y el fallback. No revierte un gasto ya ocurrido ni garantiza un techo exacto de facturación.
- Un retry de transporte exitoso de proveedor de pago marca el coste como incompleto: la respuesta final no demuestra el coste de los intentos anteriores. El contrato del proveedor todavía no informa costes acumulados por intento. Los proveedores gratuitos conservan su tratamiento de coste conocido.
- El filtro ZIP rechaza también `CONIN$` y `CONOUT$`, incluyendo variantes de mayúsculas/minúsculas y extensiones. Las pruebas no abren dispositivos reales.

Regresiones nuevas: segundo duplicado seleccionado, traducciones diferentes para duplicados, legacy ambiguo/único, offset obsoleto/fuera de rango, endian, último lote/retry con coste real mayor o desconocido, IDs inválidos sin nuevos despachos y retry de transporte pagado/gratuito. Total integrado: **882 aprobadas, 0 fallidas y 9 omitidas**; Clippy con `-D warnings`, formato y diff correctos.
## Juego UnityFS y paquete de distribución

Juego de prueba: CCTV - USSR, Unity 6000.0.24f1 / UnityFS v8. Se trabajó únicamente en copias bajo `tmp/continuation-2026-09-11/cctv-ussr/`.

1. Extracción nueva: 4.374 entradas.
2. Reutilización de la base histórica en modo lectura: 4.059 traducciones con coincidencia no ambigua de fuente/contexto. Las 315 restantes no se asignaron a ciegas.
3. Inyección: 2.290 escritas, 1.768 idénticas al original, 1 demasiado larga. Un archivo UnityFS modificado. Las traducciones históricas no representan una revisión lingüística nueva.
4. ZIP estricto construido con hashes de la carpeta original leída. Tamaño aproximado 234 MiB: contiene el bundle completo modificado, no un delta binario. El README generado para futuros parches se corrigió para explicar que pueden incluir bundles completos.
5. Se extrajo el ejecutable del instalador NSIS generado. Su UI nativa verificó el parche como `Clean` y lo instaló sobre otra copia limpia. El SHA-256 instalado coincide exactamente con el inyectado.
6. El auxiliar `apply_once.exe` del mismo paquete restauró una copia aislada del estado instalado: SHA-256 idéntico al original. La copia `consumer` queda traducida para revisión visual.
7. Un test optativo real de Unity volvió a extraer y modificar un bundle aislado: aprobado, 200 segundos en build debug. Los tests reales de Unity/Ren'Py ahora exigen una copia explícita por variable de entorno y escriben solo en un nuevo fixture temporal.

Arranque/escenas/fuentes/guardado/carga: **pendientes**. El lanzamiento no llegó a crear proceso ni ventana del juego; apareció SmartScreen al intentarlo. No se automatizó ni evitó una pantalla de seguridad. La [skill de control de Windows](C:/Users/Mike/.codex/plugins/cache/openai-bundled/computer-use/26.903.71938/skills/computer-use/SKILL.md) exige leer su guía, que establece: «Do not automate Windows security or anti-malware apps.» Esto impide intervenir automáticamente en SmartScreen; no demuestra que el juego tenga un fallo. Se pidió apertura manual de la copia. Los perfiles y preferencias de CCTV - USSR seguían ausentes al comprobarlo.

Instalador: `C:/Projects/rule95-patcher/target/release/bundle/nsis/Rule95 Patcher_0.1.0_x64-setup.exe`, 4.017.269 bytes, sin firma Authenticode. SHA-256: `73A1DE1A0836C4250E1D97873727102CA76FEE4BFE27435BCA67DA43C09343F1`. El paquete final se volvió a compilar después de las correcciones de Cursor; su auxiliar extraído repitió instalación y restauración con hashes correctos (cursor-payload-hashes.json). La comprobación visual de la UI se hizo con el paquete anterior, cuya interfaz no cambió. Se probó el payload extraído del instalador; no se ejecutó su asistente de instalación ni desinstalación en Windows.

## Medición de API

Grok 4.6 por OAuth guardado de Locust; inglés → español; sin memoria ni glosario en el corpus. Texto de diálogo extraído de una base real, convertido a un fixture HTML. Calibraciones con las mismas 100 cadenas; prueba mayor con 1.000. El coste monetario es desconocido. El total de tokens del proveedor incluye categorías que no se reflejan necesariamente en entrada + salida.

| Concurrencia | Cadenas | Segundos | Cadenas/minuto |
|---|---:|---:|---:|
| 1 | 100/100 | 681,024 | 8,81 |
| 3 | 100/100 | 224,317 | 26,75 |
| 6 | 100/100 | 139,466 | 43,02 |
| 10 | 100/100 | 78,587 | 76,35 |

Prueba mayor: **1.000/1.000 cadenas**, 697,396 segundos (11 min 37 s), **86,03 cadenas/minuto**, cero errores de lote. Uso comunicado: 572.684 tokens totales, 158.442 de entrada y 16.817 de salida. Sumando calibraciones: 1.400 traducciones de entradas, 804.507 tokens totales comunicados. Validación: las cuatro calibraciones no presentan incidencias. En la prueba grande hay un aviso por una vocalización conservada idéntica; ninguna traducción vacía ni incidencia de variables. No se alteró esa respuesta para maquillar la medición. Las calibraciones son una muestra por ajuste, no una garantía de velocidad futura. El piloto de lotes de 40 agotó dos plazos de 60 segundos y se detuvo; no se sumó a las cifras de rendimiento ni se infirió coste cero.

La herramienta repetible está en `tools/benchmark_api.py`. Crea copias de las bases, conserva logs/resultados y, para ejecuciones futuras, fija también una copia y hash del ejecutable. Evidencia actual: `tmp/continuation-2026-09-11/benchmark-v2/results.json`.

## Verificación final

- `cargo test --offline --workspace --quiet`: **882 aprobadas, 0 fallidas, 9 omitidas**. Incluye pruebas CLI de copia Wolf/Ren'Py con originales intactos, recording/ZIP correctos, instalación y restauración. Se sustituyeron expectativas antiguas que exigían el fallo causado por escribir en el original.
- Prueba optativa sobre bundle Unity real aislado: **1 aprobada** adicional.
- `cargo clippy --offline --workspace --all-targets -- -D warnings` y `cargo fmt --all --check`: correctos.
- Interfaz: **26 archivos de pruebas correctos**, compilación TypeScript/Vite correcta. Persiste advertencia de tamaño del bundle JS, sin fallo de build.
- Patcher: **6 tests Rust y 3 UI correctos**. NSIS final compilado, ejecutable extraído abre su UI; auxiliar final instala/restaura el bundle real y devuelve el SHA-256 original.
- `git diff --check`: correcto.

Los logs y hashes se conservan en `tmp/continuation-2026-09-11/`. El instalador final está enlazado arriba. Las pruebas no certifican todas las versiones de motor ni equivalen a jugar el juego completo.

## Límites restantes

- Completar la prueba visual y de partidas de Unity y ampliar la matriz de juegos/versiones; no se certifica compatibilidad universal.
- Reescritura o relocación de formatos binarios que permita aumentar longitud. Los slots fijos siguen exigiendo traducciones que quepan.
- Desglose específico de omisiones en los demás motores y cobertura instalada persistente por entrada. `PackReport.translated_strings` sigue contando traducciones almacenadas; el CLI ahora lo etiqueta así.
- Uso/coste recuperable de respuestas HTTP malformadas, estimaciones de precios por modelo y mediciones repetidas. No hay garantía de tope exacto de facturación.
- Simulación completa de upgrades, exclusión entre instaladores concurrentes y más pruebas de interrupciones de proceso. Firma del instalador y validación de su asistente en una instalación aislada de Windows.
- Integración del escáner/DLsite en Patcher y validación del contenido exportado hacia Rule95. Modelos locales siguen fuera de esta fase.

## Coordinación

El trabajo inicial fue del coordinador con agentes nativos de revisión y pruebas. A petición posterior del usuario se usó Cursor CLI con selección explícita `gpt-5.6-sol-high`; ambas sesiones confirmaron `GPT-5.6 Sol 272K High` y `apiKeySource=login`:

- Revisión terminada: `5d7eb704-0152-4d55-a56d-7146db8680b8`.
- Implementación terminada: `65a8db57-06a7-45e5-8271-9404c0b2b27c`.

Se recuperó la confirmación previa de on-demand desactivado/$0 en Papers (`docs/COORDINATION_CHECKPOINT.md:91`), corroborada por ThreeMaker y Runnked. Es evidencia declarada, no una lectura nueva del panel. Cuota restante y coste monetario de Cursor: desconocidos. No se cambió facturación ni se usaron claves externas para los agentes. La skill aplicada fue `C:/Users/Mike/.codex/skills/delegar-codigo-cuotas/SKILL.md`.

La implementación de Cursor quedó limitada a `unity.rs`, `translation.rs`, `patch/zipsec.rs` y dos archivos de tests del núcleo. Su ejecutor rechazó pruebas/formato; esos comandos los ejecutó el coordinador posteriormente con éxito. El registro completo está en `tmp/cursor-sol-high-review-20260911/`, incluyendo órdenes, hashes, informes y sesiones. Cursor escribió estas correcciones finales; no se le atribuyen los cambios anteriores.

Se utilizó además la ruta OAuth de Locust autorizada para traducción. No se hicieron commits, push, publicaciones ni despliegues. La prueba visual del juego continúa pendiente de apertura manual.

Preferencia reiterada por el usuario: aprovechar tareas de Grok en paralelo, aproximadamente 6–10 según recuerda. Ese rango no es un límite oficial verificado. El benchmark de traducción sí verificó 10 solicitudes concurrentes; para agentes de desarrollo hay que comprobar capacidad y aislar escritores. Se conserva la selección explícita de Cursor Sol High hasta que el usuario la cambie. Esta preferencia también está registrada en `CLAUDE.md` para siguientes sesiones.
