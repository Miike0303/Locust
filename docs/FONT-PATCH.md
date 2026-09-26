# Fuentes: comprobación y parche combinado

En la aplicación: abre el proyecto traducido, pulsa **Validar → Crear parche de fuentes**, selecciona una copia limpia, la fuente nueva y la ruta de la fuente que ya usa el juego. Indica un ZIP de salida nuevo fuera de la carpeta del juego; el ZIP de traducción para combinar es opcional. **Abrir en el administrador de parches** deja el resultado seleccionado para verificarlo e instalarlo. Crear el ZIP no cambia el juego. La cobertura se calcula con las traducciones de la base abierta, sin tener que exportar un archivo de texto.

La cobertura incluye el texto físico que seguirá visible en las entradas todavía sin traducir, también en una base pivotada a inglés. Si una traducción conservada pertenece a un original que cambió, primero hay que retraducirla o revisarla/aprobarla explícitamente; el generador rechaza esas entradas antes de escribir el ZIP.

`font-check` mide entradas Unicode del mapa de caracteres de TTF/OTF y cada cara de TTC/OTC por separado. Omite espacios, controles y caracteres de formato no imprimibles. Informa errores de lectura/análisis y WOFF/WOFF2 no admitidos. Cada archivo está limitado a 64 MiB. Una cobertura de 100 % no certifica selección de fuente, ligaduras, orden RTL ni espacio disponible en el juego.

```powershell
locust font-check 'C:/juegos/copia-limpia' --text-file 'C:/traduccion/texto-destino.txt'
locust font-check 'C:/mis-fuentes/fuente.ttf' --text-file 'C:/traduccion/texto-destino.txt'
```

`font-patch` genera un ZIP normal de Locust para sustituir un archivo de fuente suelto que el juego ya usa. Exige una ruta de destino existente y una fuente TTF/OTF del mismo formato. El archivo de texto debe contener los caracteres reales de las traducciones; una fuente que carece de alguno se rechaza. La herramienta no descarga fuentes. Para compartir el ZIP, utiliza una fuente cuya licencia permita esa redistribución y acompaña el parche con la licencia correspondiente.

Para incluirla en una traducción, primero genera el parche de texto habitual con sus originales (`locust patch ... --pristine <copia-limpia>`). Combínalo contra esa copia limpia:

```powershell
locust font-patch 'C:/juegos/copia-limpia' --font 'C:/mis-fuentes/fuente.ttf' --target 'fonts/gamefont.ttf' --engine rpgmaker-mz --lang es --text-file 'C:/traduccion/texto-destino.txt' --base-patch 'C:/traduccion/texto.zip' --output 'C:/traduccion/texto-y-fuente.zip'
locust apply 'C:/juegos/copia-limpia' 'C:/traduccion/texto-y-fuente.zip'
locust patch-rollback 'C:/juegos/copia-limpia'
```

La combinación conserva identificador, versión, idioma, motor y entradas originales del parche base, y añade la fuente con hashes de origen y destino. Verifica el parche base y rechaza conflictos de destino, archivos alterados, rutas inseguras y diferencias de idioma/motor. MV y MZ pertenecen al adaptador `rpgmaker-mv` de Locust; `rpgmaker-mz` y las variantes con guion bajo se aceptan como alias al combinar. El límite de combinación es 256 MiB descomprimidos y 10 000 entradas. El archivo de salida debe ser nuevo; no se sobrescribe uno existente.

`apply` conserva el respaldo y recibo habituales; `patch-rollback` restaura tanto el texto como la fuente. Para actualizar una instalación ya parcheada, vuelve a una copia limpia o revierte el parche anterior antes de aplicar el combinado. Locust registra un parche instalado por juego: no fuerces un parche de fuente independiente encima de otro de traducción. Si omites `--base-patch`, se genera únicamente el parche de fuente.

Motores admitidos: RPG Maker MV/MZ, Ren'Py y HTML con fuentes sueltas TTF/OTF. La herramienta no identifica automáticamente qué fuente usa cada pantalla, no cambia nombres de familia internos ni referencias CSS, no modifica estilos, no envuelve líneas largas y no adapta RTL. Los juegos que seleccionan fuentes por nombre interno en vez de ruta pueden necesitar cambios específicos adicionales. Prueba el resultado en el motor antes de distribuirlo.

En Unity con TextMeshPro, la cobertura de una fuente TTF no implica que su atlas SDF tenga esos glifos. Es necesario regenerar el recurso TMP/atlas incluyendo los caracteres de destino o configurar un recurso TMP de respaldo. Esta orden rechaza Unity; no sustituye recursos SDF por un TTF.

Pruebas reproducibles:

```powershell
cargo test -p locust-core font_ --lib
cargo test -p locust-cli --test font_tests
```

La segunda orden incluye el flujo real de extracción, traducciones manuales de QA, inserción, empaquetado con grabación en la base de datos, combinación, aplicación y reversión de un proyecto neutral de RPG Maker. La fuente sintética incluida es propia de las pruebas y no contiene dibujos de glifos; estos ensayos validan cobertura y transacciones, no apariencia visual.

La auditoría reutiliza el conjunto de caracteres del texto entre todas las caras y archivos de fuente. Excluye las carpetas de recuperación `.locust` y `.locust-injections`, además de los metadatos `.git` al recorrer un juego; puedes comprobar una carpeta de respaldo explícitamente pasando esa carpeta como raíz. Un informe con cero caracteres requeridos no demuestra cobertura; `font-patch` rechaza textos vacíos o compuestos únicamente por espacios y controles. Los códigos de idioma deben tener un segmento inicial alfabético y segmentos adicionales alfanuméricos no vacíos, de 1 a 8 caracteres por segmento; `es-MX` y `zh_CN` son válidos.

Las sugerencias identifican familias candidatas según la escritura, sin prometer todos los caracteres. [Noto Sans Arabic](https://notofonts.github.io/noto-docs/specimen/NotoSansArabic/), [Noto Sans Hebrew](https://notofonts.github.io/noto-docs/specimen/NotoSansHebrew/) y [Noto Sans Thai](https://notofonts.github.io/noto-docs/specimen/NotoSansThai/) son familias distintas; la recomendación latina/cirílica apunta al [proyecto correspondiente](https://github.com/notofonts/latin-greek-cyrillic). Para CJK se reconocen también los bloques de extensión, compatibilidad y planos suplementarios según [Unicode 17](https://www.unicode.org/Public/17.0.0/ucd/Blocks.txt). [Noto Sans CJK](https://notofonts.github.io/noto-docs/specimen/NotoSansCJKsc/) es una opción que debes comprobar con el archivo real: la detección de un carácter suplementario no significa que esa familia lo incluya.

Medición reproducible de rendimiento, con fuente sintética propia de Locust:

```powershell
cargo run -p locust-core --release --example font_audit_bench -- crates/cli/tests/fixtures/synthetic-ascii.ttf
```

El ejemplo genera tres colecciones de cuatro caras y un corpus multiescritura de aproximadamente 8 MiB, audita cuatro veces y devuelve tiempos y recuentos en JSON. No utiliza ni copia fuentes del sistema.


La auditoría de **Validar** usa el mismo criterio de texto previsto que el generador de parches: traducciones no vacías más el original físico de las filas aún sin traducir, aunque su origen editable se haya pivotado al inglés. Los espacios o traducciones vacías usan ese original. Si el origen protegido es inválido o una traducción está obsoleta, se informa del problema en lugar de mostrar una cobertura favorable. No se exigen glifos del original japonés cuando esa fila ya tiene una traducción válida a otro idioma.
