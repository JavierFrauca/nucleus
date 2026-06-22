# Nucleus — Roadmap

Este documento amplía la antigua sección "Próximos pasos" del README. Agrupa el
trabajo por **ejes** y marca el **estado** de cada ítem para ser honestos sobre
qué está hecho, qué está parcial y qué se difiere (con su porqué).

Leyenda de estado:

- ✅ **Hecho** — implementado y con tests en este repo.
- 🟡 **Parcial** — la parte realizable en el motor está hecha; falta una pieza que depende de infra/decisión externa.
- ⬜ **Diferido** — requiere una dependencia pesada (binario nativo, SDK, credenciales) o un rediseño; ver "Diferidos y por qué".
- 🔭 **Fuera de alcance del motor** — choca con una decisión de diseño fundamental (single-writer); sería otro producto.

Prioridad: P1 = siguiente, P3 = a futuro.

---

## Eje 0 — Velocidad (single-node)

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ✅ | P1 | **Caché de embedding de query (LRU)** | El cuello medido es embeber la query en CPU; una LRU en memoria (con la inferencia **fuera del lock**) sirve consultas repetidas sin re-embeber. `NUCLEUS_QUERY_CACHE`, off por defecto. [`engine.rs`](../crates/core/src/engine.rs). |
| ⬜ | P3 | **Caché de resultados** | Cachear hits por `(dominio, request)` invalidando con una "generación" por dominio en cada escritura. **Baja prioridad ahora**: la medición propia dice que el cuello es el *embedding* (ya cacheado), no la fusión/BM25; este caché solo añade valor con **reranking** activo (caro) y trae riesgo de staleness. |
| ✅ | P2 | **Modelo precacheado en la imagen Docker** | El build descarga el modelo por defecto y lo hornea en la imagen (`PREFETCH_MODEL=true`); el primer arranque no descarga 450 MB. [`Dockerfile`](../Dockerfile), [`prefetch_model.rs`](../crates/core/examples/prefetch_model.rs). |
| ⬜ | P2 | **GPU (CUDA) para embedding/rerank** | Rompe el techo de throughput (DirectML no ayudó; CUDA sí). |

## Eje 1 — Calidad de recuperación (el foso)

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ✅ | P1 | **Chunking por frontera (frase/palabra)** | Retrocede al último fin de oración/espacio dentro de la ventana. [`chunking.rs`](../crates/core/src/chunking.rs). |
| ✅ | P1 | **Diversidad (MMR)** | `diversity` ∈ [0,1] en la búsqueda. [`engine.rs`](../crates/core/src/engine.rs). |
| ✅ | P1 | **Snippets / highlighting** | Cada hit devuelve `snippet`: extracto centrado en el primer término que casa, elidido con `…`. |
| ✅ | P2 | **Pre-filtrado en HNSW** | Con filtro selectivo, el HNSW hace **over-fetch adaptativo** (duplica el fetch hasta cubrir `k` o agotar el grafo), evitando quedarse corto. [`hnsw.rs`](../crates/core/src/index/hnsw.rs). |
| ✅ | P2 | **Reindexado / cambio de modelo** | `JobKind::Reindex` re-embebe los chunks de un dominio (opcionalmente con otro modelo→dim) y reconstruye el índice. `POST /v1/domains/{id}/reindex`. |
| ⬜ | P3 | **Auto-inducción de subdominios/labels** | Clustering + reglas, sin LLM. Ver "Diferidos". |

## Eje 2 — Operación y seguridad

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ✅ | P1 | **Borrado en cascada** | `DELETE` de dominio (cascada total), subdominio (cascada docs) y label (desasocia). |
| ✅ | P1 | **Updates** | `PATCH` de dominio (rename) y de tag (display/desc). |
| ✅ | P1 | **Rate limiting** | Token-bucket por IP, `NUCLEUS_RATE_LIMIT_RPS`/`_BURST`, off por defecto. |
| ✅ | P2 | **Dashboard web** | Panel autocontenido en `GET /` (mismo origen): dominios, ingesta (texto/fichero), búsqueda con snippets/MMR y jobs. [`dashboard.html`](../crates/server/src/dashboard.html). |
| ✅ | P2 | **Rotación de tokens + last_used** | `POST /v1/tokens/{id}/rotate`; `last_used_at` en el listado (en memoria, no en disco para no penalizar el hot path de auth). |
| 🟡 | P2 | **Observabilidad** | **Histograma de latencia** de búsqueda (p50/p95/p99) en `/metrics` (formato Prometheus). Falta exportador **OpenTelemetry/OTLP** — ver "Diferidos". |
| ⬜ | P3 | **Backups remotos (S3 / object storage)** | Hoy local. Ver "Diferidos". |

## Eje 3 — Escala

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ⬜ | P2 | **mmap del grafo HNSW** | El grafo ya se persiste/recarga por fichero; el mmap puro depende de soporte en `hnsw_rs`. Ver "Diferidos". |
| ✅ | P2 | **Cuantización escalar (int8)** | `NUCLEUS_INDEX=sq`: índice exacto-recall con codes int8 → **~4× menos RAM** que `flat`, error de cuantización despreciable. [`sq.rs`](../crates/core/src/index/sq.rs). **PQ** (product quantization, codebooks) queda como paso siguiente para ratios mayores. |
| ⬜ | P3 | **Persistencia incremental del índice** | Hoy se vuelca entero. |
| 🔭 | P3 | **Workers distribuidos / multi-nodo** | redb es single-writer; es otro producto. Ver "Diferidos". |

## Eje 4 — API

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ✅ | P1 | **CRUD completo** | Deletes + patches de dominios/subdominios/labels. |
| ✅ | P2 | **Re-etiquetado / reasignación de documentos** | `PATCH /v1/documents/{id}`: cambia labels y/o subdominio (propagado a los chunks) sin re-ingestar. |
| ✅ | P3 | **Búsqueda multi-dominio** | `POST /v1/search` sobre varios dominios del **mismo modelo**, fusionando por score. |
| ✅ | P2 | **Ingesta por lotes** | `POST /v1/domains/{id}/documents/batch` (array de documentos, dedupe por hash por ítem). |
| ⬜ | P2 | **Webhooks / eventos de job** | Notificar al terminar un job en vez de hacer polling. Ver "Diferidos". |

## Eje 5 — Ingesta y formatos

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ✅ | P2 | **Ingesta por lotes** | (ver eje 4). |
| ⬜ | P2 | **OCR de PDFs escaneados** | Necesita binario nativo. Ver "Diferidos". |
| ⬜ | P3 | **PDFs cifrados / `.doc` heredado** | Ver "Diferidos". |
| ⬜ | P3 | **Ingesta en streaming** | Subida por stream/multipart de muchos ficheros. |

---

## Diferidos y por qué

Estos ítems **no** se implementan en el repo del motor porque requieren
infraestructura externa, dependencias pesadas o un cambio arquitectónico que
excede "una mejora del motor". Se documentan para que la decisión sea explícita:

- **Multi-nodo / workers distribuidos (🔭).** redb es *single-writer* (un proceso
  escribe). Multi-nodo implica sustituir el almacenamiento por uno con
  sharding/réplicas y repensar la consistencia: es **otro producto**, no una
  mejora incremental. Para esa escala, la propia comparativa del proyecto remite
  a Qdrant. Mantener el alcance "un nodo, millones de chunks" es una decisión, no
  un descuido.
- **OCR de PDFs escaneados.** Requiere un motor OCR (p. ej. Tesseract) como
  **binario nativo** del sistema o un modelo ONNX adicional; añade superficie de
  build y despliegue. Encaja como *feature* opcional (`--features ocr`) tras
  detectar páginas sin texto, no en el núcleo.
- **Backups remotos (S3).** Necesita el SDK de object storage y **credenciales**
  para probarse de extremo a extremo; es una integración de despliegue. El
  `BackupManager` ya produce ficheros (full + delta) listos para subir; falta el
  *sink* remoto, que se añade como capa sin tocar el formato.
- **Cuantización PQ / scalar.** Es un **índice nuevo** (entrenar codebooks,
  recall vs. memoria) detrás del trait `VectorIndex`; trabajo de calidad medible
  por sí mismo, no un parche.
- **mmap del grafo HNSW.** Depende de que `hnsw_rs` exponga carga por mmap; hoy
  el grafo se recarga por fichero (sidecar) al arrancar, que ya evita reconstruir.
- **OpenTelemetry / OTLP.** El histograma de latencia ya está en `/metrics`
  (Prometheus, sin dependencias). El tracing distribuido añade el exporter OTLP y
  un colector; es una integración de plataforma.
- **Webhooks de job.** Requiere salida HTTP configurable y reintentos; sencillo
  pero es acoplamiento con sistemas del cliente. El estado del job ya es
  consultable por `/v1/jobs/{id}`.
- **Auto-inducción de subdominios/labels.** Clustering (k-means/HDBSCAN) + reglas
  sin LLM; la calidad no generaliza entre verticales y el catálogo provisto por
  quien ingesta es la vía pragmática. Es una línea de investigación abierta.

---

## Hecho en esta iteración

Calidad: chunking por frontera, MMR, **snippets**, **pre-filtrado HNSW adaptativo**,
**reindexado/cambio de modelo**. Operación: cascada, updates, rate limiting,
**rotación de tokens + last_used**, **histograma de latencia**. API: CRUD completo,
**PATCH de documento**, **búsqueda multi-dominio**, **ingesta por lotes**.

Verificado con `cargo test --workspace` (66 tests), `cargo clippy --all-targets -- -D warnings`
y `cargo fmt --all --check`, todo limpio.
