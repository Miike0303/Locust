# Motor de traducción: verificación multilingüe — 11 de septiembre de 2026

> Informe histórico de la primera tanda. La [continuación con Astra](ENGINE-ASTRA-2026-09-12.md) registra el estado actual; la [tanda intermedia de Cursor Grok](ENGINE-PENDING-2026-09-11.md) también queda como evidencia histórica. Las pruebas siguientes corresponden únicamente a los ejecutables identificados en este informe.

El recorrido **extraer → japonés a inglés → proyecto independiente por idioma → traducir → insertar → empaquetar → aplicar → volver a extraer → revertir** funciona en las muestras comprobadas. Se probaron español, francés, alemán, portugués brasileño y chino simplificado. Esto verifica archivos y parches; no certifica el funcionamiento de todos los juegos dentro de sus motores.

## Resultados

| Motor/formato | Prueba sintética JA → EN → cinco idiomas | Archivos reales y comprobación adicional |
|---|---|---|
| Unity | Los cinco destinos completaron el recorrido. Alemán necesitó otra traducción más breve para caber en el bloque original. | UnityFS de CCTV USSR: 8/8 traducciones modificadas se recuperan en francés y chino; aplicación y reversión exactas. La novena cadena, `No`, sigue pendiente: dos bytes no permiten su equivalente chino. |
| Unreal | LocRes: cinco destinos, parches e instalación/reversión correctos. | PAK de actualización de Last Hope: 8 traducciones francesas y 10 chinas recuperadas del PAK generado. El archivo nuevo requiere el modo de verificación estructural. |
| RPG Maker MV/MZ | Eventos MV: cinco destinos y comparación JA → ES directa. | System.json japonés de MZ: JA → EN/ES/FR/DE/PT-BR/ZH y JA → EN → cinco destinos, 12 entradas por ejecución. Parches estrictos y reversión correctos. Un contenedor MV cifrado se rechazó sin modificarlo. |
| HTML | HTML genérico: cinco destinos, atributos visibles y variables conservados. | SugarCube: tres controles de navegación a español y chino. Se modifican exactamente 3 de 105 repeticiones; las otras 102 y el resto del archivo permanecen intactos. |
| Ren’Py | Scripts `.rpy`: cinco destinos. | `scripts.rpa` de Ripples: 12 traducciones chinas y 11 francesas modificadas; recuperadas al reabrir. Los scripts sueltos generados se instalan/revierten como archivos nuevos. |

Los archivos reales son subconjuntos de recursos y los textos enviados a Grok son menús/mensajes de sistema. No se tradujeron juegos completos en esta comprobación. Se verificó que los **9 archivos originales utilizados conservan sus SHA-256**.

La primera matriz registró tres fallos: capacidad insuficiente del bloque alemán de Unity, espacios de relleno de Unity y reflujo de una frase alemana de RPG Maker. Los dos últimos eran diferencias de representación que la comprobación exacta no contemplaba: se conservaron las capturas originales y se comprobó el texto con esas transformaciones específicas de cada formato. La repetición local pasó 24/25 casos; el alemán de Unity pasó después con una nueva traducción concisa de Grok, sin ampliar el bloque ni truncar palabras. Ese reintento de bloque completo todavía no es automático.

## Cambios realizados

- **Procedencia del pivot:** el inglés sigue siendo la fuente de traducción, mientras `locust_injection_source` conserva el japonés físico. Los pivots sucesivos mantienen esa referencia y la capacidad binaria original.
- **Reapertura segura:** un proyecto pivotado conserva su fuente semántica al reabrir la misma base. Una fuente física cambiada, un hash LocRes diferente o filas desaparecidas provocan rechazo antes de modificar la base de datos.
- **Límites reales:** prompts, memoria, glosario y validación comparten la capacidad original. Los bloques agrupados de Unity no reciben límites ficticios por celda. Una procedencia o capacidad incoherente produce un error de validación.
- **Un idioma por proyecto de inserción:** consola, API e interfaz impiden reutilizar una traducción como si correspondiera a varios idiomas. Se crea un proyecto/DB por destino; la interfaz permite seleccionar uno.
- **Ren’Py:** los controles de interfaz extraídos de RPA se insertan con la misma selección de literales que los scripts sueltos. Al reextraer, los archivos sueltos prevalecen sobre las filas del archivo empaquetado que comparten identificador.
- **Unity:** se recuperan japonés/chino, textos CJK largos y controles de un solo carácter en campos estructurados. Se conserva el relleno y la capacidad física del archivo.
- **RPG Maker y Unreal:** verificación del texto actual antes de escribir; comprobación del hash de fuente LocRes; recuentos de cambios y omisiones corregidos.
- **SugarCube:** inserción por pasaje y fila seleccionados, con posiciones de texto visibles y fuente exacta; se conservan macros, atributos, destinos, entidades y bytes ajenos. Se reconocen delimitadores dentro de comillas. Se omiten las filas cuyo texto está repartido entre elementos para evitar vaciar enlaces o alterar condiciones.

## Verificación y ejecutables

- **890 pruebas Rust correctas, 0 fallidas y 9 ignoradas**, en motor, formatos, proveedores, CLI y servidor. Las ignoradas no se cuentan como verificadas.
- **26 archivos de pruebas de interfaz correctos**; TypeScript y compilación de producción de la interfaz correctos.
- `cargo fmt --check` y Clippy con `-D warnings` correctos para el workspace sin el paquete de escritorio.
- Compilación release de CLI y aplicación de escritorio correcta. La aplicación nativa recién compilada no se abrió para una prueba visual.
- **526 resultados de traducción con Grok**, contando los mismos textos en distintos idiomas y el reintento; no son 526 textos únicos. **253 826 tokens reportados**. Concurrencia agregada limitada a 10. El costo monetario y el porcentaje de cuota restante son desconocidos; un subtotal numérico cero no demuestra gratuidad.
- Implementación y auditoría delegadas mediante Cursor CLI, modelo solicitado `gpt-5.6-sol-high`; las sesiones reportaron `GPT-5.6 Sol 272K High`, autenticación `login`. Integración y comprobaciones locales posteriores registradas.

Ejecutables actualizados:

- [Aplicación Project Locust](C:/Projects/Locust/target/release/locust-desktop.exe), SHA-256 `618e0117e3197a8a6d2bed4a03fcb4c977a1967b60aac2323c96943e88e1b07a`.
- [CLI Locust](C:/Projects/Locust/target/release/locust.exe), SHA-256 `a765be096f0d768389fde46652cc01555c1fabd7d69296e3813bd20d9f391c8f`.

Esta entrega recompila Locust; no sustituye ni recompila el instalador de Rule95 Patcher, que no cambió en esta tanda.

## Pendientes concretos

1. **Pruebas dentro de juegos completos:** arranque, escenas, menús, partidas guardadas, carga y cambio de idioma. La integridad del parche no demuestra esas funciones.
2. **Fuentes y presentación:** cobertura de glifos CJK, anchura, saltos de línea y lenguas con escritura de derecha a izquierda. No se verificaron árabe/hebreo ni sus interfaces.
3. **Unity:** presupuesto y reintento automático del bloque TextAsset completo; los espacios fijos demasiado pequeños todavía requieren decisiones de localización. No se debe anunciar una traducción total cuando quedan cadenas originales.
4. **Unreal:** prioridad entre varios PAK en un juego completo; la comprobación real utilizó el PAK generado por separado. No se verificaron el PAK base de varios GB, archivos cifrados/firmados ni IoStore como recorrido completo.
5. **SugarCube:** traducción que preserve fragmentos y marcadores de filas con estilos, enlaces o condiciones intercalados. Actualmente se omiten con una razón explícita.
6. **Más variantes y calidad lingüística:** otros juegos/versiones/contenedores, contexto narrativo, glosarios y revisión humana de términos. Las cadenas idénticas a la fuente se mantienen como advertencias; algunas son nombres o abreviaturas válidas y otras pueden ser localización pendiente.
7. **Varios idiomas seleccionables dentro del mismo juego:** las pruebas reales de esta tanda usaron copias por idioma. Add y registro de idiomas tienen pruebas automáticas, pero el selector de cada juego requiere verificación de ejecución.

## Evidencia reproducible

- [Harness multilingüe](C:/Projects/Locust/tools/qa_multilingual_matrix.py) y [uso](C:/Projects/Locust/tools/qa_multilingual_matrix.md).
- [Matriz inicial y fallos conservados](C:/Projects/Locust/tmp/engine-matrix-20260911/synthetic-fixed/REPORT.md).
- [Repetición local de 25 casos](C:/Projects/Locust/tmp/engine-matrix-20260911/synthetic-replay/results.json) y [alemán conciso de Unity](C:/Projects/Locust/tmp/engine-matrix-20260911/unity-de-concise/results.json).
- [Pivots sobre MZ japonés real](C:/Projects/Locust/tmp/engine-matrix-20260911/real-pivot-fixed/results.json).
- [Unity y Ren’Py reales](C:/Projects/Locust/tmp/engine-matrix-20260911/real-unicode-and-overrides-v2/results.json), [SugarCube final](C:/Projects/Locust/tmp/engine-matrix-20260911/real-sugarcube-final/results.json) y [bytes ajenos conservados](C:/Projects/Locust/tmp/engine-matrix-20260911/sugarcube-unchanged-bytes.json).
- [Pruebas Rust](C:/Projects/Locust/tmp/engine-matrix-20260911/final-workspace-tests-v3.log), [Clippy](C:/Projects/Locust/tmp/engine-matrix-20260911/final-clippy-v3.log), [compilación de aplicación](C:/Projects/Locust/tmp/engine-matrix-20260911/final-app-build.log) y [originales verificados](C:/Projects/Locust/tmp/engine-matrix-20260911/originals-final-verification.json).

Los registros identifican el ejecutable congelado de cada fase. La matriz API inicial usó `locust-fixed.exe`; las correcciones de lectura se verificaron con `locust-fixed-v2.exe`; SugarCube y la compilación final usan `locust-final.exe`. Los resultados antiguos se conservaron como evidencia, sin presentarlos como pruebas de una versión posterior.
