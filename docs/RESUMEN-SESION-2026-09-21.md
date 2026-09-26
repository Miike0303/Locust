# Cierre de sesión — Locust y Rule95

Fecha UTC: 2026-09-22T03:48:13.405266+00:00. El usuario pidió terminar las tareas en curso, no iniciar más trabajo y hacer un resumen. La última validación terminó; detener el goal hasta una petición explícita de reanudación. Ruta autorizada: Codex con la suscripción de ChatGPT, sin Cursor ni Grok Build.

## Resultado

- Se corrigieron la procedencia y conservación del respaldo original para empaquetar tras reiniciar y entre perfiles/CLI/app. Reinserciones idénticas conservan el original y eliminan solo el duplicado recién creado tras verificaciones bajo bloqueo.
- La revisión de traducciones ya insertadas en HTML y Ren'Py preserva otras cadenas y permite corregir A→B. Ahora consume originales con hash/tamaño inmutables; cambios, truncado o desaparición tras la verificación detienen la operación sin instalar resultados. Ren'Py analiza el texto cargado sin reabrirlo durante ese análisis.
- El empaquetado comprueba la generación seleccionada y los bytes realmente escritos al ZIP. Se corrigieron carreras entre selección, lectura y publicación; los ZIPs previos se conservan si falla una verificación.
- La interfaz diferencia una reinserción idéntica de un fallo real, conserva los avisos de fuente cambiada y no ofrece un parche nuevo sin escrituras.
- La auditoría de fuentes excluye copias internas de recuperación, permite auditarlas al seleccionarlas explícitamente y cubre también texto sin traducir, incluido el japonés físico de proyectos pivotados al inglés. Fuentes defectuosas reales, procedencia inválida y traducciones obsoletas siguen visibles.
- Se conservaron las mejoras previas de recuperación de inserciones, persistencia de borradores/proyectos y operaciones de inserción/parches. Se preservó el trabajo preexistente; no hubo commits, publicaciones, compras ni resets de cuota.

## Verificación final

- BATCH16: **1453 pruebas Rust aprobadas, 0 fallos, 17 ignoradas**, 59 suites; formato y Clippy estricto aprobados.
- Frontend de producción y release de CLI/escritorio compilados. Paridad de traducciones de interfaz EN/ES aprobada.
- CLI real: el juego con1 fuente activa pasa de2 fuentes/1 error contaminado por recuperación a1 fuente/0 errores; auditoría explícita de recuperación conserva1 fuente/1 error.
- UI real, navegador oculto: detecta3 glifos japoneses ausentes antes de traducir y0 después; solo cuenta la fuente activa y no cambia el juego. Capturas: `C:\Projects\Locust\tmp\goal-20260921\font-audit-ui-1790048800499`.
- Regresión real de interfaz:11 comprobaciones de inserción, revisión, reinicio, empaquetado, aplicación/reversión y protección de cambios externos. Capturas: `C:\Projects\Locust\tmp\goal-20260921\backup-restart-ui-1790048802573`.
- BATCH15:50 casos Direct/Replace de Unity, Unreal, RPG Maker, HTML y Ren'Py con traducciones guardadas del pipeline JA→EN→ES/FR/DE/PT-BR/ZH-CN, conservando hashes; Rule95 consumió los nuevos ZIPs en79 llamadas. Esta matriz no se repitió en BATCH16 porque no cambió el motor de parches.
- Los procesos privados de esta validación terminaron y los navegadores/backend de QA se cerraron. No se movió el cursor ni se abrieron ventanas nativas para esta tanda.

## Archivos y ejecutables

- `C:/Projects/Locust/target/release/locust.exe`
- `C:/Projects/Locust/target/release/locust-desktop.exe`
- CLI SHA256: `61384bbee73c8aa718bb539243cfd09292c8437185e6578f20878f33d8280363`
- Escritorio SHA256: `1ee3b7a94e7bb9a45f8b97648ec540bee94605b2bb9298e79e717f7e92179aca`
- Evidencia detallada: `tmp/goal-20260921/BATCH16.json`, `BATCH16-SOURCE.json`, `codex-font-recovery/verification/`, `docs/QA-2026-09-21.md` y `docs/CONTINUATION-2026-09-21.md`.

## Pendientes y límites

- No se certificó ejecución universal de juegos comerciales: siguen pendientes selección real de fuentes, shaping/RTL, ajuste visual y atlas/fallback de TextMeshPro.
- Los diarios de transacción siguen conservándose; una operación sin originales puede mostrar información de recuperación vacía. La retención de Add/Replace no recibió el cambio de Direct.
- La fuente sintética comprueba cmap y transacciones; no demuestra apariencia de glifos en el motor del juego.
- Cambio interno de API Rust en BATCH15: plugins personalizados que implementen `prepare_revision_entries` deben aceptar `RevisionOriginal` y usar `read_text()`. CLI, HTTP y ZIP conservan sus contratos.
- Las ventanas nativas ya abiertas pueden seguir ejecutando versiones anteriores; los nuevos ejecutables están compilados en las rutas indicadas.

No quedan tareas enviadas por esta sesión pendientes de ejecución. No reanudar automáticamente.
