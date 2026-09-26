# Revisión de Locust, Rule95 y Rule95 Patcher

Actualización: después de esta lectura se implementaron y verificaron correcciones. El resultado actual está en [ENGINE-HARDENING-2026-09-11.md](ENGINE-HARDENING-2026-09-11.md); los hallazgos de este informe describen el estado anterior.

Fecha: 11 de septiembre de 2026. Alcance: lectura del código actual, recuperación del historial de Claude y comprobaciones locales para preparar la continuación. Prioridad actual del usuario: traducción rápida por API, especialmente xAI/Grok; modelos locales quedan para después.

## Estado de los tres proyectos

| Proyecto | Función | Estado observado |
|---|---|---|
| `C:/Projects/Locust` | Extraer → SQLite → traducir → validar → inyectar → empaquetar | Implementación amplia en Rust, CLI, servidor Axum y escritorio Tauri/React. Rama `feat/desktop-ux-kimi-k3-p2`, HEAD `c71d7e7`. |
| `C:/Projects/rule95-patcher` | Instalar y deshacer parches en el equipo del jugador | Ya consume `locust_core::patch`, con verificación, backups, recibos, streaming, copia opcional y progreso. HEAD `dc74ffd`; checkout limpio al inicio. |
| `C:/Projects/rule95` | Catálogo bilingüe de parches | Astro estático, contenido Markdown, etiquetas, buscador Pagefind y sitemap. Hay dos fichas de ejemplo, EN/ES del mismo juego, con URLs `example.com`. No hay repositorio Git local. |

Locust registra 14 plugins. RPG Maker MV/MZ, XP/VX Ace y Ren'Py están clasificados como estables; Unity, Unreal y los demás formatos indicados en README siguen experimentales. Que haya un extractor o pruebas sintéticas no demuestra compatibilidad con cualquier juego de ese motor.

El motor ya tiene lotes concurrentes, protección de variables, glosario, memoria de traducción, guardado incremental, reintentos, límites por proveedor, revisión y exportación PO/XLIFF. El objetivo es cerrar fallos y hacer accesible la ruta de API existente, sin reconstruir estas funciones.

## Qué recuperé de Claude

Directorio: `C:/Users/Mike/.claude/projects/C--Projects-Locust/`. Se localizaron cinco archivos principales de conversación, dos memorias de proyecto y el índice. `history.jsonl` contiene 419 entradas asociadas a estos proyectos, repartidas entre 30 identificadores de sesión, del 15 de marzo al 24 de agosto. Ese índice no equivale a disponer de las 30 conversaciones completas.

- `004f0754-2d68-438e-99f7-7334a1977ba6.jsonl`, 11–12 de agosto: revisión de UX y diagnóstico del parcheador antiguo. Su conclusión de que el parcheador no usaba el motor de Locust quedó superada por el código actual.
- `2e9db382-60bc-4c5e-a502-52f5ab08cec9.jsonl`, 13 de agosto y madrugada UTC del 14: correcciones de cola, cancelación, WebSockets, modales y clippy. Contiene limitaciones históricas de los agentes; no deben asumirse vigentes sin comprobarlas.
- `54315e4e-0bfb-4a9a-8093-758df6a3f301.jsonl`, 14–15 de agosto: auditoría del motor y ciclo de mejoras. El usuario había pospuesto la web. El registro `docs/IMPROVEMENT-GOAL.md` llega a 30 ciclos y el código/commits confirman avances posteriores a los últimos mensajes resumidos de esa conversación.
- `b93640bc-ee2e-4f15-ad13-65e626b73fc5.jsonl`, 23–24 de agosto: traducción de Sunkissed, BOXMAN, Out of Touch, CCTV y CCTV USSR; implementación UnityFS y límites de caracteres.
- `2220864c-a092-478c-8550-04346263c992.jsonl`: sesión asociada a Cursor, con poco contenido narrativo útil.

**Corrección histórica importante:** Claude primero dijo que unos 4.500 textos saltados no cabían. Después contrastó los datos y corrigió el diagnóstico: eran principalmente traducciones idénticas al original. No hay fundamento para volver a traducirlos todos por aquel diagnóstico inicial.

Claude informó de extract/inject/re-extract correctos de UnityFS, pero dejó pendiente arrancar los CCTV con el bundle recomprimido. Su frase final «5 juegos jugables» excede esa evidencia: esta revisión no ha arrancado los juegos ni verificado su jugabilidad.

La memoria `translation-provider-grok.md` conserva la preferencia anterior de usar `grok-sub` y nunca Google. La petición actual prioriza API: se debe distinguir `grok` (clave xAI), `grok-sub` (OAuth de suscripción) y los agentes de desarrollo de Cursor. No son la misma ruta de autenticación ni de gasto.

La memoria `project_next_steps.md` está desactualizada frente al código. El archivo `LOCalization Universal Scripting Tool.txt` está vacío; la información útil de diseño está en README, CLAUDE.md, los documentos de `docs/` y las sesiones.

## Cambios existentes que deben conservarse

En Locust ya había cambios en `Cargo.lock`, `crates/formats/Cargo.toml`, `crates/formats/src/lib.rs` y `crates/formats/src/unity.rs`, además de `crates/formats/src/unity_fs.rs` sin seguimiento y `target-unityfs/` sin seguimiento. Son coherentes con la última sesión de Claude. No se modificaron, revirtieron ni confirmaron en Git durante esta revisión.

UnityFS incluye lectura de bloques y reescritura conservando el tamaño de los nodos; no implementa una reestructuración general de objetos para permitir traducciones arbitrariamente largas. Conviene cerrar su revisión y la prueba real antes de ampliar soporte.

## Prioridades para continuar

### 1. Hacer fiable y configurable Grok por API

- `crates/providers/src/lib.rs:57`: el proveedor `grok` sigue usando `grok-4-1-fast` por defecto. La documentación oficial actual identifica `grok-4.6` como modelo disponible; no cambiar silenciosamente claves o configuración existente.
- `apps/desktop/src/pages/Settings.tsx:560`: el selector de modelos se muestra para OpenAI y Claude, no para Grok. El backend acepta modelo/base URL configurables, pero esa capacidad no está expuesta para Grok en la interfaz.
- `crates/providers/src/openai.rs:67`: los proveedores compatibles, incluido Grok, tienen `pricing: None`. `estimate_cost` devuelve `None` y no se informa coste por respuesta. En `crates/core/src/translation.rs:611` el control de presupuesto solo se aplica cuando hay estimación: un límite configurado no garantiza detener el gasto de Grok. Además, `cost_limit_usd` reduce la concurrencia a uno incluso en este caso.

Aceptación: configurar y probar el modelo Grok desde escritorio; preservar claves ocultas; mostrar coste desconocido como desconocido; no prometer un límite que no se puede aplicar; pruebas de presupuesto incluyendo reintentos y llamadas pendientes. La tarifa y capacidades deben asociarse al modelo, no a la marca del proveedor.

### 2. Corregir los lotes antes de subir la concurrencia

- `crates/core/src/translation.rs:636` construye contexto, presupuesto binario y glosario por entrada, pero `crates/providers/src/openai.rs:177` usa únicamente `requests[0]` para el prompt y envía una lista de textos sin los datos de las restantes entradas. Resultado: las instrucciones particulares de una cadena pueden aplicarse al lote y perderse las de las demás.
- El cliente pide un array JSON ordenado y rechaza una cantidad distinta (`openai.rs:238`). Es mejor que desplazar traducciones, pero aún depende del orden devuelto; una respuesta mal formada hace fallar el lote completo.
- `crates/core/src/translation.rs:181`: el deadline se comprueba entre intentos; la llamada `operation().await` no está envuelta en timeout/cancelación. El cliente compatible se crea con `reqwest::Client::new()`. Una petición atascada puede impedir que el job termine o cancele a tiempo. El rate limiter se adquiere fuera del bucle de reintentos.

Aceptación: conservar el contexto y límite de cada entrada con IDs explícitos; validar IDs, cantidad y placeholders; reintentar de forma acotada solo lo fallido; cancelar una petición bloqueada de manera verificable; registrar consumo incluso en terminaciones parciales. Usar un servidor HTTP simulado, sin gastar API real para estas regresiones.

### 3. Medir velocidad con un perfil de API

Hoy el lote por defecto es de 40 cadenas, la interfaz usa tres lotes concurrentes y el núcleo uno si no se indica lo contrario. El límite interno por defecto para Grok es 90 peticiones/minuto, con override en `ProviderConfig.extra.requests_per_minute`. No equivale al límite real de la cuenta xAI y no modela tokens/minuto.

Propuesta: probar 1/3/6/10 llamadas concurrentes sobre el mismo corpus autorizado, medir cadenas/minuto, tokens, latencia, fallos y calidad; ajustar lotes por tokens y restricciones en lugar de solo por cantidad. No afirmar que 40 × 10 es óptimo sin medir. Mantener glosario/memoria, pero auditar su efecto sobre el contexto y los presupuestos binarios.

La Batch API asíncrona del proveedor es diferente de estos lotes interactivos; su compatibilidad depende del modelo. No adoptarla como supuesto atajo para la experiencia rápida del escritorio.

### 4. Cerrar Unity y el parcheador

- Separar en informes/UI «sin cambios», «excede longitud», «destino no encontrado» y otros errores. `InjectionReport` mantiene un `strings_skipped` agregado (`crates/core/src/extraction.rs:100`); el código Unity ya cuenta algunos motivos internamente, pero no los expone como contrato uniforme.
- Validar UnityFS en una copia de juego real y verificar arranque/carga/guardado, además de extract/inject/re-extract.
- Rule95 Patcher ya está integrado con el motor seguro. El escáner y DLsite existen, pero no están conectados a la UI. La interfaz promete legacy mediante «Forzar», pero no envía `confirm_legacy` en `ui/main.js:308`; el backend lo recibe y lo pasa a apply. Debe exponerse el flujo explícito o corregirse la promesa.

### 5. Completar el contrato con Rule95 después

`locust patch --astro` **ya existe** (`crates/cli/src/main.rs:142`, `write_astro_stub:604`), aunque el README de Rule95 lo describe como futuro. Genera un borrador manual, no una ficha lista para publicar: deja URLs vacías, fecha TODO, autor TODO y usa IDs de motor que no siempre encajan en `src/content.config.ts` (por ejemplo `rpgmaker-vxa` frente a `rpgmaker-vxace`). También marca `complete` sin consultar cobertura.

Aceptación futura: exportación que valide contra el esquema real, mapeo explícito de motores, metadatos/fecha válidos, estado de traducción real y comprobación de enlaces antes de publicar. Hay planes de despliegue y migración en `rule95/deploy/`; no se comprobó infraestructura remota ni se desplegó nada.

## Comprobaciones de esta revisión

| Comando | Resultado |
|---|---|
| `cargo test --offline -p locust-core -p locust-providers -p locust-formats --lib` | Exit 0. Núcleo: 247 pruebas; formatos: 354 aprobadas y 1 ignorada; proveedores: 52 aprobadas. |
| `npm run build`, Locust desktop | Exit 0; advertencias de tamaño del bundle, import mixto y datos Browserslist antiguos. |
| `cargo test --offline --lib`, Rule95 Patcher | Exit 0; 4 pruebas aprobadas, incluyendo apply/rollback y copia sin alterar el original. |
| `npm run build`, Rule95 | Exit 0; genera rutas y buscador EN/ES sobre las dos fichas de ejemplo. |
| `npm run test:unit`, Locust desktop | Exit 0; los 26 archivos de pruebas terminaron con resultado correcto. |

No se ejecutaron llamadas de traducción pagadas, pruebas de juego real, instalación de paquetes de escritorio ni la suite completa del workspace. Las compilaciones web son comprobaciones de build, no una revisión visual.

## Cursor y continuación

Preflight realizado: Cursor CLI `2026.09.10-fd3934a`, sesión de cuenta autenticada, modelos `cursor-grok-4.6-high` y `gpt-5.6-sol-high` presentes, sin overrides de API/proveedor detectados por nombre de variable. El coste/cuota restante no se obtuvo del CLI. No se extrajeron credenciales.

Dos encargos de lectura preparados: Grok 4.6 High para traducción/API y Sol High para el contrato entre aplicaciones. No se han lanzado: está pendiente la confirmación del usuario de que on-demand está desactivado, exigida por la skill `delegar-codigo-cuotas`. El análisis de este informe es del coordinador, no una revisión realizada por agentes de Cursor.

Siguiente paso concreto: ejecutar esas revisiones, contrastarlas con los hallazgos y abordar primero configuración/coste/cancelación y contexto por entrada. Mantener el trabajo de UnityFS y aplazar modelos locales hasta tener una ruta de API fiable y medida.

## Referencias externas consultadas

- [Grok 4.6: modelo y APIs admitidas](https://docs.x.ai/developers/grok-4-6).
- [Salidas estructuradas](https://docs.x.ai/developers/model-capabilities/text/structured-outputs).
- [Límites de peticiones y tokens](https://docs.x.ai/developers/rate-limits).
- [Batch API](https://docs.x.ai/developers/advanced-api-usage/batch-api).

Los valores de la cuenta y las capacidades del modelo se deben verificar al implementar; los nombres del catálogo de Cursor no se deben copiar como IDs de la API xAI.
