# Rendimiento y carga

Datos **reales medidos** (2026-06-21) sobre el binario de release y un corpus real, no
estimaciones. Sirven para dimensionar despliegues y entender los límites del motor.

> Reproducible con [`scripts/loadtest.mjs`](../scripts/loadtest.mjs) (Node 18+, sin
> dependencias). El script mide throughput, percentiles de latencia y **corrección bajo
> concurrencia** (compara el top-1 de cada consulta contra una baseline secuencial; la
> búsqueda híbrida es determinista, así que cualquier discrepancia delataría una condición
> de carrera).

## Resumen ejecutivo

- **Throughput de búsqueda**: pico **~400–410 req/s** (híbrido, sin reranking). Es
  **CPU-bound**: satura hacia concurrencia ≈ nº de núcleos y más allá solo sube la latencia.
- **Latencia en caliente**: **~11 ms** p50 por búsqueda con poca carga (binario de release).
- **Robustez**: **0 errores y 0 resultados incorrectos** hasta concurrencia 256 y en carga
  sostenida (90 s, ~37.000 peticiones). Bajo sobrecarga degrada con elegancia.
- **Memoria**: **estable ~1 GB** bajo carga máxima sostenida, **sin fugas**. No crece con la
  concurrencia ni con el tiempo.
- **Reranking**: ~**150 ms por candidato** en CPU (cross-encoder). Con la cota por defecto
  (20) son ~2,4 s/consulta; opcional y opt-in por eso.

## Entorno de prueba

| | |
|---|---|
| CPU | 12 hilos lógicos |
| GPU | AMD Radeon RX 570 (4 GB) — solo para la prueba de reranking |
| Build | `nucleus-server` **release**, ONNX Runtime estático |
| Corpus | dominio fiscal, **15.732 chunks** / 50 documentos |
| Modelo | `multilingual-e5-small` (384 dim), embeddings in-process |
| Índice | `flat` (coseno exacto) + BM25 léxico, fusión RRF |

## Búsqueda — latencia y throughput

Barrido de concurrencia, 500 peticiones por nivel, consultas variadas en lenguaje natural:

| Concurrencia | ok/err | incorrectos | req/s | p50 | p90 | p99 | max |
|---:|:---:|:---:|---:|---:|---:|---:|---:|
| 1   | 500/0 | 0 |  85 |  11 ms |   14 ms |   24 ms |   30 ms |
| 8   | 500/0 | 0 | 275 |  19 ms |   64 ms |  118 ms |  213 ms |
| 16  | 500/0 | 0 | 326 |  28 ms |  107 ms |  185 ms |  295 ms |
| 32  | 500/0 | 0 | 369 | 100 ms |  138 ms |  270 ms |  375 ms |
| 64  | 500/0 | 0 | **409** | 124 ms |  256 ms |  612 ms | 1047 ms |
| 128 | 500/0 | 0 | 395 | 288 ms |  512 ms |  791 ms |  996 ms |
| 256 | 500/0 | 0 | 372 | 497 ms | 1094 ms | 1247 ms | 1302 ms |

**Carga sostenida**, concurrencia 128 durante 90 s:

| | req totales | ok/err | incorrectos | req/s | p50 | p90 | p99 | max |
|---|---:|:---:|:---:|---:|---:|---:|---:|---:|
| 128 @ 90 s | 36.768 | 36768/0 | 0 | 408 | 298 ms | 481 ms | 740 ms | 1395 ms |

Lectura: el throughput crece con los núcleos hasta ~conc 64 (≈ nº de núcleos) y luego se
estanca; pasada la saturación, la latencia sube pero **nada falla**. La búsqueda es
**CPU-bound** (domina el cálculo del embedding de la consulta).

## Recursos bajo carga sostenida (conc 128, 90 s)

Muestreo del proceso `nucleus-server` a 1 Hz:

| Métrica | Valor |
|---|---|
| RAM en reposo | ~135 MB |
| RAM bajo carga (inicio / fin / pico / media) | 1052 / 1002 / 1070 / 1043 MB |
| CPU (media / pico, sobre 12 núcleos) | ~66 % / saturación |

La RAM **no crece** con el tiempo ni con la concurrencia (fin ≤ inicio ⇒ sin fugas). Ese
~1 GB es esencialmente **modelo + runtime ONNX + índice en memoria**.

## Límite de concurrencia y micro-batching (medido)

Probadas las dos palancas sobre el mismo corpus (sweep, 500 req/nivel):

| Configuración | pico req/s | p99 @ conc 256 | 503 (shed) |
|---|---:|---:|:---:|
| Sin límite (válvula a 512) | **388** | 1021 ms | 0 |
| Límite = núcleos (12), espera 2 s | 284 | 964 ms | 0 |
| Límite = núcleos (12), espera 200 ms | 268 | 748 ms | sí (protege) |
| Micro-batching ON (lote 16) | 321 | — | 0 |

Conclusiones:

- **Micro-batching: contraproducente en CPU.** Serializa una etapa que con embeds
  independientes ya paraleliza por todos los núcleos, y añade ~10 ms a una consulta aislada
  (11→22 ms). Queda **off por defecto** (`NUCLEUS_EMBED_BATCH_MAX=1`); útil solo en GPU.
- **Limitar al nº de núcleos estrangula** (~25% menos throughput) sin gran mejora de latencia:
  la sobre-suscripción rinde más (HT, solape de stalls de memoria). El límite es una **válvula
  de seguridad** (default 16× núcleos), no un acelerador.
- El **load-shed** (límite bajo + espera corta) sí cumple su papel: bajo avalancha rechaza el
  exceso con `503` y mantiene acotada la latencia de los admitidos (p99 748 ms vs cola creciente).
  Úsalo si tienes un SLO de latencia estricto.

## Reranking — coste vs. calidad

El cross-encoder (`bge-reranker-base`) re-puntúa cada par `(consulta, candidato)`, así que
el coste es **lineal con el nº de candidatos** (~150 ms/candidato en CPU). Barrido sobre el
mismo corpus (k=5), tomando la cota 50 como referencia de calidad:

| Candidatos | Latencia extra | top-1 = cota 50 | top-3 ∩ cota 50 |
|---:|---:|:---:|:---:|
| (sin rerank) | — | — | — |
| 50 | ~7,7 s | 6/6 | 3,0/3 |
| **20 (defecto)** | **~2,4 s** | **5/6** | 2,2/3 |
| 10 | ~1,1 s | 3/6 | 1,3/3 |
| 5 | ~0,6 s | 3/6 | 1,2/3 |

La cota **20** es el punto dulce: ~3× más rápido que el rerank completo conservando casi
toda la calidad. Ajustable con `NUCLEUS_RERANK_CANDIDATES`.

**GPU (DirectML, RX 570): no ayudó** — el reranking fue ~2,2× *más lento* que en CPU (muchas
inferencias pequeñas, domina el overhead por llamada). La palanca efectiva es la cota de
candidatos, no la GPU (en este hardware). Ver [configuración](configuracion.md#búsqueda-híbrida-y-reranking).

## Ingesta — throughput observado

Ingesta del corpus fiscal crudo (PDFs), extracción + chunking + embedding + indexado en el
motor, un documento a la vez:

- **50 de 59 documentos** ingestados → **15.732 chunks** en **~1.083 s**.
- 9 PDFs fallaron por **cifrado no soportado** (manuales Renta/IVA/Sociedades).
- Dominado por pocos documentos grandes: p. ej. un BOE de 4.425 chunks tardó ~439 s; los
  documentos pequeños, <2 s cada uno.
- La RAM de ingesta **no escala con el tamaño del documento** (embedding en ventanas de 64
  chunks); subir `NUCLEUS_WORKERS` aumenta el throughput de ingesta y el pico de RAM.

## Límites recomendados por hardware

- **Memoria**: host/contenedor ≈ **1,5–2 GB** + tamaño del índice
  (vectores = nº_chunks × dim × 4 B, + textos para BM25). Suma otro modelo (~cientos de MB)
  si activas reranking.
- **Concurrencia**: la búsqueda es CPU-bound; el servidor ya **acota la concurrencia**
  (`NUCLEUS_MAX_CONCURRENT_SEARCHES`, por defecto = núcleos) con *load-shed* a `503` y
  **micro-batching** de los embeddings de consulta. Ajusta el límite al hardware.
- **CPU**: dimensiona por núcleos; el throughput de búsqueda escala con ellos hasta saturar.
- **`NUCLEUS_WORKERS`** afecta a la **ingesta**, no a la búsqueda.

## Caveats de medición

- Las cifras de **búsqueda** son del binario de **release**. Los ejemplos de Rust
  (`rerank_ab`, `ingest_fiscal`) corren en build de **desarrollo** (sin optimizar): el
  camino de búsqueda en Rust es ~10× más lento ahí, así que sus latencias absolutas de
  *híbrido* no son comparables. El coste del **reranker es ONNX nativo** (~150 ms/candidato),
  independiente de release/debug.
- El generador de carga es un único proceso Node; parte del techo de ~400 req/s podría ser
  del cliente, así que el servidor podría tener algo más de margen.

## Cómo reproducir

```powershell
# arranca el servidor con un corpus y obtén un token admin (ver operacion.md)
$env:NUC_TOKEN = (Get-Content ruta\admin_token.txt -Raw).Trim()

# barrido de concurrencia
$env:NUC_CONCS = "1,8,16,32,64,128,256"; $env:NUC_REQS = "500"
node scripts/loadtest.mjs

# carga sostenida
$env:NUC_DURATION = "90"; $env:NUC_CONC = "128"
node scripts/loadtest.mjs
```
