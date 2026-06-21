# Nucleus — Resumen de defensa (una cara)

**Qué es:** motor de base de datos **llave en mano para RAG**. Subes documentos crudos y
preguntas en lenguaje natural; extrae, trocea, **embebe, indexa y recupera** — todo dentro
del proceso. Rust, dos crates (`nucleus-core` + `nucleus-server` axum), un binario, un fichero
redb como fuente de verdad.

**El foso (3 patas):** ① embeddings **in-process** (privado/on-prem, sin servicio externo) ·
② ingesta transparente **multi-formato** dentro del motor · ③ recuperación de calidad lista
(**híbrido vector+BM25 con RRF** + reranking opcional), con dominio/subdominio/labels.

**Stack:** redb (KV ACID) + bincode · fastembed/ONNX (e5-small 384d) · flat/HNSW + BM25 ·
cross-encoder · jobs en redb + tokio · auth API-key con scopes · qbsdiff (backups).

---

### 6 decisiones que defender
1. **Embeddings dentro** (no API externa) → es el diferencial: privacidad + sin pipeline.
2. **Híbrido + RRF** (no solo vector) → recupera términos literales (leyes, años); RRF evita el problema de escalas y la anisotropía de e5.
3. **redb** (no SQLite/Rocks) → embebido, ACID, puro Rust, un fichero.
4. **Motor síncrono + spawn_blocking** → la inferencia y redb son CPU/IO-bound; no contaminar async.
5. **Diferencial = delta binario** (no "novedades" lógico) → full-fidelity (incluye borrados), restaura exacto con `full + último diff`.
6. **Restore en caliente por swap** (`EngineHandle`) → micro-corte; el job queue sigue el cambio.

### 5 cifras
- Búsqueda híbrida **~11 ms** p50 · pico **~388 req/s** (12 hilos, CPU-bound por el embedding).
- RAM **~1 GB** estable bajo carga, **sin fugas** (135 MB en reposo).
- **0 errores / 0 resultados incorrectos** en ~40.000 peticiones concurrentes (test determinista).
- Diferencial **476 B** vs full 3,7 MB · imagen Docker 197 MB · exe Windows 32 MB autocontenido.
- ~49 tests, clippy `-D warnings` limpio, MSRV 1.82.

### 4 límites (dilos tú primero)
- **Escala**: un nodo, redb single-writer, índice en memoria → no miles de millones ni multi-nodo (ahí, Qdrant).
- **Throughput** tope = embedding en CPU; subirlo = GPU del embedding o modelo más ligero.
- **Reranking** caro en CPU (~150 ms/candidato); GPU AMD/DirectML probada **no** ayudó.
- `.doc` heredado y algunos PDF cifrados no se extraen; chunker de tamaño fijo.

### 3 hallazgos que dan credibilidad (decididos con datos, no fe)
- **Micro-batching descartado**: medido, empeora en CPU (serializa lo que paraleliza) → off por defecto.
- **Limitar a núcleos estrangula ~25%**: el límite es válvula de seguridad/load-shed, no acelerador.
- **44 GB de RAM** en una ingesta enorme → arreglado con embedding en ventanas de 64 chunks.

### Respuestas rápido
- *¿Se corrompe con concurrencia?* No: 0 incorrectos medidos; redb serializa escritura, lecturas concurrentes, `parking_lot`.
- *¿Backup consistente sin parar?* Copia lógica sobre read-txn de redb (MVCC) → snapshot point-in-time.
- *¿Por qué no Qdrant/SQL Server?* Ellos no embeben ni extraen; la comparativa justa es end-to-end (embed+buscar), donde Nucleus no paga salto de red.
- *¿Y si se cae a media ingesta?* Jobs durables (se re-encolan al arrancar); redb ACID, `kill -9` no corrompe.
