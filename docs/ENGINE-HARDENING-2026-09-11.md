Actualización: véase [la continuación y validación posterior](CONTINUATION-2026-09-11.md). Los resultados de este documento corresponden a la primera pasada.

# Motor de traducción y parches: cambios verificados

Fecha: 2026-09-11. Proyectos: Locust y Rule95 Patcher. Complementa `PROJECT-REVIEW-2026-09-11.md`, que describe el estado anterior. Rule95 web no se modificó ni se publicó.

## Resultado

Se comprobó el recorrido extracción → Grok OAuth → validación → inyección → ZIP estricto → instalación → restauración. La copia instalada coincide por SHA-256 con la traducción y la restauración coincide con el original. Es una prueba funcional con un pequeño juego HTML de laboratorio, no una certificación de todos los motores ni un benchmark representativo.

El OAuth guardado de Locust funciona con `grok-4.6`. No fue necesario introducir otra clave. La autenticación del CLI Grok Build es una ruta distinta y no estuvo disponible.

## Correcciones integradas

### Traducción y API

- Timeout y cancelación interrumpen una petición pendiente. La espera inicial de rate limit queda fuera del tiempo de HTTP; los reintentos también adquieren permiso del limitador.
- OpenAI compatible y Claude reciben contexto y glosario por entrada, conservando la respuesta como array ordenado. Se rechazan lotes con idiomas mezclados y resultados internos con IDs duplicados o ajenos. La identidad de la respuesta HTTP sigue dependiendo del orden del array; no se implementó un protocolo nuevo de IDs devueltos por el modelo.
- Se corrige un posible panic al interpretar corchetes mal ordenados y la duplicación de `/v1` en URLs compatibles, incluida la ruta de Gemini.
- La memoria usa el hash del texto original con sus variables; antes podía almacenar bajo el texto sanitizado y fallar al reutilizarlo. Antes de reutilizar valida variables y presupuesto binario.
- Se validan las variables después de restaurarlas y de ajustar longitud. Las respuestas inválidas quedan pendientes, sin contaminar la memoria.
- El guardado de cada lote es una transacción SQLite. Si desaparece alguna entrada, falla el lote completo, evitando persistencia parcial.
- El historial conserva consumo observado aunque se rechace una respuesta o falle su guardado, incluyendo proveedores que reportan dinero sin tokens. Los reintentos de longitud acumulan tokens de entrada/salida.
- Un límite de gasto con un proveedor sin estimación devuelve un error explícito antes de enviar solicitudes. Para proveedores con precios estimados, sigue siendo un control orientativo: no garantiza un tope exacto de facturación de todos los reintentos.
- Cliente HTTP reutilizado en OAuth para aprovechar conexiones; renovación serializada con nueva lectura de tokens para evitar renovaciones concurrentes. Renovación y health check tienen timeout.
- Grok y Grok OAuth usan `grok-4.6` por defecto y permiten editar el modelo desde Ajustes. Una configuración explícita existente se conserva.
- Se rechazan tamaño de lote cero y presupuestos negativos/no finitos.

### Instalación y recuperación

- Un dry-run que necesita deshacer un parche anterior devuelve un error explícito y conserva los archivos y metadatos. Antes podía ejecutar rollback durante la simulación. La simulación virtual de esa transición sigue pendiente.
- Se rechazan enlaces/reparse points interiores hacia otros destinos y entradas reservadas `.locust` dentro del ZIP. Se revisan también respaldos y destinos de rollback antes de modificar archivos.
- La carpeta raíz elegida puede ser una junction legítima: se resuelve primero. No se afirma protección frente a otro proceso que cambie enlaces entre comprobación y escritura.
- Se conserva el atributo oculto de `.locust` al trabajar con rutas canonicalizadas de Windows, sin abrir una ventana de consola.
- En Rule95 Patcher, «Deshacer» usa el destino realmente instalado tras crear una copia. El modo copia rechaza destinos dentro del propio juego y no copia archivos durante dry-run.
- Los parches antiguos tienen una aceptación explícita, que se envía al backend y se reinicia al elegir otro parche. Una instalación activa impide sustituir el parche mediante arrastre.

Optimizaciones concretas: conexiones OAuth reutilizadas, una transacción por lote y caché que vuelve a acertar con variables. No se asigna un porcentaje de aceleración sin medirlo.

Se preservó el trabajo previo de UnityFS. Solo se ajustaron referencias redundantes, el argumento de un buffer a slice y un alias de tipo para superar Clippy; no se reescribió esa implementación. En Patcher se normalizó además el formato Rust existente.

## Verificación

| Comprobación | Resultado |
|---|---|
| `cargo test --offline --workspace --quiet`, Locust | 835 aprobadas, 0 fallidas, 9 omitidas |
| `cargo clippy --offline --workspace --all-targets -- -D warnings`, Locust | Correcto |
| `cargo fmt --all --check`, Locust | Correcto |
| `npm run build`, escritorio | Correcto; advertencias existentes de bundle/import mixto/Browserslist |
| `npm run test:unit`, escritorio | Correctos los 26 archivos de pruebas |
| `cargo test --offline --lib --quiet`, Patcher | 6 aprobadas |
| `cargo fmt --all --check` y `cargo clippy --offline --all-targets -- -D warnings`, Patcher | Correctos; excepción documentada para los argumentos públicos del comando Tauri |
| `node --test tests/ui.test.cjs`, Patcher | 3 aprobadas |
| Revisión visual en navegador local | Ajustes muestra modelo Grok API/OAuth; instalador muestra aceptación legacy y controles sin solapamiento en el viewport observado |

Los tests nuevos cubren petición bloqueada, cancelación, cola a 1 RPM, memoria con variables/presupuesto binario, resultados inválidos, consumo sin tokens, transacciones fallidas, dry-run de cuatro transiciones, junctions reales en Windows y estado UI de copia/legacy/arrastre. La revisión visual del Patcher usa su modo preview; las pruebas de Rust/CLI verifican las escrituras reales. No se generó ni instaló un paquete de distribución nuevo.

### Prueba real xAI

Artefactos: `tmp/api-patch-smoke-d5904713/` (ignorado por Git). `result.json` conserva hashes y resultado; `es-patch.zip` es el parche del fixture.

- 12 entradas, inglés → español, Grok 4.6 mediante `grok-sub`.
- 3 lotes concurrentes de 4 entradas; aproximadamente 15 segundos, sin errores de lote.
- Uso comunicado: 5.915 tokens totales, 3.392 de entrada y 107 de salida. Se conserva el total del proveedor aunque no coincida con la suma de esas dos categorías.
- Coste real desconocido. El `$0.0000` del CLI refleja un dato no disponible; no demuestra gratuidad.
- Validación: sin incidencias; `[player_name]` y `{0}` conservados.
- Inyección: 11 escritas / 1 omitida en el contador actual. La fuente duplicada «The Lighthouse» se sustituyó en título y encabezado con la primera sustitución global; la segunda entrada se contó como omitida. Ambas apariciones están traducidas. Esto demuestra la necesidad de mejorar el contrato del informe y el reemplazo por ubicación en HTML.
- Instalación estricta y rollback: correctos, verificados mediante hashes.

## Qué falta, por prioridad

1. **Pruebas por juego real y motor.** Unity/UnityFS necesita comprobar arranque, escenas, fuentes, partidas y restauración de una copia. Un extract/inject correcto no demuestra que el juego cargue todos sus assets. Mantener una matriz de formatos y versiones comprobadas.
2. **Inyección identificada por ubicación e informes claros.** Exponer motivos distintos para duplicados, texto idéntico, espacio insuficiente, destino ausente y error. En HTML, sustituir por ubicación/nodo en vez de reemplazar globalmente por contenido; revisar selectores y contenido embebido. Evitar presentar toda traducción almacenada como cobertura instalada.
3. **Calidad binaria.** El ajuste mecánico de textos a slots puede reducir legibilidad o significado; requiere revisión. Donde sea viable, migrar a reserialización/relocación del formato para admitir textos más largos.
4. **Coste y rendimiento.** Mostrar «desconocido» de extremo a extremo en historial/CLI/UI, resolver precios/capacidades por modelo y contabilizar también respuestas HTTP mal formadas cuando exista uso recuperable. Medir corpus reales con concurrencias 1/3/6/10, límites por tokens y lotes adaptativos. Dividir/reintentar lotes inválidos sin repetir los ya aceptados.
5. **Recuperación y distribución.** Simulación completa de upgrades sin rollback real, exclusión entre procesos de instalación, más pruebas de cierres durante escritura, paquetes de escritorio reproducibles y prueba del instalador empaquetado.
6. **Rule95.** Validar la exportación Astro contra su esquema, mapear motores, reflejar cobertura real y completar enlaces/metadatos. El escáner/DLsite del Patcher aún necesita conectarse a la UI. Modelos locales quedan fuera de esta fase por indicación del usuario.

## Agentes y continuidad

Implementación e integración realizadas por el coordinador, con revisión independiente mediante un subagente nativo. Cursor no generó estos cambios: faltaba evidencia de on-demand desactivado para esa ruta, exigida por la skill de delegación. Grok Build no estaba autenticado; la autorización posterior de traducciones xAI sí se utilizó en la ruta OAuth de Locust. No se abrió una ruta de facturación alternativa para agentes.

Los servidores temporales de QA se cerraron. No se hicieron commits, push, despliegues ni cambios a juegos del usuario. El diff queda listo para revisión y continuación.
