# Nucleus — Roadmap

Este documento sustituye a la antigua sección "Próximos pasos" del README y la
amplía. Agrupa el trabajo pendiente por **ejes** y marca el **estado** de cada
ítem para ser honestos sobre qué está hecho, qué está en curso y qué es un
proyecto en sí mismo.

Leyenda de estado:

- ✅ **Hecho** — implementado y con tests en este repo.
- 🟡 **En curso / parcial** — base puesta, falta cerrar.
- ⬜ **Pendiente** — diseñado pero no empezado.
- 🔭 **Investigación** — requiere prototipo/medición antes de comprometer alcance.

La prioridad (P1 = siguiente, P3 = a futuro) refleja impacto sobre el "foso"
(calidad de recuperación llave en mano) y sobre la operación real, no dificultad.

---

## Eje 1 — Calidad de recuperación (el foso)

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ✅ | P1 | **Chunking por frontera (frase/palabra)** | El chunker dejaba de cortar a mitad de palabra: ahora retrocede al último límite de oración/espacio dentro de la ventana. Mantiene el solape y el tamaño objetivo. Ver [`chunking.rs`](../crates/core/src/chunking.rs). |
| ✅ | P1 | **Diversidad de resultados (MMR)** | `search` admite `diversity` ∈ [0,1]: re-ordena los candidatos con Maximal Marginal Relevance para no devolver 5 fragmentos casi idénticos. |
| ⬜ | P1 | **Snippets / highlighting** | Devolver el span del chunk que casó con la query (offsets léxicos), no solo el texto completo. |
| ⬜ | P2 | **Pre-filtrado en HNSW** | Hoy el filtro (tags/meta/subdominio) se cruza con los candidatos *después* de la búsqueda ANN; con filtros muy selectivos eso puede dejar `k` corto. Hace falta filtrado durante el recorrido del grafo o sobre-fetch adaptativo. |
| ⬜ | P2 | **Reindexado / cambio de modelo** | El modelo de embeddings se fija al crear el dominio y es inmutable de facto. Añadir un `JobKind::Reindex` que re-chunkee y re-embeba un dominio (a otro modelo/estrategia) sin borrar y re-subir. |
| ⬜ | P3 | **Auto-inducción de subdominios/labels** | Clustering + reglas, **sin LLM**. Era el "siguiente paso" original; sigue siendo el techo de calidad, pero el catálogo+retrieval provisto por el usuario es la vía pragmática mientras tanto. |

## Eje 2 — Operación y seguridad

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ✅ | P1 | **Borrado en cascada** | `DELETE` de dominio (arrastra subdominios, documentos, chunks, embeddings, tags e índices), de subdominio (arrastra sus documentos) y de label (lo desasocia de los chunks sin borrar documentos). |
| ✅ | P1 | **Updates (rename / edición)** | `PATCH` de dominio (renombrar) y de tag (`display_name`/`description`). |
| ✅ | P1 | **Rate limiting** | Token-bucket en memoria por cliente (IP), configurable por env (`NUCLEUS_RATE_LIMIT_RPS`, `NUCLEUS_RATE_LIMIT_BURST`), apagado por defecto. |
| ⬜ | P2 | **Observabilidad** | Trazas distribuidas (OpenTelemetry/OTLP) además de `/metrics`; histogramas de latencia (p50/p95/p99) en vez de solo sumas; logs estructurados de auditoría de acceso. |
| ⬜ | P2 | **Rotación / auditoría de tokens** | `last_used_at`, rotación sin corte, y un registro de accesos por token. |
| ⬜ | P3 | **Backups remotos (S3 / object storage)** | Hoy los backups son locales. Subir snapshots/deltas a almacenamiento de objetos con la misma política de retención. |

## Eje 3 — Escala

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ⬜ | P2 | **mmap del grafo HNSW** | Cargar el índice por mmap en vez de mantenerlo entero en RAM. |
| ⬜ | P2 | **Cuantización de vectores (PQ / scalar)** | Bajar la RAM del índice (límite reconocido "índice en memoria") a cambio de algo de exactitud. |
| ⬜ | P3 | **Persistencia incremental del índice** | Hoy `persist` vuelca el índice entero; volcar solo el delta. |
| 🔭 | P3 | **Workers de jobs distribuidos / multi-nodo** | redb es single-writer: multi-nodo implica repensar el almacenamiento (sharding/réplicas). Es un cambio de alcance, no una mejora incremental; sólo si se persigue un SaaS. |

## Eje 4 — API y contrato

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ✅ | P1 | **CRUD completo** | Cubierto por el eje de Operación (deletes + patches). Pendiente: mover un documento de subdominio y re-etiquetado masivo. |
| ⬜ | P2 | **Re-etiquetado / re-asignación de documentos** | Añadir/quitar labels o cambiar el subdominio de un documento sin re-ingestar. |
| ⬜ | P2 | **Webhooks / eventos de job** | Notificar al cliente cuando un job de ingesta termina, en vez de hacer polling de `/v1/jobs/{id}`. |
| ⬜ | P3 | **Búsqueda multi-dominio** | Consultar varios dominios del mismo modelo en una sola petición. |

## Eje 5 — Ingesta y formatos

| Estado | Prioridad | Ítem | Notas |
|--------|-----------|------|-------|
| ⬜ | P2 | **OCR de PDFs escaneados** | Hoy un PDF imagen no extrae texto. Integrar OCR (p. ej. tesseract) tras detectar páginas sin texto. |
| ⬜ | P3 | **PDFs cifrados / `.doc` heredado** | Soportar PDFs con contraseña (cuando se aporta) y el binario `.doc` antiguo (hoy solo `.docx`). |
| ⬜ | P3 | **Ingesta por lotes / streaming** | Subir múltiples ficheros o un stream en una sola operación. |

---

## Hecho recientemente

Los siguientes ítems se han implementado en esta iteración (ver el diff y los
tests asociados):

- **Chunking por frontera** — `crates/core/src/chunking.rs`.
- **MMR (diversidad)** — `crates/core/src/engine.rs` (`SearchRequest.diversity`).
- **Borrado en cascada y updates** — `crates/core/src/storage/mod.rs`,
  `crates/core/src/engine.rs`, `crates/server/src/routes.rs`.
- **Rate limiting** — `crates/server/src/ratelimit.rs`.

El estado "Hardening hecho" previo (búsqueda híbrida + RRF, reranking opcional,
jobs durables, backups full/diferencial con restore en caliente, auth por token,
load-shed, `/healthz`+`/readyz`+`/metrics`, Docker + CI) sigue vigente y es la
base sobre la que se construye este roadmap.
