# Ejemplo: pipeline RAG completo

Un pipeline RAG (Retrieval-Augmented Generation) de extremo a extremo con
Nucleus como base de datos y un LLM generativo en el cliente. Cubre ingesta,
recuperación, construcción del prompt y respuesta.

> Nucleus **no** incluye un LLM generativo a propósito: tú eliges el modelo
> (OpenAI, Azure, Anthropic, Ollama, un modelo local…). El motor se centra en
> **recuperar** bien.

## Diagrama

```
 PDFs/DOCX/TXT ─ingesta─▶ [Nucleus]  (extraer, trocear, embeber, indexar)
                                 │
   pregunta ─search──▶ hits (chunks rankeados, híbrido + rerank opcional)
                                 │
                          ◀── prompt = contexto + pregunta
                                 │
                          [LLM generativo] ──▶ respuesta con citas
```

## 1. Preparar la base

Crea un dominio por área de conocimiento y ingesta los documentos etiquetados:

```bash
BASE=http://127.0.0.1:8080
TOKEN=nuc_xxx

# Un dominio por corpus (cada uno fija su modelo de embeddings)
curl -s -X POST $BASE/v1/domains -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"name":"fiscal"}'
# -> {"id":1,...}

# Ingiesta con subdominio + labels (se crean al vuelo)
curl -s -X POST "$BASE/v1/domains/1/files?filename=IRPF_2026.pdf&subdomain=irpf&labels=2026,vigente" \
  -H "Authorization: Bearer $TOKEN" --data-binary @IRPF_2026.pdf
```

El etiquetado (`subdomain`, `labels`) es clave para **acotar la recuperación** y
evitar que el LLM alucine mezclando años o normas derogadas.

## 2. Recuperar contexto

```bash
curl -s -X POST $BASE/v1/domains/1/search \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{
    "query": "tipos de retención de IRPF para 2026",
    "k": 5,
    "subdomain": "irpf",
    "filter": "tag:2026 AND NOT tag:borrador"
  }'
```

- `subdomain: "irpf"` acota al tema.
- `filter` usa el [lenguaje de consulta](../lenguaje-consulta.md) para exigir
  la label `2026` y excluir borradores.
- `k: 5` trae 5 chunks. Para RAG, 3–8 suele ser un buen punto.

Para más contexto, recupera los **vecinos** del mejor chunk:

```bash
curl -s "$BASE/v1/chunks/<chunk_id>/context?before=1&after=1" \
  -H "Authorization: Bearer $TOKEN"
```

## 3. Construir el prompt y generar

### Python (con OpenAI)

```python
import requests, openai

BASE = "http://127.0.0.1:8080"
TOKEN = "nuc_xxx"
H = {"Authorization": f"Bearer {TOKEN}"}

def retrieve(question, domain=1, k=5):
    r = requests.post(f"{BASE}/v1/domains/{domain}/search",
                      headers=H, json={"query": question, "k": k,
                                       "subdomain": "irpf",
                                       "filter": "tag:2026"})
    r.raise_for_status()
    return r.json()

hits = retrieve("tipos de retención de IRPF para 2026")
context = "\n\n---\n\n".join(
    f"[{i+1}] {h['text']}" for i, h in enumerate(hits)
)

prompt = f"""Eres un asistente fiscal. Responde USANDO SOLO el contexto.
Si la respuesta no está en el contexto, di que no la sabes.
Cita el número entre corchetes al afirmar algo.

Contexto:
{context}

Pregunta: ¿Cuáles son los tipos de retención de IRPF para 2026?
"""

resp = openai.chat.completions.create(
    model="gpt-4o-mini",
    messages=[{"role": "user", "content": prompt}],
)
print(resp.choices[0].message.content)
```

### C# (con el cliente Nucleus + Azure OpenAI)

```csharp
using Nucleus.Client;

var nucleus = new NucleusClient(BASE, TOKEN);
var hits = await nucleus.SearchAsync(domainId, new SearchRequest {
    Query = "tipos de retención IRPF 2026",
    K = 5,
    Subdomain = "irpf",
    Filter = "tag:2026",
});

var context = string.Join("\n---\n", hits.Select((h, i) => $"[{i+1}] {h.Text}"));
var prompt = $"Contexto:\n{context}\n\nPregunta: ¿tipos de retención IRPF 2026?";
// → envía `prompt` a tu LLM (OpenAI/Azure/OpenAI/Ollama)
```

## 4. Mejorar la calidad

- **Reranking**: activa `NUCLEUS_RERANK_MODEL=bge-reranker-base` para reordenar
  los candidatos con un cross-encoder (ver
  [configuración](../configuracion.md#búsqueda-híbrida-y-reranking)). Mejora el
  orden a costa de latencia.
- **Diversidad**: si los top-k son demasiado parecidos, sube `diversity`
  (MMR, 0–1) para cubrir más ángulos del tema.
- **Chunking**: para documentos con estructura (leyes, manuales), pre-trocea por
  sección y envía `chunks[]` en vez de texto plano.
- **Filtros**: usa `filter` agresivamente (`tag:2026`, `NOT tag:derogado`) para
  no contaminar el contexto con información obsoleta.

## 5. Modo embebido (sin HTTP)

Para una app de escritorio o un servidor monolítico, usa la cdylib sin red:

```csharp
// clients/csharp/Nucleus.Native — P/Invoke sobre nucleus.dll
var handle = NucleusNative.Open(new { db_path = "nucleus.redb" });
var hits = NucleusNative.Search(handle, new { domain_id = 1, query = "...", k = 5 });
// → pasa `hits` al LLM igual que arriba
```

«SQLite, pero para RAG»: sin sidecar, sin servicio, embeddings dentro.

## Errores comunes

| Síntoma | Causa probable |
|---------|----------------|
| El LLM alucina datos de otro año | Falta `filter` por label de año; el contexto mezcla normativa vieja. |
| Respuesta genérica, no usa el contexto | Demasiados chunks o irrelevantes; baja `k` y mejora los filtros. |
| Latencia alta | Reranking activo con cota alta; baja `NUCLEUS_RERANK_CANDIDATES`. |
| 0 resultados | `subdomain` inexistente o `filter` demasiado restrictivo. |

Ver también: [integraciones](../integrations.md) con LangChain/LlamaIndex y la
[guía de debugging](../debugging.md).
