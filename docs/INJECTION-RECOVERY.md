# Recuperar una inserción interrumpida

Las inserciones Direct (`inject --direct`) y Add preparan sus cambios en una copia independiente y conservan un plan de recuperación antes de modificar los archivos del juego. Si el proceso termina a mitad, consulta el estado y recupera esa operación antes de volver a insertar, instalar/revertir un ZIP o restaurar un respaldo sobre el mismo juego.

Desde la aplicación, usa **Inicio → Recuperar inserción**. Puedes indicar una carpeta o archivo sin abrir su proyecto. Pulsa **Revisar recuperación** para consultar la operación; si hay conflictos, el panel enumera los archivos y exige confirmar que se guarden copias antes de reemplazarlos. La interfaz muestra las rutas de esas copias al terminar. Al fallar una apertura por inserción pendiente también se ofrece este panel.

La recuperación desde la interfaz queda vinculada al ID de la operación revisada. Si otra operación la sustituye, debes volver a consultar el estado antes de continuar. Cambiar de ruta, cerrar o recibir un error descarta la confirmación de conflictos.

```powershell
locust inject-status 'C:/juegos/mi-copia'
locust inject-recover 'C:/juegos/mi-copia'
locust inject-status 'C:/juegos/mi-copia'
```

Las órdenes imprimen JSON. `inject-status` muestra la raíz canónica y `pending`: será `null` cuando no haya inserción pendiente. Una operación pendiente identifica `transaction_id`, `phase`, motor, idioma, cantidad de archivos, ruta de originales y conflictos. El comando puede devolver un error si el juego está ocupado o los metadatos están dañados; un error no equivale a un estado limpio.

`inject-recover` restaura los originales de los archivos reemplazados y elimina los archivos nuevos que todavía coinciden con el resultado de esa inserción. Conserva archivos ajenos al plan y directorios que no estén vacíos. Si no hay operación pendiente, no cambia archivos. Una inserción terminada correctamente no se deshace con esta orden.

## Corregir una traducción ya insertada

En HTML genérico con ubicaciones guardadas y scripts Ren'Py sueltos, puedes cambiar una traducción y volver a insertar en Direct sobre la misma copia. Locust verifica que los archivos siguen coincidiendo con la inserción anterior y consulta su respaldo original para relacionar cada ubicación con el texto actual. Edita solamente las frases aceptadas; las traducciones omitidas o rechazadas permanecen como estaban. El parche distribuible sigue usando los hashes del original, mientras que la recuperación de esta nueva inserción conserva el estado inmediatamente anterior.

Una traducción igual al texto original puede devolver esa frase al original. Si reextraes el juego ya traducido y mantienes su texto actual, la reinserción lo conserva. Si un archivo registrado cambió fuera de Locust, falta el respaldo o una ubicación HTML puede identificar dos frases diferentes entre el original y el estado actual, la operación se rechaza antes de escribir. No borres los registros de procedencia para eludir ese error.

Esta actualización de ubicaciones no cubre scripts compilados, archivos RPA ni todos los demás motores. En esos casos siguen vigentes las comprobaciones de fuente del plugin. Conserva una copia original y comprueba el informe de frases omitidas.

## Conflictos y recuperación forzada

Antes de cambiar archivos, la recuperación comprueba el plan completo, las rutas, los originales guardados, sus tamaños y hashes, y los destinos actuales. Si encuentra contenido que no coincide con el original ni con el resultado esperado, rechaza la recuperación normal. Revisa los archivos indicados: pueden ser ediciones realizadas después del fallo.

Si decides restaurar los originales conservando aparte esas ediciones, usa:

```powershell
locust inject-recover 'C:/juegos/mi-copia' --force
```

`--force` copia y verifica primero todos los archivos conflictivos, y guarda un manifiesto que relaciona cada copia con su ruta de origen. El resultado enumera esas copias en `preserved_conflicts`. Permanecen bajo la operación, en `.locust-injections/operations/<id>/conflicts/<generación>/`; revísalas para recuperar después los cambios que quieras conservar.

Force no permite saltarse originales corruptos, rutas inseguras, enlaces/reparse, tipos de archivo incompatibles ni destinos de solo lectura. Resuelve la condición indicada antes de reintentar. Si aparece un error después de guardar copias de conflictos, conserva el directorio de la operación aunque el comando no haya llegado a imprimir el informe final.

La recuperación se puede repetir tras otra interrupción: reconoce archivos que ya volvieron a su estado original y mantiene sus copias de origen. Un fallo de disco, de permisos o una edición externa durante el recorrido todavía puede interrumpirla; consulta de nuevo el estado y conserva los datos de recuperación.

## Etapas y datos conservados

| `phase` | Significado | Qué hace la recuperación |
|---|---|---|
| `preparing` | Se preparaba la copia; no se habían instalado resultados en el juego | Cancela la preparación y limpia solamente los datos propios que pueda reconocer |
| `applying` | Existe un plan y pueden haberse instalado algunos resultados | Vuelve al estado anterior utilizando el plan completo |
| `committed_unrecorded` | Los archivos llegaron a su estado final, pero falta la confirmación terminal de la operación | Permite volver al estado anterior; no presupone que SQLite esté vacío |
| `rolling_back` | La propia recuperación quedó incompleta | Continúa reconociendo los archivos que ya se restauraron |

La última fase de escritura de archivos y el commit de SQLite son operaciones distintas. Incluso en `committed_unrecorded`, SQLite puede haber confirmado su escritura justo antes de que el proceso terminase. La recuperación no modifica una base de datos a partir del journal. Si una grabación previa apunta a bytes que se acaban de revertir, al empaquetar deben verificarse sus hashes: no debe tratarse esa grabación como prueba de una traducción todavía instalada. Tras recuperar, vuelve a insertar desde la DB adecuada antes de generar un nuevo parche.

Add registra sus resultados dentro del bloqueo de la transacción. Los callers reciben ese resultado ya calculado; no vuelven a leer los archivos para grabarlos después de liberar el bloqueo. Esto coordina la grabación con la inserción, pero no convierte el sistema de archivos y SQLite en una única transacción atómica.

El historial vive en `.locust-injections`, separado de los recibos de ZIP de `.locust`. Incluye identidad de la operación, fases, plan, originales y, cuando corresponda, copias de conflictos. La copia de trabajo y los resultados temporales se limpian de forma conservadora después de confirmar una fase terminal. Datos cambiados o no reconocidos pueden quedar para diagnóstico. No borres ni edites estos archivos para quitar un aviso de operación pendiente: perderías la información que permite decidir qué restaurar. Directorios con nombres parecidos, sin autoridad válida, no se adoptan como recuperación.

## Inserción, ZIP y respaldo completo

| Operación que quedó pendiente | Inspección | Recuperación correspondiente |
|---|---|---|
| Inserción Direct o Add | `locust inject-status <juego>` | `locust inject-recover <juego>` |
| Instalación o reversión de un ZIP | `locust patch-status <juego>` | `locust patch-rollback <juego>` |
| Restauración elegida de un respaldo completo | Gestión de respaldos de Locust | Restaurar el respaldo concreto a su origen registrado |

Los dos comandos de reversión tienen contratos distintos. En particular, `inject-recover --force` conserva aparte los archivos conflictivos; no atribuyas esa misma conservación a `patch-rollback --force`. No uses una restauración completa para intentar eludir una transacción pendiente. La restauración de respaldos y los mutadores de parches comprueban el estado pendiente bajo el mismo bloqueo del juego.

Los respaldos v2 guardan metadatos en `<id>/manifest.json` y el contenido en `<id>/payload/`. Así se conserva también un `manifest.json` legítimo del juego. Cada archivo tiene tamaño y SHA-256; la restauración verifica el inventario antes de escribir y usa exclusivamente el origen registrado. La raíz debe seguir existiendo con el tipo esperado. Para una fuente que era un solo archivo, el contenido se guarda como `payload/file`.

Después de una inyección Direct, la base de datos guarda el ID, origen y almacén de su respaldo original junto con los hashes de los archivos insertados. El botón **Abrir Parche → Empaquetar** activa los hashes prístinos; al reabrir Empaquetar por separado puedes activar esa casilla y recuperar la misma copia, incluso tras reiniciar o cambiar de perfil. `locust patch` la recupera automáticamente cuando no indicas `--pristine`. Una inserción sin cambios conserva la asociación anterior.

El empaquetador comprueba el inventario, los hashes y la generación registrada. Una copia ausente, alterada o incorrecta detiene el proceso sin reemplazar un ZIP existente; nunca elige arbitrariamente un respaldo más reciente. Una carpeta prístina explícita permite trabajar con otra copia intacta. Los registros antiguos sin esta procedencia mantienen su búsqueda heredada y pueden necesitar una carpeta explícita. La copia corresponde al estado previo de esa generación de inserción: si ya estaba traducida, no equivale a un juego de distribución intacto.

Direct no elimina automáticamente respaldos completos de otras operaciones para conservar solo los últimos tres: otros proyectos pueden seguir utilizándolos al empaquetar. El espacio ocupado puede crecer; administra los respaldos conservando los originales de los parches que aún necesitas generar.

Los respaldos nuevos excluyen de la copia el store `.locust-injections` únicamente cuando está reconocido y vinculado a ese juego. Un store desconocido o inválido produce un error. Restaurar un respaldo antiguo que contenga ese namespace se rechaza para no reintroducir un journal de otro momento. El historial de la transacción conserva sus propios originales y no depende solamente de la retención de respaldos completos.

Los respaldos legacy no tienen hashes históricos por archivo. Se pueden comprobar sus archivos legibles, cantidades y tamaños, pero no demostrar retrospectivamente que no sufrieron una alteración del mismo tamaño. Los orígenes relativos se rechazan porque no quedó guardado el directorio de trabajo original. Si el formato antiguo sobrescribió el `manifest.json` del juego con metadatos, esa información perdida no se puede reconstruir. Una restauración completa tampoco combina ediciones posteriores ni elimina automáticamente todos los archivos añadidos desde el respaldo.

## Archivos individuales, bloqueo y espacio

Puedes consultar o recuperar mediante la ruta de un archivo seleccionado. La operación usa su carpeta padre como raíz de bloqueo y del journal; distintas operaciones sobre archivos de esa carpeta comparten ese bloqueo. Si el archivo ya no existe, consulta la carpeta padre que contiene el historial de esa operación.

Para una selección de carpeta, la preparación copia el árbol del juego, excluyendo los namespaces de recuperación, y compara inventarios antes y después de ejecutar el plugin. Para un archivo individual prepara su copia y registra los resultados que aparezcan dentro de ese ámbito de trabajo. Los plugins que descubren otros recursos pueden necesitar que selecciones la carpeta del juego.

El coste de disco incluye el respaldo completo, una copia de trabajo independiente y copias de los archivos modificados como originales/resultados. Durante una recuperación forzada se añaden las copias de conflictos. Los hashes requieren leer los archivos varias veces; no es una operación de coste constante ni una optimización para juegos de varios GB. No se usan hardlinks para la copia: un plugin que escribiera sobre ellos podría modificar el original. El espacio insuficiente durante la preparación debe resolverse antes de instalar resultados; no elimines los originales de recuperación para liberar espacio.

Se conservan permisos básicos de archivos, incluidos el modo Unix y el atributo de solo lectura que se comprueban en la operación. Esto no garantiza conservar ACL completas, propietario, atributos extendidos, timestamps ni todos los metadatos del sistema de archivos. Los archivos de solo lectura que habría que modificar requieren resolver ese estado antes de recuperar. El bloqueo excluye operaciones cooperantes de Locust; no inmoviliza un editor externo, el juego o su actualizador. Tampoco se promete durabilidad frente a cualquier corte eléctrico o fallo físico del disco.

## Controles y proyectos JA → EN → destino

Cada idioma de destino usa su propia DB. En un pivot, el inglés es la fuente semántica de traducción y el japonés físico permanece en `locust_injection_source`. La validación y el gate de inserción comparan los controles de la traducción con ambas referencias. Pivot rechaza un inglés intermedio que haya perdido controles antes de crear la DB de destino. Una traducción guardada vacía también puede ser un error si elimina controles del original.

Si, por ejemplo, `こんにちは {name}` terminó como `Hello`, corrige la traducción inglesa para conservar `{name}` y crea un pivot nuevo para cada destino. Conserva la DB anterior mientras recuperas el trabajo que necesites; no borres la procedencia japonesa para hacer pasar la validación. En una DB ya pivotada con inglés incorrecto, reparar solo el destino puede seguir fallando porque las dos referencias deben concordar. Las traducciones manuales/importadas pueden quedar guardadas para corregirlas: valida antes de insertar. Si además hay un aviso de original cambiado, retraduce o revisa explícitamente la traducción frente al original actual.

El protector reconoce códigos RPG como `\V[1]`, `\I[2]` y `\G`, y un subconjunto de Ren'Py: `[player.name]`, índices numéricos o de texto, conversiones, tags conocidos con parámetros como `{color=#fff}`, y su anidación cuando el original está balanceado. Respeta escapes literales `[[` y `{{`. No evalúa Python ni cubre expresiones arbitrarias, llamadas, aritmética, variables Unicode o todos los tags personalizados. Que una cadena pase este control no demuestra que toda su sintaxis o presentación sea correcta en el juego.

El renderizado de fuentes, atlas TMP/SDF, composición RTL, ajuste de línea y montaje real de overlays requieren comprobaciones distintas. Véase [FONT-PATCH.md](FONT-PATCH.md). Esta guía describe el contrato de recuperación; no sustituye las pruebas de una compilación concreta ni certifica compatibilidad con todos los juegos.

## API HTTP

Las mismas operaciones están disponibles como `POST /api/inject/status` y `POST /api/inject/recover`. El cuerpo identifica explícitamente el juego:

```json
{"game_path":"C:/juegos/mi-copia","force":false}
```

`force` se aplica a recuperación; se puede omitir y vale `false`. La API devuelve el estado o el informe de recuperación en JSON. Los conflictos de operación/juego devuelven un error, normalmente HTTP 409. Desconectar la petición no libera la reserva mientras el trabajador siga restaurando archivos.

La recuperación admite `expected_transaction_id` opcional. Si se proporciona, el motor comprueba bajo bloqueo que sigue siendo la operación pendiente; un ID distinto o ya terminado devuelve error sin restaurar archivos. La interfaz siempre lo envía. Los clientes antiguos que lo omiten mantienen la recuperación idempotente.

`POST /api/patch/pack` admite `pristine_backup_id`, el ID exacto devuelto por `/api/inject`. Requiere `pristine: true` y no se combina con `pristine_path`. La API verifica origen, tipo e inventario del respaldo antes de empaquetar; no restaura el juego como paso intermedio. Un respaldo inválido devuelve HTTP 400.

### Ubicación del ZIP

Guarda el ZIP fuera de la carpeta del juego y de las copias prístinas o respaldos. El empaquetador rechaza esos destinos antes de crear carpetas o temporales y protege también el archivo de la base de datos activa y sus archivos auxiliares SQLite. La API protege todo el almacén de respaldos, incluidos sus manifiestos. Un destino externo válido permite reemplazar un ZIP anterior.
