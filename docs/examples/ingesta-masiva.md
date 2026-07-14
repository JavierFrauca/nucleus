# Ejemplo: ingesta masiva de documentos

Cómo cargar un corpus grande (miles de documentos) en Nucleus de forma eficiente
y robusta, usando el modo servidor HTTP.

> Prerrequisitos: un dominio creado y un token con scope `Write` sobre él. Ver
> la [guía rápida](../guia-rapida.md).

## Estrategia

La ingesta es **asíncrona**: cada `POST /files` o `POST /documents` devuelve un
`job_id` al instante y el trabajo (extraer, trocear, embebe, indexar) corre en
background en la cola de jobs. Para un corpus grande:

1. **Lanza el servidor con `NUCLEUS_WORKERS` adecuado** (2–4 para PDFs grandes;
   más solo si son documentos pequeños). Más workers = más pico de RAM.
2. **Ingiesta en lotes** sin esperar a que cada job termine: la cola absorbe.
3. **Controla el progreso** consultando jobs y listando documentos.
4. **Deduplicación automática**: el mismo contenido (hash SHA-256) no se
   re-ingesta; se devuelve `duplicate: true`.

## Script de carga (bash + curl)

```bash
#!/usr/bin/env bash
set -euo pipefail
BASE=http://127.0.0.1:8080
TOKEN=nuc_xxx
DOMAIN=1
SUBDOMAIN=${1:-irpf}      # p.ej. irpf, iva, sociedades
LABEL=${2:-2026}

shopt -s nullglob
for f in ./corpus/*.pdf ./corpus/*.docx ./corpus/*.txt; do
  name=$(basename "$f")
  echo "→ $name"
  resp=$(curl -s -X POST "$BASE/v1/domains/$DOMAIN/files?filename=$name&subdomain=$SUBDOMAIN&labels=$LABEL" \
    -H "Authorization: Bearer $TOKEN" --data-binary @"$f")
  echo "  $resp"
done
```

## Script de carga (Node.js, con control de progreso)

Para corpus grandes, conviene limitar la concurrencia y reportar progreso:

```javascript
// load-corpus.mjs — Node 18+, sin dependencias
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

const BASE = "http://127.0.0.1:8080";
const TOKEN = "nuc_xxx";
const DOMAIN = 1;
const DIR = "./corpus";
const CONCURRENCY = 4;

const files = (await readdir(DIR)).filter(f =>
  /\.(pdf|docx|xlsx|html|md|txt)$/i.test(f)
);

let done = 0;
async function upload(name) {
  const bytes = await readFile(join(DIR, name));
  const url = `${BASE}/v1/domains/${DOMAIN}/files?filename=${encodeURIComponent(name)}&subdomain=irpf&labels=2026`;
  const res = await fetch(url, {
    method: "POST",
    headers: { Authorization: `Bearer ${TOKEN}` },
    body: bytes,
  });
  const json = await res.json();
  if (!res.ok) throw new Error(`${name}: ${json.error}`);
  done++;
  console.log(`[${done}/${files.length}] ${name} → ${JSON.stringify(json)}`);
}

// Pool de concurrencia sencilla.
const queue = [...files];
const workers = Array.from({ length: CONCURRENCY }, async () => {
  while (queue.length) await upload(queue.shift());
});
await Promise.all(workers);
console.log("Carga completada.");
```

```bash
node load-corpus.mjs
```

## Comprobar que todo se indexó

Los jobs pasan por `Pending → Running → Done` (o `Failed`). Tras la carga:

```bash
# ¿Cuántos documentos hay en el dominio?
curl -s "$BASE/v1/domains/1/documents?limit=1" -H "Authorization: Bearer $TOKEN" | jq length

# ¿Algún job falló? (requiere scope Admin)
curl -s "$BASE/v1/jobs?limit=100" -H "Authorization: Bearer $TOKEN" \
  | jq '.[] | select(.status=="Failed")'
```

Si hay jobs `Failed`, su campo `error` indica la causa (p. ej. un PDF corrupto
que no se pudo extraer).

## Consejos de rendimiento

- La **primera ingesta** descarga el modelo (~450 MB); deja que el primer job
  termine antes de lanzar el resto en paralelo masivo.
- Para ficheros muy grandes (>50 MB), procesa secuencialmente con
  `CONCURRENCY=1` para acotar la RAM; el motor embebe en ventanas, pero el pico
  escala con los workers.
- Si repites la carga, la **deduplicación por hash** evita dobles: el segundo
  envío del mismo fichero responde `duplicate: true` y no re-embeds nada.
- Tras una carga grande con `NUCLEUS_INDEX=hnsw`, llama a
  `POST /v1/maintenance/persist` para volcar el grafo a disco antes de reiniciar.
