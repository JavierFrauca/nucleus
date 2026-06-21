# Arquitectura

## Workspace

Dos crates:

```
crates/
├── core/    (nucleus-core)   — librería del motor, sin dependencias HTTP
└── server/  (nucleus-server) — binario HTTP (axum) sobre nucleus-core
```

### `nucleus-core`

| Módulo | Responsabilidad |
|--------|-----------------|
| `error` | `NucleusError` + alias `Result`. Errores grandes (redb, bincode) van *boxed* para que `Result` sea barato. |
| `id` | Newtypes de id (`DomainId`, `SubdomainId`, `DocumentId`, `ChunkId`, `TagId`, `JobId`, `TokenId`). |
| `model` | Entidades: `Domain`, `Subdomain`, `Document`, `Chunk`, `Tag`. |
| `storage` | Persistencia con **redb** + códec **bincode 2.x**; tablas e índices secundarios; CRUD; get-or-create por nombre. |
| `index` | `trait VectorIndex` + `FlatIndex` (coseno exacto) y `HnswIndex` (aproximado, persistente). `build_index(kind, dim)`. |
| `embed` | `trait Embedder` + `LocalEmbedder` (fastembed/ONNX, in-process) + `MockEmbedder` (tests). |
| `chunking` | `trait Chunker` + `FixedSizeChunker` (ventana de caracteres con solapamiento). |
| `extract` | Extracción de texto multi-formato (pdf, docx, xlsx/xls, html, md/txt…). |
| `query` | Lenguaje de consulta: lexer + parser + evaluación (`tag:`/`doc:`/`meta.*`, AND/OR/NOT). |
| `jobs` | Cola persistida en redb + workers tokio; `JobQueue`, `JobKind`. |
| `auth` | `ApiToken`, `Scope`, hashing/verificación. |
| `engine` | **Fachada** que une todo: crear dominios/subdominios/labels, ingestar, buscar, tokens, persistir índices. |

### `nucleus-server`

| Fichero | Responsabilidad |
|---------|-----------------|
| `main` | Configuración (env), `tracing`, construcción del `Engine` + `JobQueue`, *bootstrap* del token admin, `axum::serve` con apagado ordenado. |
| `app` | `AppState`, `Config`, mapeo de `NucleusError` → respuestas HTTP, extractor de token `Auth`, helper `blocking`. |
| `routes` | DTOs y handlers de todos los endpoints + el router. |

## Almacenamiento (redb)

Un único fichero `.redb` (ACID). Tablas principales (clave `u64`, valor bincode):
`domains`, `subdomains`, `documents`, `chunks`, `embeddings`, `tags`, `tokens`, `jobs`,
`seq` (secuencias de id). Índices secundarios (multimap) para no escanear:
`docs_by_domain`, `chunks_by_domain`, `chunks_by_doc`, `chunks_by_tag`,
`chunks_by_meta`, `chunks_by_subdomain`, `tags_by_domain`, `subdomains_by_domain`, e
índices por nombre `tag_ids` y `subdomain_ids` (`"dominio\u{1f}nombre" -> id`) para
get-or-create.

## Flujo de ingesta

```
POST /files (bytes)  ──>  extract::extract_text (in-engine, spawn_blocking)
POST /documents (text/chunks)
        │
        ├─ resolver subdomain + labels por nombre (get-or-create)
        ├─ create_document_record  (fila del documento; rápido)
        └─ JobQueue.enqueue_ingest ──> job persistido (devuelve job_id)

worker (tokio):
   chunking ─> embed_documents (en VENTANAS de 64) ─> insert_chunk (+índices) ─> upsert índice vectorial ─> link_chunks (prev/next)
```

La inferencia corre en `spawn_blocking` (CPU-bound). El embedding por ventanas acota la
memoria independientemente del tamaño del documento.

## Flujo de búsqueda

```
POST /search
   │
   ├─ vector de consulta: embed_query(texto)  ó  query_vector dado
   ├─ conjunto candidato (intersección): tags ∩ document_ids ∩ subdomain ∩ filter(query language)
   ├─ VectorIndex.search(query, k, candidatos) ─> top-k (coseno)
   └─ cargar los chunks ─> Hits
```

El filtro `filter` se resuelve por álgebra de conjuntos sobre los índices (no escanea
chunks). El universo (chunks del dominio) se usa para `NOT`.

## Decisiones de diseño

- **Todo dentro**: extracción + chunking + embeddings + índice, sin servicios externos.
  El LLM de respuesta lo pone el cliente; Nucleus no incrusta un LLM generativo.
- **redb + bincode** en lugar de hand-rollear durabilidad; el índice vectorial es
  derivado y reconstruible.
- **`VectorIndex` como trait**: `flat` exacto por defecto, `hnsw` opcional para escala.
- **Embeddings in-process** (fastembed) como diferenciador y para privacidad/on-prem.
- **Estructura aportada por el cliente** (dominio/subdominio/labels por nombre); la
  auto-inducción sin LLM (clustering + reglas) es una capa opcional futura.

## Estado y hoja de ruta

- **Hecho**: ingesta multi-formato transparente, embeddings in-process, índice
  flat/HNSW (HNSW persistente), jobs persistidos, auth por token con scopes, lenguaje de
  consulta con push-down por índices, dominios/subdominios/labels por nombre, contexto
  de vecinos, soporte GPU opcional.
- **Siguiente (opcional)**: auto-inducción de subdominios (clustering de embeddings con
  centrado) y labels (reglas + zero-shot) **sin LLM**; búsqueda híbrida léxico+vector;
  reranking in-process; mmap del grafo HNSW; workers distribuidos.
