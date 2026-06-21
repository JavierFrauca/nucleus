# Guía rápida

De cero a buscar en lenguaje natural en cinco pasos. Se asume el servidor compilado
(ver [instalación](instalacion.md)).

## 1. Arrancar el servidor

```bash
cargo run --release -p nucleus-server
```

Copia el **token admin** que imprime al primer arranque. En los ejemplos:

```bash
TOKEN=nuc_xxxxxxxx
BASE=http://127.0.0.1:8080
```

(En PowerShell: `$TOKEN="nuc_..."; $BASE="http://127.0.0.1:8080"`.)

## 2. Crear un dominio

Un **dominio** es una colección/namespace que segmenta tu base. Lo define el usuario
y fija el modelo de embeddings (por defecto, multilingüe).

```bash
curl -s -X POST $BASE/v1/domains \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"name":"fiscal"}'
# -> {"id":1,"name":"fiscal","model":"multilingual-e5-small","dim":384,...}
```

## 3. Ingestar documentos

Tú sueltas el fichero o el texto; **Nucleus extrae, trocea, embebe e indexa**. Puedes
(opcionalmente) pasar `subdomain` y `labels` **por nombre**: el motor los crea si no
existen.

### a) Subir un fichero crudo (PDF, DOCX, XLSX, HTML, MD, TXT…)

```bash
curl -s -X POST "$BASE/v1/domains/1/files?filename=IRPF_2026.pdf&subdomain=irpf&labels=2026,irpf" \
  -H "Authorization: Bearer $TOKEN" \
  --data-binary @IRPF_2026.pdf
# -> {"document_id":1,"job_id":1,"chars":23816}
```

### b) Enviar texto/chunks directamente (JSON)

```bash
curl -s -X POST $BASE/v1/domains/1/documents \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"title":"Nota IVA","subdomain":"iva","labels":["2025","iva"],
       "text":"Tipos del IVA en 2025: general 21%, reducido 10%, superreducido 4%."}'
# -> {"document_id":2,"job_id":2}
```

La ingesta es **asíncrona**: devuelve un `job_id`. Consulta su estado:

```bash
curl -s $BASE/v1/jobs/1 -H "Authorization: Bearer $TOKEN"
# -> {"id":1,"status":"Done","attempts":1,"error":null}
```

> La **primera** ingesta descarga el modelo (~450 MB) y tarda; las siguientes van rápidas.

## 4. Buscar en lenguaje natural

```bash
curl -s -X POST $BASE/v1/domains/1/search \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"query":"tipos de retención de IRPF en 2026","k":5}'
```

Respuesta: lista de chunks rankeados por similitud:

```json
[
  {"chunk_id":1,"document_id":1,"score":0.927,
   "text":"Tabla de tipos de retención de IRPF para el ejercicio 2026…",
   "tags":[1,2],"metadata":{"filename":"IRPF_2026.pdf"}}
]
```

### Acotar la búsqueda

```bash
# por subdominio (nombre)
-d '{"query":"impuestos de 2025","subdomain":"iva","k":5}'

# por label vía lenguaje de query
-d '{"query":"tributos","filter":"tag:2026 AND NOT tag:borrador","k":5}'

# por documento(s)
-d '{"query":"...","document_ids":[1],"k":5}'
```

Ver el [lenguaje de consulta](lenguaje-consulta.md) para `filter`.

## 5. Contexto de un resultado (vecinos)

Cada chunk está encadenado a sus vecinos. Para recuperar contexto alrededor de un
chunk:

```bash
curl -s "$BASE/v1/chunks/5/context?before=1&after=2" -H "Authorization: Bearer $TOKEN"
```

## Equivalente en PowerShell

```powershell
$h = @{ Authorization = "Bearer $TOKEN" }
# crear dominio
Invoke-RestMethod "$BASE/v1/domains" -Method Post -Headers $h -ContentType 'application/json' `
  -Body '{"name":"fiscal"}' -UseBasicParsing
# subir un PDF
Invoke-RestMethod "$BASE/v1/domains/1/files?filename=IRPF_2026.pdf&subdomain=irpf&labels=2026,irpf" `
  -Method Post -Headers $h -InFile .\IRPF_2026.pdf -UseBasicParsing
# buscar
Invoke-RestMethod "$BASE/v1/domains/1/search" -Method Post -Headers $h -ContentType 'application/json' `
  -Body '{"query":"retención IRPF 2026","k":5}' -UseBasicParsing
```

> En Windows PowerShell usa `-UseBasicParsing` en `Invoke-WebRequest`/`Invoke-RestMethod`.

Siguiente: la [referencia de la API](api.md) y los [conceptos](conceptos.md).
