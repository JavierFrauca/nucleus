# Nucleus — Dossier técnico (guía de defensa)

Documento de referencia para explicar y **defender** el proyecto: qué es, cómo está
construido, qué decisiones se tomaron y por qué, qué no hace, y respuestas a las preguntas
incómodas. Pensado para tenerlo abierto en una revisión con un colega.

---

## 1. En una frase

Nucleus es un **motor de base de datos llave en mano para RAG**: subes documentos crudos y
preguntas en lenguaje natural; el motor **extrae, trocea, embebe, indexa y recupera** — todo
dentro del proceso, sin servicios externos de embeddings ni pipeline que montar.

**El foso (diferenciación):** no es "otro vector DB". Reduce el hueco entre una base de datos
y un RAG que funciona. Tres patas:
1. **Embeddings in-process** (no hay servicio externo; privacidad total / on-prem).
2. **Ingesta transparente multi-formato** dentro del motor (PDF/DOCX/XLSX/HTML/MD/TXT).
3. **Recuperación de calidad** lista: híbrido (vector + BM25) + reranking opcional, con
   dominios/subdominios/labels como ejes.

---

## 2. Arquitectura de alto nivel

```
                         ┌─────────────────────── nucleus-server (axum) ───────────────────────┐
   Cliente HTTP  ───────▶│  routes.rs   (handlers, DTOs, auth, load-shed)                       │
   (C#/JS/curl)          │  app.rs      (AppState, Config, métricas, ApiError)                  │
                         │  main.rs     (wiring, scheduler de backups, shutdown, hot-swap)      │
                         └───────────────┬─────────────────────────────────────────────────────┘
                                         │ Arc<EngineHandle>  (RwLock<Arc<Engine>>, swap en restore)
                         ┌───────────────▼──────────────────── nucleus-core ───────────────────┐
                         │  Engine (fachada síncrona)                                            │
                         │   ├─ Storage (redb + bincode)  ── tablas + índices secundarios        │
                         │   ├─ Embedder (fastembed/ONNX) ── e5-small in-process                 │
                         │   ├─ VectorIndex (flat | hnsw) en memoria, por dominio                │
                         │   ├─ LexicalIndex (BM25) en memoria, por dominio                      │
                         │   ├─ Reranker (cross-encoder, opcional)                               │
                         │   ├─ Chunker, extract, query (DSL), auth, util                        │
                         │   ├─ JobQueue (cola persistida en redb + workers tokio)               │
                         │   └─ BackupManager (full + diferencial bsdiff)                        │
                         └───────────────────────────────────────────────────────────────────────┘
                                         │
                                  ┌──────▼──────┐
                                  │  nucleus.redb│  (única fuente de verdad, ACID)
                                  └─────────────┘
```

**Regla mental:** todo el estado persistente vive en el `.redb`. Los índices (vector y BM25)
son **en memoria** y se **reconstruyen** desde el `.redb` al arrancar (el flat) o se cargan de
sidecar (HNSW). Los embeddings se guardan aparte del texto para cargar el índice sin arrastrar
el texto.

---

## 3. Workspace y ficheros principales

Workspace Cargo con dos crates (separar el motor de la capa HTTP permite testear el núcleo
aislado y publicar `nucleus-core` como librería):

```
crates/core/   (nucleus-core)  — librería, sin axum
  src/
    lib.rs        re-exports públicos
    engine.rs     ★ Engine (fachada): ingest/search/admin + EngineHandle (hot-swap) + RRF
    storage/mod.rs ★ redb: tablas, índices secundarios, CRUD atómico, backup_to (snapshot lógico)
    storage/codec.rs  bincode 2 (modo serde) encode/decode
    error.rs      NucleusError (thiserror) + Result
    id.rs         newtypes u64 tipados (DomainId, DocumentId, ChunkId, TagId, …)
    model/        domain, subdomain, document, chunk, tag
    embed/        Embedder (trait) + LocalEmbedder (fastembed) + MockEmbedder (tests)
    index/        VectorIndex (trait) + flat (coseno exacto) + hnsw (aprox.) + lexical (BM25)
    rerank.rs     Reranker (trait) + LocalReranker (cross-encoder) + MockReranker
    batch.rs      EmbedBatcher (micro-batching opcional de embeddings de query)
    chunking.rs   Chunker + FixedSizeChunker (1000 chars, solape 200)
    extract/mod.rs extracción multi-formato (pdf/docx/xlsx/html/md/txt/csv)
    query/mod.rs  lexer + parser recursivo del DSL de filtros + evaluación por conjuntos
    jobs/mod.rs   JobQueue (cola persistida + workers), JobKind (Ingest/Delete)
    auth.rs       ApiToken, Scope, hashing SHA-256, verificación
    backup.rs     ★ BackupManager (full snapshot + diferencial bsdiff + catálogo + restore)
    util.rs       now_millis, sha256_hex, format_utc (timestamps sin chrono)
  examples/       rerank_ab (benchmark), ingest_fiscal (carga corpus), mint_token (admin)

crates/server/  (nucleus-server)  — binario axum
  src/
    main.rs       wiring, scheduler de backups, graceful shutdown, resolución de DB activa
    app.rs        AppState, Config (env), Metrics, ApiError→HTTP, extractor Auth
    routes.rs     router + todos los handlers + DTOs + tests e2e

docs/             instalacion, guia-rapida, conceptos, configuracion, api, openapi.yaml,
                  lenguaje-consulta, operacion, rendimiento, arquitectura, este dossier
clients/csharp/   SDK .NET (NuGet-ready)
clients/typescript/ SDK JS/TS (npm-ready)
packaging/        build-release.ps1/.sh, install.ps1, nucleus.service, README
scripts/loadtest.mjs  test de carga (Node, sin deps)
Dockerfile · docker-compose.yml · .github/workflows/ci.yml
```

Los ★ son los que más probablemente te preguntarán.

---

## 4. Modelo de datos

Jerarquía: **dominio → subdominio → documento → chunk**, con **labels (tags)** transversales.

- **Domain**: namespace que segmenta la base. Fija el **modelo de embeddings** (y por tanto la
  dimensión) y tiene su propio índice vectorial y vocabulario de tags. (`id, name, model, dim, created_at`)
- **Subdomain**: tema concreto dentro de un dominio. (`id, domain_id, name, description, created_at`)
- **Document**: `id, domain_id, subdomain_id?, title, source?, metadata(map), tags[], created_at`.
- **Chunk**: unidad recuperable. `id, document_id, domain_id, subdomain_id?, ordinal, text,
  tags[], metadata, prev?, next?`. El **vector se guarda aparte** (tabla `embeddings`). `prev/next`
  encadenan los chunks para recuperar contexto (vecinos).
- **Tag** (label): `id, domain_id, name, display_name, description, parent?` (jerárquico).
- **IDs tipados**: newtypes sobre `u64` (`DomainId`, `ChunkId`…) para no confundir ids entre sí
  en tiempo de compilación.

**Contrato llave en mano:** el cliente pasa `subdomain` y `labels` **por nombre** en la ingesta;
el motor los crea si no existen (get-or-create). No hay que pre-crear ni manejar ids.

---

## 5. Almacenamiento (redb + bincode)

- **redb**: KV embebido, ACID, puro Rust, un solo fichero. Clave = `u64` del id; valor = bytes
  bincode de la entidad. Cada operación mutante corre en **una transacción de escritura**, así
  que las actualizaciones multi-tabla son atómicas.
- **bincode 2** (modo serde): `encode_to_vec` / `decode_from_slice` con `config::standard()`.
- **Índices secundarios** (multimap) para responder sin escanear: `docs_by_domain`,
  `chunks_by_domain`, `chunks_by_doc`, `chunks_by_tag`, `tags_by_domain`, `chunks_by_meta`
  (filtros de metadatos como lookup, no scan), `subdomains_by_domain`, `chunks_by_subdomain`.
- **Índices de nombre** para get-or-create: `subdomain_ids`, `tag_ids`, y `docs_by_hash`
  (deduplicación por hash de contenido).
- **Versión de esquema**: `META.schema_version` (=1) con gate de migración en `Storage::open`
  (rechaza abrir un DB de una versión más nueva; ejecuta migraciones de versiones viejas).
- **Inserción de chunks en 1 transacción por documento** (`insert_chunks`): encadena prev/next
  y actualiza todos los índices de golpe (atómico y rápido).

---

## 6. Embeddings in-process

- **fastembed 4** (ONNX Runtime vía `ort` 2.0-rc). Modelo por defecto **`multilingual-e5-small`**
  (384 dim) → español e inglés. También `bge-small-en-v1.5`, `all-minilm-l6-v2`.
- **Detalle e5**: requiere prefijos `query:` / `passage:`; el embedder los aplica por modelo
  automáticamente (documentos vs consulta).
- El trait `Embedder` es **síncrono** a propósito (la inferencia es CPU-bound); los llamantes la
  corren en `spawn_blocking`. `MockEmbedder` (bag-of-words determinista) permite testear sin bajar
  modelos.
- **GPU** opcional: feature `gpu` → execution provider DirectML (Windows) con fallback a CPU.
- En Windows, `ort` enlaza ONNX Runtime **estáticamente** → el binario es autocontenido (no
  necesita DLL). En Linux es `.so` dinámico (el Dockerfile/bundle la incluyen).

---

## 7. Recuperación: índices, fusión y reranking

- **Índice vectorial** (`VectorIndex` trait): `FlatIndex` (coseno exacto por fuerza bruta,
  vectores normalizados → coseno = producto punto) o `HnswIndex` (aproximado, `hnsw_rs`,
  persistente vía `ouroboros`). Uno por dominio, en memoria.
- **Índice léxico** (`LexicalIndex`): **BM25** en memoria, por dominio. Recupera coincidencias
  exactas que el denso pierde (números de ley, "art. 14", años).
- **Fusión híbrida**: las dos listas se combinan con **RRF** (Reciprocal Rank Fusion, k=60),
  robusto a las escalas de score tan distintas (y a la anisotropía del coseno de e5).
- **Reranking** (opcional): segunda etapa con un *cross-encoder* (`bge-reranker-base`) que
  re-puntúa los mejores candidatos leyendo el par `(consulta, chunk)` completo. Cota de
  candidatos configurable (`NUCLEUS_RERANK_CANDIDATES`, def. 20).
- **Lenguaje de consulta** (`filter`): `tag:`, `doc:`, `meta.k:`, con `AND`/`OR`/`NOT` y
  paréntesis; se evalúa por **álgebra de conjuntos sobre los índices secundarios** (no escanea
  ni decodifica chunks).

---

## 8. Flujo de ingesta

1. `POST /v1/domains/{id}/files?filename=…&subdomain=…&labels=…` con los **bytes crudos**.
2. El servidor extrae el texto **off-runtime** (`spawn_blocking`) según la extensión.
3. **Dedupe**: hash SHA-256 del contenido; si ya existe en el dominio, responde `duplicate`.
4. Resuelve subdominio + labels por nombre (get-or-create).
5. Crea la fila del documento y **encola un job** (responde `job_id` al instante).
6. Un worker: chunkea → embebe **en ventanas de 64 chunks** → `insert_chunks` (1 txn) →
   actualiza índice vectorial + BM25.

**Por qué ventanas de 64:** fastembed colecciona la salida de *todos* sus lotes antes de
devolver; pasarle un documento entero hace que el pico de RAM escale con el nº de chunks (un
PDF de 3,5 MB → ~4.400 chunks reventó a ~45 GB). El sub-loteo acota el pico (~estable).

---

## 9. Flujo de búsqueda

1. `POST /v1/domains/{id}/search` con `query` (texto) o `query_vector`, `k`, filtros.
2. **Load-shed**: se pide un permiso de concurrencia (semáforo); si no hay hueco en
   `NUCLEUS_SEARCH_WAIT_MS`, responde `503`.
3. Resuelve subdominio por nombre (uno inexistente → 0 resultados).
4. Embebe la query (en `spawn_blocking`, en paralelo).
5. Construye el conjunto candidato (intersección de tags/documentos/subdominio + filtro DSL
   sobre índices secundarios).
6. Recupera denso (vector) + léxico (BM25), **fusiona con RRF**.
7. Opcional: **rerank** de la ventana superior.
8. Carga los chunks y responde hits `{chunk_id, document_id, score, text, tags, metadata}`.

---

## 10. Concurrencia y rendimiento (con datos reales)

- El motor es **síncrono**; el servidor lo envuelve con `blocking()` (`spawn_blocking`) para no
  bloquear el runtime async con I/O de redb ni inferencia ONNX.
- **`EngineHandle`** = `RwLock<Arc<Engine>>`: permite cambiar el motor en caliente (restore). El
  `JobQueue` y los handlers leen el motor vivo en cada uso, así el swap los afecta a todos.
- **Locks `parking_lot`** (sin envenenamiento).

**Medido** (binario release, corpus fiscal 15.732 chunks, 12 hilos lógicos):
- Búsqueda híbrida en caliente ~**11 ms** p50; throughput pico ~**388 req/s** (CPU-bound por el
  embedding de la query). Satura hacia concurrencia ≈ núcleos.
- **RAM estable ~1 GB** bajo carga sostenida (modelo + runtime ONNX + índice); **sin fugas**.
- **0 errores y 0 resultados incorrectos** bajo concurrencia (hasta 256, sostenido 90 s) — la
  búsqueda híbrida es determinista y el test compara el top-1 contra una baseline secuencial.

**Dos palancas que se midieron y la decisión honesta:**
- **Micro-batching de embeddings**: hipótesis para subir throughput. **Empeora en CPU** (321 vs
  388 req/s, +10 ms aislado) porque serializa una etapa que ya paraleliza por núcleos. → **OFF
  por defecto**; útil solo en GPU.
- **Límite de concurrencia**: limitar a *núcleos* **estrangula ~25%** (la sobre-suscripción rinde
  más por HT/solape). Es una **válvula de seguridad** (default 16× núcleos), no un acelerador. El
  **load-shed** (límite bajo + espera corta) sí protege la p99 bajo avalancha (`503` al exceso).

Detalle no obvio: una build **dev** (sin optimizar) hace la búsqueda ~10× más lenta que release
(la parte Rust de vector/BM25). Los números de arriba son de **release**.

---

## 11. Backup y restauración

- **Full**: snapshot **consistente** del `.redb`. Es una **copia lógica vía la API de redb**
  (read-txn → nuevo Database, tabla a tabla), NO `fs::copy`: redb mantiene un lock de byte-range
  y `fs::copy` falla en Windows (error 33).
- **Diferencial**: **delta binario** (`qbsdiff`/bsdiff) del estado actual contra el último full.
  Es **full-fidelity** (incluye borrados) y para restaurar basta `full + último diferencial`
  (modelo SQL Server). Medido: un diferencial fue 476 B frente a un full de 3,7 MB.
- Los **índices no se respaldan**: se reconstruyen del `.redb` al restaurar (más simple y siempre
  consistente).
- **Programación**: tarea en background con intervalo configurable (`30m`/`6h`/`1d`/`2w`),
  política full-cada-N + diferenciales, y **retención** (purga fulls antiguos).
- **Restore en caliente**: toma una copia de seguridad del estado actual → reconstruye la copia
  elegida en un fichero nuevo → abre el motor → **`EngineHandle::swap`** (micro-corte; el job
  queue sigue el cambio) → escribe `active_db.txt` para que un reinicio reabra el restaurado.
- Endpoints admin: `POST/GET /v1/backups`, `POST /v1/backups/restore`, `GET/POST /v1/backups/schedule`.

---

## 12. Seguridad

- Tokens tipo **API-key opaca** (`nuc_` + 256 bits hex). Solo se guarda el **hash SHA-256**; el
  texto plano se muestra **una vez** (al crear). Verificación por hash, comparación + expiración.
- **Scopes** por token: `{ domain: All | One(id), perm: Read | Write | Admin }`. `Read < Write < Admin`.
- **Bootstrap**: si no hay tokens, se acuña un admin al arrancar y se escribe a fichero (no a logs).
- Endpoints admin: crear dominios, tokens, jobs (listado), backups, persist.
- Hardening: token a fichero, CORS opt-in, cotas de entrada (body 64 MB, `k` ≤ 1000), apagado
  ordenado (Ctrl-C/SIGTERM) con volcado de índices, `/healthz` + `/readyz` + `/metrics`.

---

## 13. API, contrato y distribución

- **API HTTP** con axum 0.8. Errores → HTTP: 400 (inválido/dim), 401, 403, 404, 503 (load-shed),
  500 (resto, con `tracing`).
- **Contrato OpenAPI 3** (`docs/openapi.yaml`, 21 paths) como fuente de verdad para clientes/docs.
- **SDKs**: C# (`clients/csharp`, `netstandard2.0`+`net8.0`, NuGet) y TS/JS (`clients/typescript`,
  ESM, Node y navegador). Ambos tipados, sobre la API HTTP.
- **Empaquetado**: imagen Docker (`nucleus:0.1.0`, ~197 MB, probada e2e), bundle Windows
  (exe autocontenido + instalador, probado standalone), crate `nucleus-core` publicable, scripts
  Linux + systemd. CI (fmt + clippy `-D warnings` + test).

---

## 14. Decisiones clave y alternativas descartadas (lo que te preguntarán)

| Decisión | Por qué | Alternativa descartada |
|---|---|---|
| **redb** como storage | embebido, ACID, puro Rust, un fichero | SQLite (C, FFI), RocksDB (pesado, C++), sled (menos maduro) |
| **Embeddings in-process** | foso: privacidad/on-prem, sin servicio externo | Llamar a OpenAI/servidor externo (saca datos, añade latencia de red) |
| **e5-small multilingüe** | calidad/recurso, ES+EN | modelos grandes (RAM/latencia), solo-inglés |
| **Híbrido + RRF** | el denso pierde términos literales; RRF es robusto a escalas | solo vector (falla con códigos/años); umbral fijo de coseno (anisotropía e5: todo 0,88) |
| **Reranking opcional** | gran salto de calidad, pero caro (~150 ms/candidato CPU) | siempre on (mata el throughput); por eso es opt-in con cota |
| **Motor síncrono + spawn_blocking** | inferencia y redb son CPU/IO-bound; no contaminar async | async hasta abajo (complejidad sin beneficio) |
| **Diferencial = delta binario** | full-fidelity (incluye borrados), restaura exacto | "incremental de novedades" lógico (no captura borrados; necesita import con ids) |
| **Backup = copia lógica** | redb bloquea el fichero; `fs::copy` falla en Windows | copia de fichero (error 33), parar el servidor (no en caliente) |
| **Restore por swap (EngineHandle)** | en caliente, micro-corte; el job queue sigue | sobrescribir el fichero abierto (imposible en Windows); reinicio obligatorio |
| **Micro-batching OFF** | medido: empeora en CPU (serializa lo que paraleliza) | on por defecto (–17% throughput) |
| **Límite concurrencia = válvula** | medido: limitar a núcleos estrangula ~25% | cap agresivo a núcleos (peor throughput sin ganar latencia) |
| **NO LLM generativo dentro** | overreach; el motor recupera, el LLM lo pone el llamante | meter un LLM (alcance/coste/mantenimiento) |
| **Auto-inducción de labels: aparcada** | las reglas no generalizan; el catálogo+retrieval es la vía, pero el usuario decidió usar labels provistos | reglas regex (cierran el abanico a un vertical) |
| **IDs tipados (newtypes)** | evita confundir ids en compilación | u64 desnudos |
| **2 crates (core/server)** | testear el núcleo aislado, publicar la librería | un crate con bin+lib (axum dentro del core) |

---

## 15. Limitaciones conocidas (sé honesto antes de que las saquen)

- **Escala**: monoproceso (redb es single-writer), índice **en memoria**, sin sharding ni
  réplicas. No es para miles de millones de vectores ni multi-nodo. Para esa escala → Qdrant.
- **Throughput** limitado por el embedding en CPU (~388 rps/12 hilos). Subir el techo = GPU del
  embedding o modelo más ligero (ni batching ni más pools ayudan, está medido).
- **Reranking** es lento en CPU (~150 ms/candidato); la GPU AMD probada (DirectML) **no** ayudó.
- **`.doc` binario heredado** no soportado (sí `.docx`); algunos PDF cifrados fallan al extraer.
- **Chunker** de tamaño fijo corta a mitad de palabra; chunking por frase mejoraría.
- **Restore** cambia el nombre físico del DB (puntero `active_db.txt`); backups locales (sin S3).
- Madurez operativa joven frente a productos con años (observabilidad, tuning).

---

## 16. Comparativa rápida (para la pregunta "¿por qué no Qdrant / SQL Server?")

- **Qdrant**: base **vectorial** dedicada, escala masiva (sharding/réplicas), máximo QPS de ANN —
  pero **no extrae documentos** (traes el pipeline de extracción y troceado). Elígelo para escala enorme.
- **SQL Server 2025 + vectores**: RDBMS generalista con tipo `VECTOR`/DiskANN; vectores **junto a
  datos relacionales** y HA empresarial — híbrido y chunking a mano.
- **Nucleus**: RAG **funcionando ya**, on-prem/privado, sin montar pipeline; escala media (millones
  de chunks en un nodo). El diferencial está en la ingesta **end-to-end** dentro del motor
  (extracción + chunking + embeddings + índice), no en un componente aislado.

(Aviso de honestidad: el número de Nucleus es medido; los de Qdrant/SQL Server son
características conocidas, no medidas en el mismo banco.)

---

## 17. Datos de rendimiento de bolsillo

- Búsqueda híbrida: **~11 ms** p50 (release, en caliente). Throughput pico **~388 req/s** (12 hilos).
- RAM: **~135 MB** reposo → **~1 GB** estable bajo carga (modelo + ONNX + índice). Sin fugas.
- Ingesta: 50 docs / **15.732 chunks** en **~1.083 s** (dominado por pocos PDFs grandes).
- Reranking: **~150 ms/candidato** CPU; cota 20 ≈ +2,4 s, cota 50 ≈ +7,5 s. GPU DirectML no ayudó.
- Diferencial: 476 B vs full 3,7 MB. Imagen Docker 197 MB; exe Windows 32 MB autocontenido.
- MSRV: Rust **1.82** (usa `Option::is_none_or`). ~49 tests (47 core + 2 e2e), clippy `-D warnings` limpio.

---

## 18. Preguntas difíciles → respuestas

- **"¿Y si se cae a mitad de una ingesta?"** Los jobs son durables (redb); al arrancar se
  re-encolan los `Running`/`Pending`. redb es ACID por transacción; un `kill -9` no corrompe.
- **"¿Cómo garantizas un backup consistente sin parar?"** Copia lógica sobre una read-txn de redb
  (vista MVCC consistente) → snapshot point-in-time, en caliente.
- **"¿El restore pierde datos?"** No: antes de sustituir toma una copia de seguridad del estado
  actual; y el diferencial es full-fidelity (restaura exacto, incluye borrados).
- **"¿Por qué no usas el diferencial 'de novedades' que es lo típico?"** Porque no captura
  borrados y exige reimportar preservando ids. El delta binario es correcto y más simple.
- **"¿El híbrido no es más lento?"** El BM25 es en memoria y barato; el cuello es el embedding de
  la query, no la fusión. ~11 ms en total en caliente.
- **"¿Por qué el reranking no está siempre activo?"** Cuesta ~150 ms/candidato en CPU; mata el
  throughput. Es opt-in con cota de candidatos (punto dulce 20).
- **"¿Escala?"** En un nodo, a millones de chunks, sí. Multi-nodo / miles de millones, no es el
  objetivo (ahí Qdrant). Es una decisión de alcance, no un descuido.
- **"¿Y la concurrencia, no se corrompe?"** Medido: 0 resultados incorrectos en ~40.000 peticiones
  concurrentes (test compara contra baseline determinista). Locks `parking_lot`, lecturas de índice
  concurrentes, escritura serializada por redb.
- **"¿Por qué Rust?"** Sin GC (latencia predecible), seguridad de memoria, un binario sin runtime,
  y el ecosistema (redb, fastembed/ort, hnsw_rs) es puro/embebible.

---

## 19. Cómo verificarlo en vivo (por si te lo piden)

```powershell
# Tests + lint
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# Arrancar
$env:NUCLEUS_DB="C:\nucleus\data.redb"; .\target\release\nucleus-server.exe
# token admin en el fichero indicado al arrancar

# Flujo: crear dominio → subir fichero → esperar job → buscar → backup → restore
# (ver docs/guia-rapida.md y docs/operacion.md)

# Carga
node scripts/loadtest.mjs
```

Documentos de apoyo: [arquitectura.md](arquitectura.md), [operacion.md](operacion.md),
[rendimiento.md](rendimiento.md), [api.md](api.md), [configuracion.md](configuracion.md).
