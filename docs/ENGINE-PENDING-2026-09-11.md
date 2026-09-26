# Pendientes del motor: continuación con Cursor Grok 4.6

**Registro histórico.** La continuación con Astra resolvió varias limitaciones descritas aquí, incluidas lectura de PAK modernos, IoStore ExternalFile, crecimiento de TextAsset y duplicación de originales. Consulta el [estado actual](ENGINE-ASTRA-2026-09-12.md); conserva los resultados de esta página asociados únicamente a sus ejecutables y fechas.

Esta tanda corrige el presupuesto compartido de Unity, los fragmentos de SugarCube, la lectura de PAK clásicos y el registro de idiomas de RPG Maker. Las comprobaciones y limitaciones se detallan abajo. Complementa la [matriz anterior](ENGINE-MATRIX-2026-09-11.md); los resultados antiguos no se atribuyen al ejecutable nuevo.

## Cambios y comprobaciones

### RPG Maker: registro de idiomas

Se corrigió el respaldo de mapas para guardar los bytes anteriores a la primera modificación. Un fallo de respaldo impide escribir; registrar otro idioma conserva el primer respaldo. Las ramas de opciones se identifican por profundidad, y las asignaciones de idioma se cambian sin alterar instrucciones adyacentes, comentarios, cadenas ni `ConfigManager.language` al buscar `ConfigManager.lang`. Hay 13 pruebas específicas correctas.

Se ejecutó MZ 1.4.4 con sus bibliotecas originales y mapas/datos de QA nuevos, sin historia del juego fuente. Cinco copias traducidas a ES/FR/DE/PT-BR/ZH-CN pasaron título, opciones, partida nueva, menú, guardado y carga: una variable cambió y recuperó su valor guardado. Se reutilizaron 12 traducciones Grok previas por idioma, cotejando identificador y fuente; no son llamadas nuevas ni traducciones completas del juego.

Un segundo escenario usa el plugin Iavra original y un selector creado en un evento de mapa. `register-lang` agregó los cinco destinos a japonés/inglés, mantuvo las instrucciones de persistencia y guardó un respaldo idéntico al mapa inicial. El selector funcionó en los cinco idiomas y conservó chino después de recargar. Los dos textos por idioma de este escenario son referencias manuales. Se usó `?test`, ruta de carga JSON que admite el plugin; su distribución cifrada y los menús VisuMZ del juego original no están certificados.

Evidencia: `tmp/pending-grok-20260911/runtime/mz-runtime-results.json`, `iavra-register-fixed-results.json`, `iavra-map-runtime-results.json` y capturas adyacentes.

### Unreal: lectura acotada y prioridad

El lector consulta por posición el índice y los recursos LocRes de PAK clásicos v3–v8, con límites de tamaño, offsets y verificaciones SHA-1. Las identidades usan ruta virtual normalizada, espacio de nombres y clave. Recursos y culturas diferentes permanecen separados. Las versiones de un mismo recurso se reemplazan enteras según la prioridad convencional; `_10_P` gana a `_2_P`. Un recurso comprimido que gana la prioridad produce un error en lugar de recuperar silenciosamente su versión antigua. Pasaron 48 pruebas unitarias y 15 estructurales específicas.

**Cambio de compatibilidad:** se rechazan índices v9–v11, incluido el PAK de actualización de Last Hope de 1,35 MB que la versión anterior recorría heurísticamente y su base de 8,95 GB. Los resultados históricos de ese PAK no validan esta versión. Siguen pendientes los índices modernos, LocRes comprimidos, cifrado, firmas e IoStore. La prioridad `LOCUST_P` es una política de Locust, no una certificación del orden de montaje de cada juego.

### Unity: presupuesto compartido

La traducción comprueba la reconstrucción del TextAsset completo para líneas de localización y CSV simple, conservando la fuente física japonesa al pivotar desde inglés. Una celda puede crecer cuando el bloque todavía cabe. Si no cabe, se permiten hasta dos rondas de traducción concisa, respetando el tamaño de lote y el presupuesto aproximado de tokens de cada petición de reintento. Las celdas seleccionadas se guardan después de comprobar el bloque, sin truncar palabras ni ampliar el espacio binario. Las que siguen sin caber quedan pendientes.

Se verificaron selección parcial, grupos repartidos entre lotes, cancelación durante un reintento, conservación del costo observado y aislamiento entre grupos que fallan y otros que sí caben. La búsqueda de celdas de un mismo bloque usa un índice; después de cada lote solo se revisan los grupos afectados. La memoria y el glosario no pueden guardar una celda agrupada como válida antes de comprobar el bloque.

El modo automático admite como máximo **128 miembros, 32 KiB de texto original y 1 MiB de original duplicado por grupo**. Fuera de esos límites conserva metadatos ligeros y da un diagnóstico antes de solicitar traducciones. Los grupos válidos todavía duplican el original hasta ese límite; no se afirma que exista un almacenamiento compartido sin duplicación. Las bases antiguas sin los nuevos metadatos necesitan reextracción. CSV con comillas/comas internas y otros diseños no soportados requieren tratamiento específico.

La matriz nueva **JA → EN → ES/FR/DE/PT-BR/ZH-CN pasó 5/5 destinos**, incluyendo inserción, empaquetado, aplicación, reextracción exacta de traducciones y reversión por hashes. Fueron 60 resultados nuevos de Grok y 54 473 tokens reportados, con hasta dos peticiones concurrentes. El reintento automático se verificó con respuestas controladas; esta matriz API no necesitó una corrección manual de traducciones. Evidencia: `tmp/pending-grok-20260911/unity-api-v2c/report.json`. La matriz congeló su ejecutable antes de las últimas restricciones de metadatos y la optimización de grupos; **los cinco artefactos pasaron nuevamente con el release final**, incluyendo validación, inserción, empaquetado, instalación, reextracción y reversión (`unity-final-replay/results.json`), sin llamadas API adicionales.

### SugarCube: fragmentos y enlaces

Se implementó extracción e inserción por fragmentos de filas con estilos, condiciones y enlaces. Las comprobaciones de fuente y estructura impiden sobrescribir un destino o una condición cambiados. El navegador detectó un fallo en una versión intermedia: los enlaces mostraban entidades HTML literalmente. Se corrigió usando cadenas JSON constantes para las etiquetas de enlaces y conservando el escape de texto normal. No se evalúan expresiones arbitrarias procedentes de la traducción; las etiquetas dinámicas no soportadas se omiten.

Pasaron 23 pruebas unitarias y 20 de fragmentos, además de tres recorridos de navegador en SugarCube 2.37.3: español, chino y texto literal con comillas, barras, flechas y secuencias parecidas a macros/enlaces. El texto visible coincide exactamente, las variables y destinos se conservan, y navegación y guardado/carga pasan. Los 18 fragmentos modificados se recuperaron con el mismo identificador y texto al reextraer; los tres parches se instalaron y revirtieron con coincidencia exacta de hashes. Son referencias manuales de QA. La puntuación aislada y los textos internos de macros no soportadas permanecen sin traducir.

## Pruebas de ejecución y límites visuales

- **Ren’Py:** bibliotecas originales con una pantalla neutral nueva. Se insertaron seis etiquetas españolas de referencia manual. En la aplicación nativa se guardó el contador 7, se cambió a 99 y se cargó de vuelta a 7. Carpeta de guardado exclusiva de QA. Esto no valida las escenas del juego original.
- **Unity real:** copia completa de CCTV USSR, parche chino previo y comparación con copia restaurada. El log confirma dos glifos chinos ausentes en `AlegreyaSC-Bold`, sustituidos por cuadrados. Las referencias de cámara nulas, scripts ausentes y avisos de serialización aparecen también sin parche; las duraciones distintas impiden comparar los recuentos como regresión. Se revirtió la copia y se detuvo la ejecución en la pantalla de confirmación de edad. No se certifican escenas ni guardado/carga.
- **Fuentes:** MZ renderizó una muestra CJK/latina/árabe/hebrea en `Window_Base`. La línea larga se recortó. No se certifican cobertura tipográfica universal, orden hebreo, adaptación completa RTL ni anchuras automáticas.
- **SugarCube neutral:** motor original 2.37.3 con dos pasajes nuevos. Navegación, condicional de monedas, texto literal y guardado/carga pasan en los tres escenarios finales. Evidencia en `runtime/sugarcube-final-*-runtime.json` y capturas adyacentes.

La guía de control de escritorio impide enviar confirmaciones de edad: [guidance.md](C:/Users/Mike/.codex/plugins/cache/openai-bundled/computer-use/26.903.71938/docs/guidance.md), «Do not submit age verification». La prueba de Unity se detuvo en ese punto; no es un fallo resuelto del motor de traducción.

## Proveedor, estado y reproducibilidad

Todos los trabajadores de esta tanda usan **Cursor CLI, `cursor-grok-4.6-high`**, reportado como `Cursor Grok 4.6 High`, autenticación `login`. La implementación se delegó en capturas aisladas de la base con cambios previos, registradas por SHA-256. El coordinador realizó integración, QA, correcciones de compilación/codificación y las correcciones finales de lotes, cancelación e índices de Unity con sus pruebas. Una sesión de Unity se interrumpió después de errores repetidos de argumentos de edición; se conservó el trabajo y se terminó con encargos acotados en Grok. No se cambió facturación ni se usó Sol en esta tanda. Los costos monetarios y los porcentajes de cuota de Cursor y xAI son desconocidos.

El workspace de Rust, excluyendo el paquete de escritorio, pasó **973 pruebas, 0 fallidas y 9 ignoradas**. Después de ampliar las comprobaciones de metadatos se volvieron a ejecutar sus 12 pruebas, todas correctas. `cargo fmt --check` y Clippy con `-D warnings` también pasaron. La interfaz pasó sus 26 archivos de pruebas y la compilación TypeScript/Vite; conserva el aviso de un fragmento JavaScript de más de 500 kB. Los nueve archivos originales auditados conservan sus SHA-256 (`tmp/pending-grok-20260911/originals-verification.json`). No se recompiló Rule95 Patcher ni se hicieron commits.

Las órdenes, sesiones, manifiestos de integración y logs se conservan en `tmp/pending-grok-20260911`. Las capturas y escenarios de ejecución están en `runtime`. Los fallos intermedios se conservaron; un archivo de resultados anterior no implica aprobación de una revisión posterior.

## Ejecutables finales

La compilación release de ambos ejecutables pasó (`tmp/pending-grok-20260911/final-release-build.log`). No se abrió la aplicación de escritorio recién compilada para una prueba visual.

- [Project Locust](C:/Projects/Locust/target/release/locust-desktop.exe): SHA-256 `de1e107755ba69ec00d90d33744426c5fd2e57c3004a5d1eb8cb5bea743c91ea`.
- [CLI Locust](C:/Projects/Locust/target/release/locust.exe): SHA-256 `b2a0e0da9e4b88fdceaab03cf3a113cd2ec278d1b98350f5c4bd03bd328e4840`.

Siguen pendientes la adaptación de fuentes y espacios visuales por juego, los PAK modernos de Unreal y la comprobación de escenas/guardados de juegos completos fuera de los escenarios neutrales descritos. No se afirma ausencia universal de bugs ni compatibilidad con todos los títulos/versiones.
