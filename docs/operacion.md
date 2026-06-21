# Operación

Guía para ejecutar Nucleus en serio: seguridad, jobs, persistencia, memoria, backups.

## Arranque y token inicial

Al primer arranque con una BD vacía, el servidor crea e **imprime una sola vez** un
token de administración global. Guárdalo: es la única forma de operar al principio. Si lo
pierdes y no tienes otro token admin, tendrás que partir de una BD nueva.

Con ese token admin, crea tokens con menos privilegios para tus aplicaciones (ver
[seguridad](#seguridad)).

## Seguridad

- **Tokens API-key con scopes por dominio** (`Read`/`Write`/`Admin`). Se guardan
  hasheados (SHA-256); el secreto se muestra solo al crearlos.
- Principio de **mínimo privilegio**: da a cada app un token con el scope justo, p. ej.
  solo lectura sobre un dominio:
  ```json
  { "name": "buscador-web", "scopes": [ { "domain": { "One": 1 }, "perm": "Read" } ] }
  ```
- Revoca con `DELETE /v1/tokens/{id}`. Caduca con `expires_at` (millis Unix).
- **Expón Nucleus tras TLS** (reverse proxy: nginx/Caddy/Traefik). El servidor habla
  HTTP plano; no pongas `NUCLEUS_ADDR` en una interfaz pública sin TLS por delante.
- Limita el acceso de red al puerto; el `admin` puede crear dominios y tokens.

## Jobs y escalabilidad

- La ingesta (troceo + embeddings) corre en una **cola persistida en la BD** con
  `NUCLEUS_WORKERS` workers.
- La cola **sobrevive a reinicios**: los jobs `Running` interrumpidos se re-encolan al
  arrancar.
- Más workers = más throughput de ingesta, pero más CPU y memoria. La inferencia es
  CPU-bound (o GPU con la feature). Para ingestas masivas, ajusta `NUCLEUS_WORKERS` al
  nº de núcleos disponible y vigila la RAM.
- Estado de cada job en `GET /v1/jobs/{id}` (`Pending`/`Running`/`Done`/`Failed`, con
  `attempts` y `error`). Los fallos se reintentan con tope de intentos.

## Memoria y rendimiento

- La ingesta embebe en **ventanas acotadas de chunks**, de modo que el pico de RAM **no
  escala con el tamaño del documento** (un PDF de varios MB no dispara la memoria).
- Aun así, `NUCLEUS_WORKERS` multiplica el pico (cada worker embebe en paralelo). Para
  documentos muy grandes, mantén 2–4 workers.
- El **índice vive en memoria** (por dominio). Con `flat`, son todos los vectores; con
  muchos millones de chunks, considera `hnsw`.
- La **primera** inferencia carga el modelo en memoria (cientos de MB) y, si no está en
  caché, lo descarga. Fija `NUCLEUS_MODEL_CACHE`.

## Pruebas de carga y límites por hardware

Hay un test de carga sin dependencias en [`scripts/loadtest.mjs`](../scripts/loadtest.mjs)
(Node 18+). Verifica throughput, percentiles de latencia y **corrección bajo concurrencia**
(compara el top-1 de cada consulta contra una baseline secuencial; el híbrido es
determinista, así que cualquier discrepancia delataría una condición de carrera).

```powershell
$env:NUC_TOKEN = (Get-Content ruta\admin_token.txt -Raw).Trim()
node scripts/loadtest.mjs                                   # barrido de concurrencia
$env:NUC_DURATION="90"; $env:NUC_CONC="128"; node scripts/loadtest.mjs   # sostenido
```

Resultados de referencia (corpus de ~15.700 chunks, búsqueda híbrida **sin** reranking,
CPU de 12 hilos lógicos):

- **Throughput**: pico ~**400–410 req/s**; satura hacia concurrencia ≈ nº de núcleos
  (la búsqueda es **CPU-bound** por el cálculo del embedding de la consulta). Más allá,
  el throughput se estanca y solo sube la latencia.
- **Robustez**: 0 errores y 0 resultados incorrectos hasta concurrencia 256 y en sostenido
  (90 s, ~37.000 peticiones). Bajo sobrecarga **degrada con elegancia** (sube la latencia,
  nada falla).
- **Memoria**: **estable** (~1 GB en este corpus) bajo carga máxima sostenida, sin fugas.
  Ese ~1 GB es básicamente el **modelo + runtime ONNX + índice en memoria**, y **no crece**
  con la concurrencia ni con el tiempo. Para dimensionar: base ≈ 1 GB + tamaño del índice
  (vectores = nº_chunks × dim × 4 bytes, + textos para BM25).
- **Reranking**: con `NUCLEUS_RERANK_MODEL` activo cada consulta cuesta ~2,4 s (cota 20) en
  CPU, así que el throughput concurrente cae drásticamente. Úsalo solo donde la calidad lo
  justifique, o reduce `NUCLEUS_RERANK_CANDIDATES`.

**Límite de concurrencia (válvula de seguridad).** El servidor acota las búsquedas
concurrentes con `NUCLEUS_MAX_CONCURRENT_SEARCHES` (por defecto **16× núcleos** = generoso, no
estrangula). Al superarse, esperan hasta `NUCLEUS_SEARCH_WAIT_MS` un hueco y, si no lo hay,
reciben **`503`** (*load-shed*, contados en `nucleus_search_rejected_total`). **Medido**:
limitar al nº de núcleos baja el throughput ~25% (la sobre-suscripción ayuda por HT/solape),
así que el default es generoso; para una latencia de cola estricta, **baja el límite a ~núcleos
y la espera a ~200 ms** para rechazar pronto bajo avalancha en lugar de encolar.

**Micro-batching de embeddings (`NUCLEUS_EMBED_BATCH_MAX`, off por defecto).** Agrupar las
consultas en una sola inferencia ONNX **empeora en CPU**: serializa una etapa que con embeds
independientes ya paraleliza por todos los núcleos (medido: ~321 vs ~388 req/s, y +10 ms en
una consulta aislada). Déjalo en `1`. Súbelo (p.ej. 16) **solo en GPU**, donde una inferencia
de lote grande compensa el coste por llamada.

**Memoria**: contenedor/host ≈ 1,5–2 GB + tamaño del índice (más si activas reranking, que
carga un segundo modelo). Sube `NUCLEUS_WORKERS` solo para ingesta concurrente, no afecta a
la búsqueda.

## Índices y persistencia

- **`flat`** (por defecto): exacto, **no persiste**; se reconstruye desde la BD al
  arrancar (rápido).
- **`hnsw`**: aproximado, **persiste** a `NUCLEUS_INDEX_DIR`:
  - se vuelca al **apagar** (Ctrl-C, *graceful shutdown*) o con `POST /v1/maintenance/persist`,
  - se **recarga** al arrancar (con *fallback* a reconstrucción si falta o está corrupto).
- Recomendación con HNSW: llama a `persist` periódicamente o asegúrate de un apagado
  limpio para no reconstruir el grafo entero al reiniciar.

## Backups y restauración

Nucleus incluye **copias de seguridad a nivel de motor**, en caliente (sin parar el
servidor). Todo el estado vive en el `.redb`; los índices **no** se respaldan (se
reconstruyen al restaurar). El modelo en `NUCLEUS_MODEL_CACHE` es regenerable.

**Tipos de copia**
- **Full**: snapshot consistente y autónomo del `.redb` (se toma reteniendo el lock de
  escritura, copia lógica vía la API de redb para evitar el lock de fichero del SO).
- **Diferencial**: *delta binario* (bsdiff) del estado actual contra el último full. Es
  **full-fidelity** (incluye borrados) y para restaurar basta `full + último diferencial`,
  como en SQL Server.

Cada copia lleva **timestamp** (`AAAA-MM-DD_HH-MM-SS`) y queda en el catálogo
(`catalog.json` en `NUCLEUS_BACKUP_DIR`).

**Acciones (admin)**
```bash
# copia ahora
curl -X POST $BASE/v1/backups -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"kind":"full"}'        # o "differential"
# listar catálogo
curl $BASE/v1/backups -H "Authorization: Bearer $TOKEN"
# restaurar (toma una copia de seguridad del estado actual y cambia el motor en caliente)
curl -X POST $BASE/v1/backups/restore -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"id":"2026-06-21_15-30-12-123-full"}'
```

**Programación** (configurable en caliente o por entorno):
```bash
curl -X POST $BASE/v1/backups/schedule -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"enabled":true,"interval_secs":21600,"full_every":7,"keep_fulls":7}'
```
O por entorno: `NUCLEUS_BACKUP_INTERVAL=6h`, `NUCLEUS_BACKUP_FULL_EVERY`, `NUCLEUS_BACKUP_KEEP`
(ver [configuración](configuracion.md)). El scheduler hace un full cada *N* copias y
diferenciales entre medias, y **purga** los full más antiguos según la retención.

**Restore en caliente**: el servidor toma primero una copia de seguridad del estado actual,
reconstruye la copia elegida en un fichero nuevo, abre el motor sobre él y lo **cambia
atómicamente** (un micro-corte para las peticiones en vuelo; el job queue sigue el cambio).
Un puntero `active_db.txt` recuerda el fichero activo para que un reinicio reabra el restaurado.

> Backup/restore manual offline sigue siendo válido: parar el servidor y copiar el `.redb`.

## Apagado

Usa **Ctrl-C** (apagado ordenado): el servidor vuelca los índices persistibles antes de
salir. Un `kill -9` es seguro para los datos (redb es ACID por transacción) pero te hará
reconstruir el índice HNSW al reiniciar.

## Observabilidad

- Logs vía `tracing`; controla el nivel con `RUST_LOG` (p. ej.
  `RUST_LOG=nucleus_server=debug,tower_http=info`).
- `GET /healthz` para *health checks* del orquestador/load balancer.

## GPU

Compila con `--features gpu` y arranca con `NUCLEUS_GPU=true` para usar DirectML
(Windows) con *fallback* a CPU. Verifica en los logs que el provider GPU se inicializa;
si no hay GPU/driver, cae a CPU automáticamente.
