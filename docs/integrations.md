# Integraciones con frameworks de LLM/RAG

Nucleus es **agnóstico al LLM**: el motor se encarga de recuperar chunks
relevantes (búsqueda híbrida + reranking opcional); tú montas el LLM generativo
en el cliente y le pasas el contexto recuperado. Esta página muestra cómo
integrarlo con los frameworks de RAG más comunes.

> ¿Falta tu framework? La integración siempre sigue el mismo patrón: **(1)
> ingestar en Nucleus → (2) buscar → (3) inyectar los hits como contexto del
> prompt**. Abre un issue si quieres que documentemos uno concreto.

## Patrón general

```
documento → [Nucleus: extraer, trocear, embebe, indexar]
                                          │
pregunta → [Nucleus: search] ── hits ──┐
                                       ▼
                          prompt = contexto (hits) + pregunta
                                       │
                                       ▼
                              [LLM generativo] → respuesta
```

Los clientes oficiales están en **C#/.NET** y **TypeScript/JS**
([clients/](../clients)). Para otros lenguajes, o bien generas un cliente desde
[`docs/openapi.yaml`](openapi.yaml), o usas la cdylib embebida (C ABI).

## LangChain (Python)

LangChain no trae un *vector store* nativo para Nucleus, pero su interfaz
`VectorStore` es fácil de implementar sobre la API HTTP. La forma más rápida es
un cliente ligero con `requests`:

```python
import requests

class NucleusStore:
    """Adaptador mínimo de Nucleus a la forma que LangChain espera."""
    def __init__(self, base, token, domain_id):
        self.base = base.rstrip("/")
        self.h = {"Authorization": f"Bearer {token}"}
        self.domain_id = domain_id

    def add_texts(self, texts, metadatas=None, subdomain=None, labels=None):
        ids = []
        for i, t in enumerate(texts):
            meta = (metadatas or {}).get(i, {})
            body = {"title": meta.get("source", "doc"), "text": t, "metadata": {str(k): str(v) for k, v in meta.items()}}
            if subdomain: body["subdomain"] = subdomain
            if labels: body["labels"] = labels
            r = requests.post(f"{self.base}/v1/domains/{self.domain_id}/documents",
                              headers=self.h, json=body)
            r.raise_for_status()
            ids.append(r.json()["document_id"])
        return ids

    def similarity_search(self, query, k=4, filter=None):
        body = {"query": query, "k": k}
        if filter: body["filter"] = filter
        r = requests.post(f"{self.base}/v1/domains/{self.domain_id}/search",
                          headers=self.h, json=body)
        r.raise_for_status()
        from langchain_core.documents import Document
        return [Document(page_content=h["text"], metadata=h.get("metadata", {}))
                for h in r.json()]
```

Uso en un RAG mínimo:

```python
from langchain_openai import ChatOpenAI
from langchain_core.prompts import ChatPromptTemplate

store = NucleusStore("http://127.0.0.1:8080", TOKEN, domain_id=1)
llm = ChatOpenAI(model="gpt-4o-mini")

def ask(question):
    docs = store.similarity_search(question, k=5)
    context = "\n\n".join(d.page_content for d in docs)
    prompt = ChatPromptTemplate.from_template(
        "Responde usando SOLO este contexto.\n\n{context}\n\nPregunta: {q}"
    )
    return llm.invoke(prompt.format(context=context, q=question)).content
```

> Para encajar en el ecosistema LangChain completo (retrievers, chains), envuelve
> `NucleusStore` como un `Retriever` implementando `get_relevant_documents`.

## LlamaIndex (Python)

LlamaIndex expone `BaseRetriever` y `VectorStoreInterface`. Un retriever sobre
Nucleus es directo:

```python
from llama_index.core import Document
from llama_index.core.retrievers import BaseRetriever
from llama_index.core.schema import NodeWithScore
import requests

class NucleusRetriever(BaseRetriever):
    def __init__(self, base, token, domain_id, k=4):
        self.base = base.rstrip("/")
        self.h = {"Authorization": f"Bearer {token}"}
        self.domain_id = domain_id
        self.k = k

    def _retrieve(self, query_bundle):
        r = requests.post(
            f"{self.base}/v1/domains/{self.domain_id}/search",
            headers=self.h,
            json={"query": query_bundle.query_str, "k": self.k},
        )
        r.raise_for_status()
        from llama_index.core.schema import TextNode
        return [
            NodeWithScore(node=TextNode(text=h["text"]), score=h["score"])
            for h in r.json()
        ]
```

Y úsalo en un `QueryEngine`:

```python
from llama_index.core import QueryBundle
from llama_index.core.query_engine import RetrieverQueryEngine
from llama_index.llms.openai import OpenAI

retriever = NucleusRetriever("http://127.0.0.1:8080", TOKEN, domain_id=1, k=5)
engine = RetrieverQueryEngine.from_args(retriever, llm=OpenAI(model="gpt-4o-mini"))
print(engine.query("¿Tipos de retención de IRPF en 2026?"))
```

## .NET (C#) — con el cliente oficial

El cliente oficial
([`clients/csharp/Nucleus.Client`](../clients/csharp/Nucleus.Client)) cubre toda
la API. Para RAG, combina la recuperación con tu LLM favorito:

```csharp
using Nucleus.Client;

var nucleus = new NucleusClient("http://127.0.0.1:8080", "nuc_...");
var hits = await nucleus.SearchAsync(domainId, new SearchRequest {
    Query = "tipos de retención IRPF 2026", K = 5
});

var context = string.Join("\n\n", hits.Select(h => h.Text));
var prompt = $"Contexto:\n{context}\n\nPregunta: ¿tipos de retención IRPF 2026?";
// → envía `prompt` a tu LLM (OpenAI, Azure OpenAI, Ollama, etc.)
```

El cliente se publica en NuGet como `NucleusDatabase.Client`
([instalación](nuget.md)).

## Node.js / TypeScript — con el cliente oficial

```typescript
import { NucleusClient } from "nucleus-client";

const nucleus = new NucleusClient("http://127.0.0.1:8080", "nuc_...");
const hits = await nucleus.search(domainId, { query: "retención IRPF 2026", k: 5 });

const context = hits.map(h => h.text).join("\n\n");
// → inyecta `context` en tu prompt y llama a tu LLM
```

## ¿Cuándo conviene el modo embebido (DLL) en vez de HTTP?

Para apps de **escritorio o servidor en un solo proceso** (privacidad total, sin
red), referencia la cdylib de Nucleus directamente:

- **C#/.NET**: [`clients/csharp/Nucleus.Native`](../clients/csharp/Nucleus.Native)
  (P/Invoke sobre `nucleus.dll`).
- **Rust**: usa el crate [`nucleus-core`](../crates/core) directamente.
- **C/C++/otros**: el C ABI de [`crates/ffi`](../crates/ffi) vía `nucleus.h`.

El modo embebido **no tiene HTTP ni sidecar**: «SQLite, pero para RAG con
embeddings dentro».

## Aprovechar el etiquetado y los filtros

Nucleus indexa por **dominio → subdominio → documento → chunk** con **labels**
transversales. En RAG, usarlos mejora mucho la precisión:

- Ingiere con `subdomain` y `labels` por nombre (se auto-crean).
- En la recuperación, acota con `subdomain`, `filter` (`tag:2026 AND NOT tag:borrador`)
  o `document_ids`.

Ver el [lenguaje de consulta](lenguaje-consulta.md) y los
[ejemplos](examples/).
