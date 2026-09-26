# Matriz QA multilingüe sintética

`qa_multilingual_matrix.py` valida el CLI de Locust con cinco fixtures mínimos:
RPG Maker MV, HTML, Ren'Py, Unity SerializedFile v17 y Unreal LocRes v0.

Uso completo:

```powershell
python tools/qa_multilingual_matrix.py `
  --locust target/release/locust.exe `
  --config "$env:LOCALAPPDATA/project-locust/config.json" `
  --output tmp/qa-matrix-20260911 `
  --provider grok-sub `
  --targets es,fr,de,pt-BR,zh-CN `
  --workers 5 `
  --request-concurrency 2
```

Preparación sin llamadas al proveedor:

```powershell
python tools/qa_multilingual_matrix.py `
  --locust target/release/locust.exe `
  --config "$env:LOCALAPPDATA/project-locust/config.json" `
  --output tmp/qa-matrix-prepare `
  --prepare-only
```

`--dry-run` es alias de `--prepare-only`. El directorio indicado en `--output`
debe ser nuevo. No hay reanudación: para repetir se debe usar otro directorio,
lo que evita mezclar bases, grabaciones de inyección y resultados anteriores.

## Qué comprueba

1. Congela una copia del ejecutable y registra SHA-256 del binario y la
   configuración, sin copiar el contenido de configuración.
2. Genera fixtures con 8–12 cadenas japonesas, duplicados, Unicode y
   placeholders.
3. Extrae, traduce JA→EN con `--no-memory`, valida y crea un `pivot` por idioma.
4. Traduce EN→destino con contexto correcto para cada par y valida.
5. Restaura una copia japonesa fresca en la misma ruta del proyecto, inyecta en
   modo directo y exige cambios de hashes, cadenas escritas y grabación.
6. Empaqueta con `--pristine`, exige hashes originales en el manifiesto, aplica
   a otra copia japonesa y verifica rollback por hashes.
7. Re-extrae el artefacto traducido cuando la inyección tuvo cobertura.
8. Ejecuta una comparación directa JA→ES independiente en RPG Maker.

Cada comando conserva stdout, stderr, código de salida y duración. `report.json`
contiene snapshots SQLite, tokens, costo y su completitud, omisiones binarias
contra la capacidad física original, hashes y resultados por fase. Las filas de
un bloque Unity agrupado no reciben un límite ficticio por celda: la inyección
comprueba el bloque reconstruido. `REPORT.md` resume el resultado en español.

La relectura conserva los textos crudos y admite únicamente el relleno final de
espacios de Unity y el reflujo de líneas de RPG Maker. Las advertencias de texto
idéntico se registran para revisión lingüística; otros errores bloquean la fase.

Una fase fallida bloquea únicamente sus dependientes; los demás motores e
idiomas continúan. Un `locust inject` con código 0 pero cero cadenas escritas se
marca como fallo de cobertura.

## Límites

- Los fixtures son estructuralmente auténticos pero sintéticos; no prueban que
  un juego comercial arranque o muestre correctamente el texto.
- El harness no modifica filas SQLite para compensar que `pivot` reemplace la
  fuente japonesa por la inglesa. Los formatos que necesitan comprobar la
  fuente original pueden fallar y ese resultado es evidencia del pipeline.
- `workers × request-concurrency` no puede superar 10. El timeout configurable
  debe estar entre 300 y 900 segundos.
- El archivo pasado mediante `--config` se usa en solo lectura y se verifica al
  final. El proveedor puede renovar su token OAuth externo según el
  comportamiento normal del CLI.
