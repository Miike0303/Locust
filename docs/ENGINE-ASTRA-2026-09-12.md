# Motor de traducción: continuación Astra

El usuario pidió cerrar anticipadamente la tanda al quedar 5 % de cuota, sustituyendo el plazo original de las 01:00. Este informe reemplaza las limitaciones ya resueltas de `ENGINE-PENDING-2026-09-11.md`. Los resultados de cada ejecutable se identifican por su evidencia; no se atribuyen pruebas de una versión anterior a cambios posteriores.

## Cambios integrados

- **Unity:** reconstrucción de TextAsset validados en SerializedFile 17–22, con actualización de tamaños y offsets; reubicación de nodos UnityFS 6–8. Los objetos desconocidos se conservan. Los originales físicos japoneses se comparten en memoria y SQLite, también al pivotar por inglés; límite de 1 MiB / 16 384 miembros por grupo. Los slots heurísticos de otros objetos siguen teniendo tamaño fijo.
- **UnityFS:** reutilización de bloques comprimidos que conservan exactamente sus bytes descomprimidos, reubicación en el mismo buffer y escritura de una sola pasada. En una copia técnica de CCTV, reutilizar 5 388 bloques conservó una salida de 261,3 MB frente a 311,5 MB sin reutilización. Las optimizaciones posteriores eliminaron buffers completos duplicados y la doble compresión: en la comparación final sin caché, escribir pasó de 46,2 a 18,2 segundos, con un pico residente cercano a 1,028 GB y 12,29 MiB adicionales de memoria comprometida frente a la versión intermedia. Son observaciones en un equipo compartido, no una garantía de velocidad. Se preservaron el significado JSON de dos TextAsset técnicos modificados, otros 3 530 objetos y nueve nodos. No es una prueba de diálogos ni de renderizado del juego. Evidencia: `fonts/unity-writer-singlepass/ASTRA-UNITY-SINGLEPASS.md` y `singlepass-benchmark-results.json`.
- **Unreal:** lectura acotada de PAK clásicos y modernos con índice de directorios completo, LocRes sin compresión o con Zlib/Gzip; análisis nativo de ExternalFile LocRes indexados en IoStore v5/v7/v8 con None/Zlib/LZ4. Se verificaron recursos reales del PAK base de Last Hope y su actualización, además de fixtures independientes de retoc. Las salidas son overlays PAK.
- **Parches:** un único plan de ZIP validado para verificar e instalar; detección de nombres duplicados y alias del manifiesto; archivos temporales de propiedad exclusiva; bloqueo del sistema operativo para impedir aplicar/verificar/revertir simultáneamente el mismo juego desde procesos distintos.
- **Fuentes:** auditoría real de cobertura Unicode y creación de ZIP para sustituir una TTF/OTF suelta existente en HTML, RPG Maker MV/MZ y Ren'Py. Puede combinarse con un ZIP de traducción. La fuente debe cubrir todos los caracteres traducidos y proceder del usuario; no se incluyen fuentes de terceros.
- **Interfaz:** diagnóstico persistente cuando hay contenedores ilegibles, conservación de traducciones ausentes en una extracción parcial, presupuesto binario correcto para grupos Unity y diálogo de creación de parches de fuentes. Carga de páginas bajo demanda; el arranque de Tauri reserva el socket real del servidor antes de usarlo.
- **Rule95 Patcher:** conserva directorios vacíos al copiar, detecta enlaces/reparse antes de crear el destino, evita sobreescribir archivos concurrentes y verifica el ZIP antes de copiar el juego.
- **Concurrencia del proyecto:** reserva atómica compartida entre HTTP y Tauri. La cancelación responde inmediatamente, pero mantiene la protección hasta que terminan el trabajador y sus escrituras SQLite. Cerrar una petición no deja una escritura pendiente libre para afectar otra base. Guardar, revisar, importar, validar, generar fuentes y empaquetar usan la misma protección; los conflictos devuelven un error visible.
- **Archivos de juegos:** los overlays Unreal y los contenedores Tyrano/YPF usan staging exclusivo y respaldos por generación. No se borran archivos ajenos con sufijos fijos. ASAR externo y encabezado se preparan juntos y se restauran si falla la instalación del conjunto. Una terminación abrupta puede requerir recuperación manual desde las rutas conservadas; esta inserción local no añade un journal nuevo.
- **Texto original cambiado:** al volver a extraer, una traducción retenida cuyo original cambió recibe un marcador con hashes. La validación y el editor lo muestran; insertar, pivotar y generar fuentes lo rechazan hasta guardar una traducción actual o marcarla explícitamente revisada/aprobada. Un marcador malformado también bloquea la operación. El estado pendiente de una base antigua, por sí solo, no bloquea traducciones: la protección depende de cambios observados y su procedencia.
- **Reversión de parches:** todas las ramas, incluida una instalación interrumpida, comprueban antes de modificar archivos que cada respaldo necesario existe, tiene tamaño/hash correcto y apunta a un destino válido. La restauración vuelve a comprobar los bytes copiados. Los archivos añadidos y luego editados se conservan salvo reversión forzada explícita; los archivos reemplazados se restauran desde el respaldo conforme al contrato existente.

## Matriz nueva con Grok

El ejecutable congelado para `tmp/pending-astra-20260911/api-growth-matrix` completó cinco motores —HTML, Ren'Py, RPG Maker MV, Unity y Unreal— con japonés → inglés → español/francés/alemán/portugués brasileño/chino simplificado. Cada destino usa su propia base de datos. Se comprobó validación, inserción, empaquetado, instalación, reextracción y reversión. También pasó la comparación directa japonés → español en RPG Maker.

La ejecución duró aproximadamente cinco minutos: 31 ejecuciones registradas, 316 traducciones y 178 191 tokens reportados por el proveedor. Usó un máximo configurado de diez peticiones simultáneas (cinco trabajos, dos peticiones por trabajo). Cero fallos. De las 35 advertencias, 31 indican que OAuth no informó costo completo y cuatro señalan texto idéntico. Las cuatro corresponden al nombre de lugar `Starfall Town`; el proyecto todavía debe decidir si lo conserva o localiza. No se interpreta un costo desconocido como cero. Evidencia detallada: `api-growth-matrix/REPORT.md`, `report.json` y `quality-summary.json`.

Rule95 Patcher también instaló y revirtió los 25 ZIP de destino mediante su CLI independiente. Se compararon hashes del manifiesto y del árbol restaurado, incluyendo modo copia y conservación de directorios vacíos. Se comprobó que una verificación incompatible y una reversión que conserva archivos añadidos editados devuelven códigos de error; la reversión forzada explícita completa devuelve éxito. Evidencia: `rule95-artifact-debug-v2/report.json`.

La auditoría de fuentes se ejecutó además contra seis archivos TTF/TTC instalados, sin copiarlos ni modificarlos, usando los 115 textos distintos de la matriz. Los resultados distinguen las caras de cada colección y los glifos ausentes; no sustituyen una comprobación visual dentro del juego. Evidencia: `system-font-audit/report.json`.

## Límites reales

Actualización 2026-09-17: ya se conservan los borradores al cerrar el panel, cambiar de fila y navegar entre páginas, separados por proyecto/entrada. Véase `CONTINUATION-2026-09-17.md`. Son borradores de sesión; cerrar o recargar la aplicación todavía requiere guardar antes.

No hay soporte general para cifrado, firmas, Oodle, índices PAK congelados o sin directorios, ni texto Zen arbitrario embebido en IoStore. La cobertura de fuentes no modifica atlas TextMeshPro ni garantiza ajuste de línea, composición RTL o calidad lingüística. Los formatos experimentales siguen siendo experimentales: pasar fixtures y comparar bytes no certifica todos los juegos ni su montaje de overlays.

Las pruebas de esta tanda trabajan sobre fixtures propios y copias. El control de ventanas se detuvo al recibirse Escape; las comprobaciones posteriores de interfaz usan un navegador sin ventana, sin controlar el escritorio del usuario.

## Protección final de concurrencia

Antes de guardar resultados automáticos, una transacción compara el original semántico, traducción, estado y proveedor con la instantánea enviada. Un cambio o eliminación concurrente rechaza todo el lote; no publica éxitos ni llena la memoria con respuestas rechazadas. Conserva tokens/costo observados y evita reenviar silenciosamente mediante fallback. Incluye grupos TextAsset, memoria y glosario; nueve pruebas con dos conexiones SQLite reales pasan.

Un refresco tardío ya no pisa una edición local sin guardar. Una respuesta tardía de cancelar una tarea terminada tampoco modifica la siguiente. Nueve pruebas headless de estas carreras pasan; cuatro fallaban antes del arreglo. La interfaz muestra errores de guardado y espera a que se guarde el borrador antes de cambiar su estado a revisado/aprobado.

## Verificación de cierre

- Workspace integrado: 1 239 pruebas Rust pasan, cero fallos, 14 ignoradas; Clippy de todo el workspace/all-targets con -D warnings y cargo fmt --check pasan.
- Interfaz: 28 archivos de pruebas unitarias pasan; TypeScript y Vite producción pasan (principal 429,63 kB, Editor 145,04 kB).
- CLI release final: 25/25 replays de cinco motores por cinco idiomas, 255 traducciones exactas y rollback íntegro, cero API. Evidencia final-replay-release/REPORT.md.
- Rule95: 11 pruebas pasan, Clippy estricto y release pasan. El CLI release congelado aplicó/revirtió 25/25 ZIP reales de Locust, incluyendo modo copia, hashes, directorios vacíos y códigos de error/force. Evidencia rule95-artifact-release/results/report.json.
- UI producción contra backend release final: fuentes/font-ui-qa/run-1789192829540 y stale/stale-ui-qa/run-1789192832460 pasan, cero errores JavaScript. Los procesos backend QA se cerraron.
- Hashes exactos de ejecutables: final-artifact-sha256.json. Build escritorio termina en final-desktop-release.log; comprobar su estado final indicado abajo.

Cierre completado por petición anticipada del usuario: CLI, escritorio Locust, Rule95 Patcher y apply_once compilaron en release. Las cuatro huellas SHA256 están en final-artifact-sha256.json. Todos los checks finales mencionados arriba terminaron satisfactoriamente. No quedan agentes ni servidores QA propios trabajando; no continuar hasta las 01:00 por la instrucción anterior.

Ejecutables: C:/Projects/Locust/target/release/locust-desktop.exe; C:/Projects/Locust/target/release/locust.exe; C:/Projects/rule95-patcher/target/release/rule95-patcher.exe; C:/Projects/rule95-patcher/target/release/apply_once.exe. Son builds locales; no se generó una publicación ni se certificó el arranque visual nativo en esta tanda.
