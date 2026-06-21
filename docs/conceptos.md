# Conceptos

Nucleus es una **base de datos orientada a RAG**: sueltas documentos y preguntas en
lenguaje natural; el motor se encarga de extraer texto, trocear, generar embeddings,
indexar y recuperar. Su objetivo es **reducir el hueco entre una base de datos y un RAG
que funciona**.

## Jerarquía de organización

```
Dominio            (lo define el usuario; fija el modelo de embeddings)
└── Subdominio     (tema dentro del dominio; opcional)
    └── Documento  (referencia siempre a un dominio; opcionalmente a un subdominio)
        └── Chunk  (unidad recuperable, con su embedding)

Labels (etiquetas) — transversales al dominio, se asignan a documentos/chunks
```

### Dominio
Colección/namespace que segmenta la base. **Lo define el usuario.** Cada dominio:
- fija **un modelo** de embeddings (y por tanto una **dimensión**),
- tiene su **propio índice vectorial**,
- agrupa sus subdominios, labels, documentos y chunks.

Un documento **siempre** pertenece a un dominio.

### Subdominio
Un **tema concreto dentro de un dominio** (p. ej. en `fiscal`: `irpf`, `iva`). Es
opcional. En el contrato actual lo **aporta quien ingesta, por nombre**, y el motor lo
crea si no existe. Sirve para **acotar** la búsqueda a un subtema.

### Label (etiqueta / tag)
Faceta transversal del dominio: año (`2025`, `2026`), tipo (`ley`, `norma`, `adr`),
intención (`resumen`, `explica`)… Se asignan a documentos y se **heredan a sus chunks**.
Se aportan **por nombre** en la ingesta (auto-creadas) y permiten filtrar.

> Internamente las labels son la entidad `Tag` (con jerarquía opcional vía `parent`).

### Documento
Lo que ingestas. Tiene `title`, `source` opcional, `metadata` (clave/valor), `tags`
(labels) y opcionalmente `subdomain_id`. Al ingestarlo, el motor lo **trocea en chunks**.

### Chunk
La unidad que se recupera. Contiene el texto, su **embedding**, e **hereda** del
documento sus `tags`, `metadata` y `subdomain`. Los chunks de un documento se **encadenan**
(`prev`/`next`) para poder recuperar contexto alrededor de un resultado.

## Embeddings (dentro del motor)

Nucleus **genera los embeddings in-process** con [fastembed](https://github.com/Anush008/fastembed-rs)
(ONNX Runtime). No hay servicio externo. Modelos soportados:

| id | dim | notas |
|----|-----|-------|
| `multilingual-e5-small` | 384 | **por defecto**, multilingüe (ES + EN) |
| `bge-small-en-v1.5` | 384 | solo inglés |
| `all-minilm-l6-v2` | 384 | solo inglés |

El modelo se fija **por dominio** al crearlo. Los modelos e5 reciben automáticamente los
prefijos `query:` / `passage:`. La primera vez se descarga el modelo de HuggingFace y se
cachea.

## Troceado (chunking)

Si envías un fichero o un `text`, el motor lo trocea con una ventana fija de caracteres
con solapamiento. Si ya tienes los fragmentos, envíalos como `chunks[]` y se usan tal
cual.

## Índice vectorial

Cada dominio tiene un índice en memoria (reconstruible desde el almacenamiento):
- **`flat`** (por defecto): coseno exacto por fuerza bruta. Ideal para exactitud y
  filtros precisos.
- **`hnsw`**: aproximado (grafo HNSW) para gran escala; **persistente** en disco.

Ver [configuración](configuracion.md) y [operación](operacion.md).

## Búsqueda

Una consulta combina:
1. **Vector de consulta**: del `query` (texto, se embebe con el modelo del dominio) o un
   `query_vector` precomputado.
2. **Filtros** (se intersecan): `subdomain`, `tags`/`labels` (y `match_all`),
   `document_ids`, y un `filter` en [lenguaje de consulta](lenguaje-consulta.md) sobre
   tags/doc/metadata.
3. **Ranking** por similitud coseno dentro del conjunto permitido; devuelve los `k`
   mejores chunks.

> Hoy, **la clasificación (dominio/subdominio/labels) la aporta quien ingesta**. La
> auto-inducción de subdominios/labels (clustering + reglas, **sin LLM**) es una fase
> opcional futura.

## Jobs (asincronía)

La ingesta pesada (troceo, embeddings) se ejecuta en una **cola persistida** con workers.
Por eso la ingesta devuelve un `job_id` y se consulta su estado. La cola sobrevive a
reinicios. Ver [operación](operacion.md).

## Seguridad

Todo (salvo `/healthz`) requiere un **token** tipo API-key con **scopes por dominio**
(`Read`/`Write`/`Admin`). Ver [API](api.md#autenticación) y [operación](operacion.md#seguridad).
