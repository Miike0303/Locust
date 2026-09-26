# Continuación: persistencia y recuperación del motor

## Cierre del 19 de septiembre y nueva tanda

La tanda de persistencia/recuperación está **completada con límites explícitos**. Evidencia autoritativa: `tmp/goal-20260917/RESULT.json`, `STATE.json` y `final-source-sha256.json`. Los 255 archivos fuente y dependencias siguen coincidiendo con los usados para verificar los ejecutables.

- Rust: 1.336 pruebas pasan, 16 ignoradas, 55 suites; fmt y Clippy sin advertencias; release CLI y escritorio compilados.
- Frontend: 31 archivos de pruebas, TypeScript y Vite; seis recorridos con backend real, incluyendo cierre/recarga de borradores.
- Replay release JA→EN→es/fr/de/pt-BR/zh-CN en HTML, Ren'Py, RPG Maker MV, Unity y Unreal: 25/25 casos y 255/255 filas. Inserción, empaquetado, aplicación, reextracción y rollback exacto; cero llamadas API nuevas.
- Rule95 final: 25/25 casos, 38 entradas intactas. Nueve originales de juegos y nueve copias prístinas mantienen hashes/tamaños.
- Ren'Py con recurso real pasa. Unity real: extracción release de 4.374 filas en 13,359 s; el smoke completo de inserción debug agotó 180 s y es **inconcluso**.
- Revisión nativa parcial: bienvenida del build final comprobada. El controlador de ventanas falló durante el diálogo de archivo; no se certifica el editor nativo con esa prueba. La cobertura completa del editor corresponde al navegador con backend real.

La recuperación Direct/Add ya está disponible por CLI/API. La copia independiente consume espacio y tiempo; restaura permisos básicos, no promete ACL/xattrs ni garantías ante corte eléctrico. La restauración de respaldos es atómica por archivo. Los fixtures no certifican todos los juegos, renderizado TMP/SDF, RTL o contenedores no admitidos.

Nueva solicitud del usuario: mejorar la app hasta las **14:00 del 19 de septiembre, hora de México**, o agotar la cuota disponible. La nueva tanda continúa con recuperación desde la interfaz y persistencia coherente de proyectos recientes; estado en `tmp/goal-20260919/STATE.json`. No se compran créditos ni se canjean resets. El texto restante es histórico y no describe pendientes actuales de la tanda cerrada.

## Historial de la reactivación

> Autorización posterior del usuario: **ya se puede usar pantalla/visualización y Computer Use** para validar la aplicación y los juegos. Sustituye la restricción de escritorio en los checkpoints anteriores; las implementaciones siguen en segundo plano por defecto.

> Actualización tras «reactiva el goal»: el goal está **activo** y sustituye el cierre descrito debajo. Desarrollo con CLI/agentes Astra y pruebas aisladas; la autorización posterior permite revisión visual. El estado de trabajo vigente está en `tmp/goal-20260917/STATE.json` y los requisitos en `REQUIREMENTS.md` del mismo directorio.
>
> Integrado: borradores persistentes por revisión; respaldos v2 con inventario, SHA y IDs no reutilizables; markers de parches sin ventana de ausencia; recuperación genérica Direct/Add con copia independiente y registro bajo bloqueo; comandos/API de estado y recuperación; controles frente a fuente semántica y física; exclusión de backups al extraer; aislamiento del historial al copiar; validación de destinos VNTextPatch. Véase [la guía de recuperación](INJECTION-RECOVERY.md).
>
> Evidencia intermedia: 636 pruebas core/server/CLI aprobadas, dos pruebas nuevas de CLI y seis de API aprobadas, ocho terminaciones reales de proceso, frontend y seis recorridos con backend real aprobados. Replay preview 25/25 casos y 255/255 traducciones en cinco motores por cinco idiomas, con aplicación y reversión exactas; nueve originales y nueve copias intactos. Las últimas entregas se están verificando en la suite completa y builds release. Quedan su replay final, revisión nativa y Rule95 compartido. Las cifras y artefactos del cierre siguiente son históricos.

## Cierre previo a la reactivación

Se retomó el proyecto por solicitud del usuario, sin reactivar el antiguo goal por tiempo.

El editor conserva ahora los borradores al cerrar el panel con Escape o su botón, cambiar de fila y navegar a otra página. El texto se guarda en memoria de la sesión, separado por base de datos, ruta del juego, formato e identificador de entrada. Las bases pivotadas a distintos idiomas no comparten borradores.

Una escritura pendiente pertenece a esa entrada y continúa aunque el panel se desmonte. Si se vuelve a abrir, se espera la misma promesa, sin otra petición. Los errores conservan el texto y se muestran al volver. El borrador solo se elimina cuando el texto del servidor lo confirma; un refresco atrasado no borra una edición posterior. La revisión sigue esperando el guardado exitoso y comprueba que el proyecto no cambió antes de escribir el estado.

Las consultas del editor incluyen el proyecto en su clave para evitar reutilizar una entrada de otra base con el mismo ID. La cola conserva el database_path y los avisos de extracción devueltos al abrir un proyecto.

## Verificación

- 29 archivos de pruebas unitarias de frontend pasan. El nuevo editorDrafts.test.ts comprueba separación entre bases/filas, errores síncronos y asíncronos, escrituras compartidas, respuestas tardías y borradores vacíos.
- TypeScript y Vite producción pasan. No se modificó el motor Rust: las 1 239 pruebas y la matriz de 25 casos de la sesión anterior siguen siendo evidencia histórica, no se presentan como una nueva ejecución.
- Prueba de navegador headless con frontend de producción y backend real: Escape, cambio de fila, navegación a Revisión y regreso, fallo de guardado, reapertura mientras guarda y revisión posterior al guardado pasan.
- Los flujos existentes de creación/verificación de parches de fuentes y original modificado también pasan contra el mismo frontend actualizado.
- Logs y capturas: tmp/continuation-20260917/. No hubo llamadas a proveedores de traducción ni control del escritorio, y solo se escribieron fixtures propios.

## Límites y próximos pendientes

La conservación del borrador es durante la sesión: cerrar/recargar la aplicación todavía descarta texto que no se guardó en la base. Guardar con Ctrl+Enter o al salir del campo sigue siendo necesario para persistirlo. Quedan las comprobaciones visuales dentro de juegos, atlas TextMeshPro/SDF, composición RTL, compatibilidad de contenedores no admitidos y recuperación de inserciones directas tras caídas abruptas, según docs/ENGINE-ASTRA-2026-09-12.md.

No se publicaron cambios ni se modificaron juegos originales. Se preservó el trabajo previo del checkout.

Compilación release de escritorio completada: C:/Projects/Locust/target/release/locust-desktop.exe. Huellas SHA256 en tmp/continuation-20260917/artifact-sha256.json; resultado final en RESULT.json. Servidor QA cerrado. El CLI y el motor Rust no cambiaron en esta continuación.
