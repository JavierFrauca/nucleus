# Debugging

Guía para diagnosticar problemas con Nucleus: logs, síntomas comunes y cómo
aislarlos. Pensada para el modo servidor (`nucleus-server`); el modo embebido
(DLL) expone los mismos errores a través del C-ABI (ver
[operación](operacion.md)).

## Logs

El servidor usa `tracing`. El nivel se controla con `RUST_LOG`:

```bash
# General
RUST_LOG=info cargo run --release -p nucleus-server

# Verboso (incluye warn de las dependencias)
RUST_LOG=debug cargo run --release -p nucleus-server

# Solo el motor, a trace, y lo demás en warn
RUST_LOG=nucleus_core=trace,nucleus_server=trace,warn cargo run --release -p nucleus-server
```

> Subir a `debug`/`trace` genera **muchas** líneas y puede afectar al
> rendimiento. Úsalo para diagnosticar, no en producción.

## Síntomas comunes y diagnóstico

### La primera ingesta tarda muchísimo

Esperado la **primera vez**: fastembed descarga el modelo de embeddings
(**~450 MB**, `multilingual-e5-small`) a `NUCLEUS_MODEL_CACHE`. Las ingestas
posteriores usan la caché y van rápido.

- Confirma que `NUCLEUS_MODEL_CACHE` apunta a un directorio **persistente** (si
  no, se redescarga en cada arranque).
- Si la red hacia HuggingFace es lenta, pre-descarga el modelo en una máquina
  con buena conexión y copia el contenido de la caché.

### Una búsqueda devuelve siempre 0 resultados

1. ¿El dominio tiene datos? `GET /v1/domains/{id}/documents`.
2. ¿El `subdomain` existe? Un nombre inexistente en `subdomain` devuelve 0
   (no se auto-crea al buscar, solo al ingestar).
3. ¿El `filter` es válido? Un filtro mal formado responde con error 400; uno
   bien formado pero sin matches responde `[]`.
4. ¿El token tiene scope `Read` sobre ese dominio? Sin él, 403.

### Una búsqueda devuelve resultados inesperados / poco relevantes

- La búsqueda es **híbrida** (vector + BM25). Prueba a activar **reranking**
  (`NUCLEUS_RERANK_MODEL=bge-reranker-base`) para mejorar el orden.
- Revisa el **chunking**: los chunks por defecto son de ventana fija. Para
  documentos con estructura, pre-trocea y envía `chunks[]`.
- Usa `GET /v1/chunks/{id}/context` para ver el chunk con sus vecinos y entender
  qué se recuperó.

### 503 Service Unavailable bajo carga

Es el **load-shed**: no hay hueco en el semáforo de concurrencia dentro de
`NUCLEUS_SEARCH_WAIT_MS`. Palancas:

- Subir `NUCLEUS_MAX_CONCURRENT_SEARCHES` (defecto: 16× núcleos).
- Subir `NUCLEUS_SEARCH_WAIT_MS` (defecto: 2000) para esperar más antes de
  rechazar.
- La búsqueda es **CPU-bound** (domina el embedding de la query); más CPU real
  ayuda más que aumentar la concurrencia pasada la saturación.

### La latencia de búsqueda es alta con reranking

El cross-encoder puntúa cada par `(consulta, candidato)` y cuesta **~150 ms por
candidato en CPU**. Baja `NUCLEUS_RERANK_CANDIDATES` (defecto 20; con 10 es la
mitad de latencia). Ver la tabla en
[configuración → reranking](configuracion.md#coste-y-nucleus_rerank_candidates).

### La memoria crece con documentos grandes

La ingesta embebe en **ventanas acotadas**, pero más workers = más ingestas
concurrentes = más pico. Para PDFs enormes, baja `NUCLEUS_WORKERS` a 2–4. Ver
[operación → memoria](operacion.md#memoria-y-rendimiento).

### Error al abrir la base de datos

- **`wrong passphrase` / clave**: la BD se abrió con otra passphrase u otra
  clave de máquina. El cifrado está siempre activo; sin la clave correcta no se
  abre. Ver [cifrado en reposo](operacion.md#cifrado-en-reposo).
- **Versión de esquema más nueva**: la BD fue creada por una versión posterior.
  Actualiza el binario o parte de una BD nueva. Ver
  [compatibilidad de esquema](compatibilidad-esquema.md).

## Aislar problemas de rendimiento

```bash
# Perfilado rápido: medir una búsqueda concreta
time curl -s -X POST $BASE/v1/domains/1/search \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"query":"prueba","k":5}' > /dev/null

# Test de carga (Node 18+, sin deps)
node scripts/loadtest.mjs
```

Para repetir el benchmark del corpus fiscal, ver
[rendimiento](rendimiento.md) y los benchmarks de Criterion en
[`crates/core/benches/`](../crates/core/benches/README.md).

## Debuggear el motor aislado (sin HTTP)

El crate `nucleus-core` es una librería; puedes escribir un ejemplo temporal
que la ejerza directamente, sin red:

```bash
cargo run --example ingest_fiscal -p nucleus-core
```

Los ejemplos en `crates/core/examples/` muestran ingestión, búsqueda, mint de
tokens y benchmarks de reranking.

## Reportar un bug

Antes de abrir un issue, recoge:

1. Versión (`nucleus-server --version` o el tag del binario).
2. SO y arquitectura.
3. Pasos para reproducir (o el corpus + query, si es de recuperación).
4. Logs con `RUST_LOG=debug` alrededor del problema.
5. Comportamiento esperado vs. real.

Ver [CONTRIBUTING](../CONTRIBUTING.md) para el flujo completo.
