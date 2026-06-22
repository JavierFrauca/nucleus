# Referencia de la API

API REST sobre HTTP (axum). Todos los cuerpos son JSON salvo la subida de ficheros
(bytes crudos). Los identificadores son enteros (`u64`); las fechas son `created_at` en
milisegundos Unix (`i64`).

## Autenticación

Todas las rutas salvo `GET /healthz` requieren la cabecera:

```
Authorization: Bearer <token>
```

Los tokens son **API-keys opacas** (`nuc_…`), se guardan **hasheadas** (SHA-256) y se
muestran **una sola vez** al crearlas. Cada token lleva **scopes**:

```json
{ "domain": "All" | { "One": <domain_id> }, "perm": "Read" | "Write" | "Admin" }
```

- `perm` es ordenada: `Admin` ⊇ `Write` ⊇ `Read`.
- **Admin global** = un scope con `domain: "All"` y `perm: "Admin"`. Necesario para crear
  dominios y gestionar tokens.
- Al primer arranque con BD vacía, el servidor crea e imprime un **token admin** inicial.

## Modelo de errores

Errores como `{ "error": "<mensaje>" }` con el código HTTP:

| Código | Cuándo |
|--------|--------|
| `400` | petición inválida, dimensión de vector incorrecta, modelo desconocido |
| `401` | falta token o es inválido/expirado |
| `403` | token válido sin el scope requerido |
| `404` | dominio/subdominio/documento/chunk/tag/job inexistente |
| `500` | error interno (almacenamiento, embeddings, E/S) |

---

## Salud

### `GET /healthz`
Sin auth (liveness). Devuelve `ok` (texto plano).

### `GET /readyz`
Sin auth (readiness). `ready` si el almacenamiento responde; error si no.

### `GET /metrics`
Sin auth. Métricas en texto Prometheus (`nucleus_search_total`,
`nucleus_search_latency_ms_total`, `nucleus_ingest_total`,
`nucleus_ingest_duplicate_total`). **Protégelo a nivel de red/proxy.**

---

## Dominios

### `POST /v1/domains` · *Admin*
Crea un dominio.
```json
{ "name": "fiscal", "model": "multilingual-e5-small" }
```
`model` es opcional (por defecto `multilingual-e5-small`). Respuesta:
```json
{ "id": 1, "name": "fiscal", "model": "multilingual-e5-small", "dim": 384, "created_at": 1781883615750 }
```

### `GET /v1/domains` · *autenticado*
Lista todos los dominios → `[Domain, …]`.

### `GET /v1/domains/{id}` · *Read*
Devuelve un `Domain`.

### `PATCH /v1/domains/{id}` · *Admin*
Renombra el dominio. Cuerpo `{ "name": "nuevo" }`; devuelve el `Domain` actualizado.

### `DELETE /v1/domains/{id}` · *Admin*
Borra el dominio **en cascada**: subdominios, documentos, chunks, embeddings, etiquetas
e índices, además de las búsquedas por nombre y por hash del dominio. Responde `204`
(o `404` si no existe).

---

## Subdominios

### `POST /v1/domains/{id}/subdomains` · *Write*
Crea (o devuelve si ya existe por nombre) un subdominio.
```json
{ "name": "irpf", "description": "Impuesto sobre la renta" }
```
Respuesta:
```json
{ "id": 1, "domain_id": 1, "name": "irpf", "description": "Impuesto sobre la renta", "created_at": 1781883996775 }
```

### `GET /v1/domains/{id}/subdomains` · *Read*
Lista los subdominios del dominio → `[Subdomain, …]`.

### `DELETE /v1/domains/{id}/subdomains/{sub_id}` · *Write*
Borra el subdominio y, **en cascada**, los documentos asignados a él (con sus
chunks/embeddings/índices). Responde `204`.

---

## Labels (tags)

### `POST /v1/domains/{id}/tags` · *Write*
Crea una etiqueta.
```json
{ "name": "2026", "display_name": "Ejercicio 2026", "description": "", "parent": null }
```
Solo `name` es obligatorio. Respuesta: `Tag` = `{ id, domain_id, name, display_name, description, parent, created_at }`.

### `GET /v1/domains/{id}/tags` · *Read*
Lista las etiquetas del dominio → `[Tag, …]`.

### `PATCH /v1/domains/{id}/tags/{tag_id}` · *Write*
Edita `display_name` y/o `description` (no el `name`, que es la clave de búsqueda).
Campos omitidos se dejan igual. Cuerpo `{ "display_name": "…", "description": "…" }`;
devuelve el `Tag` actualizado.

### `DELETE /v1/domains/{id}/tags/{tag_id}` · *Write*
Borra la etiqueta y la **desasocia** de chunks y documentos (que **no** se borran:
las labels son transversales). Responde `204`.

> En la ingesta puedes pasar labels **por nombre** (`labels`) y se crean solas; no hace
> falta usar este endpoint salvo para definir jerarquías o descripciones.

---

## Ingesta

La ingesta es **asíncrona**: devuelve `job_id`. Consulta el estado en `/v1/jobs/{id}`.
Puedes pasar `subdomain` (nombre) y `labels` (nombres) y se **crean si no existen**.

### `POST /v1/domains/{id}/documents` · *Write*
Texto o chunks ya troceados (JSON).
```json
{
  "title": "Nota IVA 2025",
  "source": "intranet",
  "subdomain": "iva",
  "labels": ["2025", "iva"],
  "metadata": { "autor": "AEAT" },
  "text": "Tipos del IVA en 2025: general 21%, reducido 10%, superreducido 4%."
}
```
- Usa **`text`** (el motor trocea) **o** `chunks: ["…","…"]` (ya troceado).
- `tags` (ids existentes) sigue admitido por compatibilidad; preferible `labels` (nombres).
- `source`, `metadata`, `subdomain`, `labels`, `tags` son opcionales.

Respuesta (`duplicate: true` y `job_id: 0` si ese contenido ya estaba ingestado en
el dominio — **deduplicación por hash de contenido**):
```json
{ "document_id": 2, "job_id": 2, "duplicate": false }
```

### `POST /v1/domains/{id}/documents/batch` · *Write*
Ingesta **varios documentos** en una petición: el cuerpo es un **array** de objetos
con la misma forma que `/documents`. Cada uno se deduplica por separado. Responde un
array de `IngestResp` en el mismo orden.
```json
[ {"title":"a","text":"…"}, {"title":"b","chunks":["…","…"]} ]
```

### `POST /v1/domains/{id}/files` · *Write*
Sube un **fichero crudo**; Nucleus extrae el texto **dentro del motor**. El cuerpo son
los bytes; los metadatos van en *query string*.

Parámetros de query:

| Param | Obligatorio | Descripción |
|-------|-------------|-------------|
| `filename` | sí | Nombre con extensión (selecciona el extractor). |
| `title` | no | Por defecto, el `filename`. |
| `subdomain` | no | Nombre de subdominio (se crea si no existe). |
| `labels` | no | Nombres de labels separados por coma (se crean). |
| `tags` | no | Ids de tag existentes separados por coma. |

```bash
curl -X POST "$BASE/v1/domains/1/files?filename=IRPF_2026.pdf&subdomain=irpf&labels=2026,irpf" \
  -H "Authorization: Bearer $TOKEN" --data-binary @IRPF_2026.pdf
```
Respuesta:
```json
{ "document_id": 1, "job_id": 1, "chars": 23816 }
```

**Formatos soportados**: `txt`, `md`, `csv`, `tsv`, `log`, `html`/`htm`/`xhtml`, `pdf`,
`xlsx`/`xlsm`/`xlsb`/`xls`/`ods`, `docx`. El `.doc` binario heredado **no** está
soportado (convierte a `.docx`/`.pdf`). Límite de subida: 64 MB.

---

## Documentos

### `GET /v1/domains/{id}/documents?offset=&limit=` · *Read*
Lista paginada de documentos del dominio → `[Document, …]` (`limit` por defecto 50,
máx. 500).

### `GET /v1/documents/{id}` · *Read* (sobre el dominio del documento)
Devuelve el `Document`.

### `DELETE /v1/documents/{id}` · *Write*
Borra el documento y todos sus chunks/embeddings/índices. Responde `204`.

### `PATCH /v1/documents/{id}` · *Write*
Re-asigna labels y/o subdominio (propagado a sus chunks) sin re-ingestar.
```json
{ "labels": ["revisado", "2026"], "subdomain": "irpf" }
```
- `labels` (nombres, se crean) y/o `tags` (ids) presentes → **reemplazan** el conjunto.
- `subdomain` presente → mueve el documento (se crea si no existe); cadena vacía lo
  desasigna; **omitirlo** lo deja igual.
- Devuelve el `Document` actualizado.

---

## Chunks

### `GET /v1/chunks/{id}` · *Read*
Devuelve el `Chunk` = `{ id, document_id, domain_id, subdomain_id, ordinal, text, tags, metadata, prev, next }`.

### `GET /v1/chunks/{id}/context?before=N&after=M` · *Read*
Devuelve el chunk con hasta `N` vecinos anteriores y `M` posteriores (encadenados),
en orden de documento → `[Chunk, …]`. Por defecto `before=1`, `after=1`.

---

## Búsqueda

### `POST /v1/domains/{id}/search` · *Read*
```json
{
  "query": "tipos de retención de IRPF en 2026",
  "k": 5,
  "subdomain": "irpf",
  "tags": [1, 2],
  "match_all": false,
  "document_ids": [10],
  "filter": "tag:2026 AND NOT tag:borrador",
  "diversity": 0.3
}
```

| Campo | Descripción |
|-------|-------------|
| `query` | Texto a embeber con el modelo del dominio. |
| `query_vector` | Alternativa a `query`: vector precomputado (`[f32]`, debe coincidir con `dim`). |
| `k` | Nº de resultados (por defecto 10). |
| `subdomain` | Acota a un subdominio (por **nombre**). Si no existe → resultados vacíos. |
| `tags` | Acota a chunks con estas labels (ids). |
| `match_all` | Si `true`, exige **todas** las `tags`; si no, cualquiera. |
| `document_ids` | Acota a estos documentos. |
| `filter` | Expresión del [lenguaje de consulta](lenguaje-consulta.md). |
| `diversity` | Diversidad de resultados (MMR) en `[0, 1]`. `0` (defecto) = orden por relevancia pura; subirlo penaliza chunks redundantes entre sí. |

Los filtros presentes se **intersecan**. Respuesta: lista de hits rankeados.
`snippet` es un extracto centrado en el término que casa (se omite en búsquedas
solo-vector); `domain_id` distingue resultados en búsquedas multi-dominio:
```json
[
  { "chunk_id": 1, "document_id": 1, "domain_id": 1, "score": 0.927,
    "text": "Tabla de tipos de retención de IRPF…",
    "snippet": "…tipos de retención de IRPF para 2026…",
    "tags": [1, 2], "metadata": { "filename": "IRPF_2026.pdf" } }
]
```

### `POST /v1/search` · *Read (en cada dominio)*
Busca en **varios dominios del mismo modelo** y fusiona por score. Los filtros por
id (tags, document_ids, subdomain) no aplican entre dominios; usa `filter` (por
nombre de tag).
```json
{ "domain_ids": [1, 2], "query": "contrato laboral", "k": 5, "diversity": 0.2 }
```
Respuesta: misma forma que la búsqueda por dominio (cada hit lleva su `domain_id`).

**Ranking híbrido.** Cada búsqueda fusiona el índice **vectorial** (semántico) con el
**léxico BM25** (términos literales) mediante RRF, de modo que tanto un sinónimo como una
cita exacta (un código, un artículo) recuperan el chunk. Si el servidor arranca con
`NUCLEUS_RERANK_MODEL`, se añade una etapa final de **reranking** (*cross-encoder*) que
reordena los mejores candidatos. El `score` refleja la última etapa aplicada (RRF, o el
*cross-encoder* si el reranking está activo), por lo que su escala no es comparable entre
configuraciones. Ver [configuración](configuracion.md#búsqueda-híbrida-y-reranking).

---

## Jobs

### `GET /v1/jobs?offset=&limit=` · *Admin*
Lista paginada de jobs (resumen).

### `GET /v1/jobs/{id}` · *autenticado*
Estado de un job de ingesta:
```json
{ "id": 1, "status": "Done", "attempts": 1, "error": null }
```
`status` ∈ `Pending` | `Running` | `Done` | `Failed`.

---

## Tokens

### `POST /v1/tokens` · *Admin*
```json
{
  "name": "app-lectura",
  "scopes": [ { "domain": { "One": 1 }, "perm": "Read" } ],
  "expires_at": null
}
```
Respuesta (el `token` se muestra **una sola vez**):
```json
{ "id": 2, "name": "app-lectura", "token": "nuc_…" }
```

### `GET /v1/tokens` · *Admin*
Lista metadatos de tokens (sin el secreto). `last_used_at` es el último uso con
éxito (en memoria; `null` si no se ha usado desde el arranque):
```json
[ { "id": 1, "name": "bootstrap-admin", "scopes": [ { "domain": "All", "perm": "Admin" } ],
    "created_at": 1781883600000, "expires_at": null, "last_used_at": 1781884000000 } ]
```

### `DELETE /v1/tokens/{id}` · *Admin*
Revoca un token. Responde `204` (idempotente).

### `POST /v1/tokens/{id}/rotate` · *Admin*
Rota el secreto: mismo id/scopes/expiración, **nuevo** secreto (invalida el viejo).
Devuelve el nuevo `token` (mostrado **una sola vez**), igual que al crearlo.

---

## Mantenimiento

### `POST /v1/maintenance/persist` · *Admin*
Vuelca a disco los índices persistibles (HNSW). Responde:
```json
{ "persisted": 3 }
```

### `POST /v1/domains/{id}/reindex` · *Admin*
Re-embebe todos los chunks del dominio y reconstruye su índice, como **job** en
background. Con `{ "model": "bge-small-en-v1.5" }` cambia el modelo del dominio (y
la dimensión); sin cuerpo, re-embebe con el modelo actual. Responde `{ "job_id": N }`.

---

## Copias de seguridad

Ver [operación](operacion.md#backups-y-restauración) para el detalle. Todos *Admin*.

### `POST /v1/backups` — `{ "kind": "full" | "differential" }`
Toma una copia ahora y purga full antiguos según la retención. Responde el registro:
```json
{ "id": "2026-06-21_17-55-47-996-full", "kind": "Full", "created_at": 1782064547996,
  "parent": null, "file": "2026-06-21_17-55-47-996-full.redb", "bytes": 3686400 }
```

### `GET /v1/backups`
Catálogo de copias (más antiguas primero).

### `POST /v1/backups/restore` — `{ "id": "<backup-id>" }`
Toma una copia de seguridad del estado actual, reconstruye la copia elegida y **cambia el
motor en caliente**. Responde `{ "restored": "...", "active_db": "..." }`.

### `GET` / `POST /v1/backups/schedule`
Lee/actualiza la programación en caliente:
```json
{ "enabled": true, "interval_secs": 21600, "full_every": 7, "keep_fulls": 7 }
```
Con índice `flat` devuelve `{"persisted":0}` (no persiste; se reconstruye).
