# Configuración

El servidor `nucleus-server` se configura por **variables de entorno**. Todas tienen
valores por defecto razonables.

| Variable | Defecto | Descripción |
|----------|---------|-------------|
| `NUCLEUS_DB` | `nucleus.redb` | Ruta del fichero de base de datos (redb). Se crea si no existe. |
| `NUCLEUS_ADDR` | `127.0.0.1:8080` | Dirección y puerto de escucha HTTP. |
| `NUCLEUS_WORKERS` | `2` | Nº de workers que procesan jobs de ingesta en paralelo. |
| `NUCLEUS_MODEL_CACHE` | (caché de fastembed) | Directorio donde se descargan/cachean los modelos de embeddings. Conviene fijarlo a una ruta persistente. |
| `NUCLEUS_INDEX` | `flat` | Backend del índice vectorial: `flat` (exacto) o `hnsw` (aproximado, persistente). |
| `NUCLEUS_INDEX_DIR` | `<dir de NUCLEUS_DB>/nucleus_indexes` | Dónde se vuelca/carga el grafo HNSW. |
| `NUCLEUS_GPU` | `false` | `true` para usar GPU en la inferencia. **Solo efectivo si el binario se compiló con `--features gpu`**; si no, se ignora (CPU). |
| `NUCLEUS_ADMIN_TOKEN_FILE` | `<dir BD>/nucleus_admin_token.txt` | Fichero donde se escribe el token admin inicial (no se vuelca a logs). |
| `NUCLEUS_CORS_ANY` | `false` | `true` permite cualquier origen CORS (clientes en navegador). |
| `NUCLEUS_RERANK_MODEL` | (vacío → reranking desactivado) | Modelo de *cross-encoder* para reordenar resultados. Valor soportado: `bge-reranker-base`. |
| `NUCLEUS_RERANK_CANDIDATES` | `20` | Nº de candidatos que re-puntúa el reranker por consulta. Más = mejor orden pero más lento (ver tabla abajo). Solo aplica si `NUCLEUS_RERANK_MODEL` está activo. |
| `NUCLEUS_MAX_CONCURRENT_SEARCHES` | 16× núcleos | Máximo de búsquedas concurrentes (válvula de seguridad). Al superarse, esperan y, si no hay hueco en `NUCLEUS_SEARCH_WAIT_MS`, reciben `503`. Bájalo para acotar la latencia de cola. |
| `NUCLEUS_SEARCH_WAIT_MS` | `2000` | Cuánto espera una búsqueda un hueco de concurrencia antes de devolver `503` (load-shed). Bájalo (p.ej. 200) para rechazar antes bajo avalancha. |
| `NUCLEUS_EMBED_BATCH_MAX` | `1` (desactivado) | Agrupa embeddings de consulta en una sola inferencia. **Off por defecto**: en CPU empeora (serializa lo que ya paraleliza). Súbelo (p.ej. 16) solo en GPU. |
| `NUCLEUS_EMBED_BATCH_WINDOW_MS` | `5` | Ventana para llenar un lote (solo si `NUCLEUS_EMBED_BATCH_MAX > 1`). |
| `NUCLEUS_BACKUP_DIR` | `<dir BD>/nucleus_backups` | Directorio de copias de seguridad (snapshots, deltas y catálogo). |
| `NUCLEUS_BACKUP_INTERVAL` | (vacío → desactivado) | Cadencia de copias programadas: `30m`, `6h`, `1d`, `2w` o segundos. |
| `NUCLEUS_BACKUP_FULL_EVERY` | `7` | Cada cuántas copias programadas se hace una **full** (el resto, diferenciales). |
| `NUCLEUS_BACKUP_KEEP` | `7` | Cuántas copias full (con sus diferenciales) se conservan; el resto se purgan. |
| `RUST_LOG` | `info` | Nivel de logs (`tracing`). Ej.: `nucleus_server=info,warn`. |

## Ejemplos

### Linux/macOS

```bash
export NUCLEUS_DB=/var/lib/nucleus/data.redb
export NUCLEUS_ADDR=0.0.0.0:8080
export NUCLEUS_WORKERS=4
export NUCLEUS_MODEL_CACHE=/var/lib/nucleus/models
export NUCLEUS_INDEX=hnsw
cargo run --release -p nucleus-server
```

### Windows (PowerShell)

```powershell
$env:NUCLEUS_DB        = "C:\nucleus\data.redb"
$env:NUCLEUS_ADDR      = "127.0.0.1:8080"
$env:NUCLEUS_WORKERS   = "4"
$env:NUCLEUS_MODEL_CACHE = "C:\nucleus\models"
$env:NUCLEUS_INDEX     = "flat"
.\target\release\nucleus-server.exe
```

## Elección del índice: `flat` vs `hnsw`

- **`flat`** (por defecto): coseno **exacto** por fuerza bruta. Resultados deterministas
  y filtros exactos. Recomendado para volúmenes pequeños/medianos. **No persiste**: se
  reconstruye desde el almacenamiento al arrancar (barato).
- **`hnsw`**: índice **aproximado** (grafo HNSW) para gran escala. Con pre-filtros los
  resultados son aproximados (HNSW ordena globalmente y luego se interseca). **Persiste**
  a disco (ver abajo) para no reconstruir al arrancar.

El índice es **por dominio**, pero el backend (`flat`/`hnsw`) es global del servidor.

## Persistencia del índice HNSW

Cuando `NUCLEUS_INDEX=hnsw`:
- El grafo se **vuelca** a `NUCLEUS_INDEX_DIR` al apagar el servidor (Ctrl-C) o llamando
  a `POST /v1/maintenance/persist`.
- Al arrancar, se **recarga** desde ahí (con *fallback* a reconstrucción si no hay
  volcado o está corrupto).
- El índice `flat` no persiste (se reconstruye siempre).

## GPU

1. Compila con la feature: `cargo build --release --features gpu`.
2. Arranca con `NUCLEUS_GPU=true`.

Usa el execution provider **DirectML** de ONNX Runtime (Windows) con *fallback*
automático a CPU si no hay GPU/driver. Sin la feature de compilación, `NUCLEUS_GPU` no
tiene efecto. Ver [instalación](instalacion.md#build-con-gpu-opcional).

## Búsqueda híbrida y reranking

La búsqueda **siempre** es híbrida: combina el índice **vectorial** (semántico) con un
índice **léxico BM25** (coincidencia exacta de términos), fusionando ambos rankings con
**RRF** (*Reciprocal Rank Fusion*). Esto recupera tanto lo semánticamente parecido como
lo que cita un término literal (códigos, artículos, nombres propios). No requiere
configuración: ambos índices se construyen por dominio al ingestar y al arrancar.

El **reranking** es una segunda etapa **opcional** y más precisa: un *cross-encoder*
re-puntúa los mejores candidatos de la fusión leyendo el par `(consulta, chunk)` completo.
Mejora notablemente el orden a costa de algo de latencia (corre in-process vía ONNX).

- Se activa con `NUCLEUS_RERANK_MODEL=bge-reranker-base`.
- El modelo se descarga (a `NUCLEUS_MODEL_CACHE`) y carga **perezosamente** en la primera
  búsqueda; esa primera petición será más lenta.
- Desactivado por defecto: sin la variable, solo se aplica la fusión híbrida.

```powershell
$env:NUCLEUS_RERANK_MODEL = "bge-reranker-base"
$env:NUCLEUS_RERANK_CANDIDATES = "20"   # opcional; 20 es el defecto
.\target\release\nucleus-server.exe
```

### Coste y `NUCLEUS_RERANK_CANDIDATES`

El cross-encoder puntúa cada par `(consulta, candidato)`, así que el coste crece de forma
lineal con el nº de candidatos. `NUCLEUS_RERANK_CANDIDATES` acota cuántos de los mejores
candidatos de la fusión se re-puntúan (al menos `k`, como mucho los recuperados).

Medido sobre el corpus fiscal de demo (~15.700 chunks, `k=5`, CPU, `bge-reranker-base`),
tomando la cota 50 como referencia de calidad:

| Candidatos | Latencia/búsqueda | top-1 igual a cota 50 | top-3 ∩ cota 50 |
|-----------:|------------------:|:---------------------:|:---------------:|
| (sin rerank) | ~115 ms | — | — |
| 50 | ~7.700 ms | 6/6 | 3,0/3 |
| **20 (defecto)** | **~2.400 ms** | **5/6** | 2,2/3 |
| 10 | ~1.100 ms | 3/6 | 1,3/3 |
| 5 | ~600 ms | 3/6 | 1,2/3 |

El defecto **20** es el punto dulce: ~3× más rápido que el rerank completo conservando casi
toda la calidad. Baja a `10` si priorizas latencia; sube a `50` para máxima precisión.

> **Nota sobre GPU:** medido aquí, `NUCLEUS_GPU=true` (DirectML) **no** acelera el reranking
> —de hecho fue más lento en una AMD Radeon RX 570— porque el cross-encoder hace muchas
> inferencias pequeñas y domina el overhead por llamada. La cota de candidatos es la palanca
> efectiva en CPU. La GPU puede ayudar en hardware NVIDIA/CUDA (no probado).

## Memoria e ingesta de documentos grandes

La ingesta embebe los chunks en **ventanas acotadas** para que el pico de RAM no escale
con el tamaño del documento. Aun así, más `NUCLEUS_WORKERS` = más ingestas concurrentes
= más pico de memoria. Para corpus con documentos muy grandes, mantén `NUCLEUS_WORKERS`
bajo (2–4). Ver [operación](operacion.md#memoria-y-rendimiento).
