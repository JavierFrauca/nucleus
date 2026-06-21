# Lenguaje de consulta (`filter`)

El campo `filter` de la búsqueda permite una **expresión booleana** que restringe los
chunks candidatos. Se interseca con el resto de filtros (`subdomain`, `tags`,
`document_ids`) y con la búsqueda vectorial.

Se resuelve por **álgebra de conjuntos sobre los índices secundarios** (no escaneando
cada chunk): cada término se traduce a un conjunto de chunks vía índice, y se combinan
con ∩ (AND), ∪ (OR) y ∖ (NOT, sobre el universo de chunks del dominio).

## Gramática

```text
expr    := or
or      := and ( "OR"  and )*
and     := unary ( "AND" unary )*
unary   := "NOT" unary | primary
primary := "(" expr ")" | term
term    := "tag:" VALUE | "doc:" NUMBER | "meta." KEY ":" VALUE
```

- Palabras clave `AND`, `OR`, `NOT` **insensibles a mayúsculas**.
- **`AND` liga más fuerte que `OR`**: `a OR b AND c` ≡ `a OR (b AND c)`.
- `VALUE` es una palabra suelta o una cadena `"entre comillas"` (las comillas permiten
  espacios).

## Términos

| Término | Significado |
|---------|-------------|
| `tag:<nombre>` | El chunk lleva la etiqueta (label) con ese **nombre** (resuelto dentro del dominio). |
| `doc:<id>` | El chunk pertenece a ese documento. |
| `meta.<clave>:<valor>` | El metadato `clave` del chunk es igual a `valor`. |

> Los chunks **heredan** del documento sus labels y su metadata, así que `tag:` y
> `meta.*` operan sobre lo que aportaste al ingestar.

## Ejemplos

```text
tag:legal
tag:legal AND NOT tag:borrador
tag:legal AND (meta.lang:es OR meta.lang:en)
doc:42 OR tag:"contrato marco"
NOT tag:derogado
meta.tipo:ley AND tag:2026
```

Uso en la búsqueda:

```bash
curl -s -X POST $BASE/v1/domains/1/search \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"query":"tributos","filter":"tag:2026 AND NOT tag:borrador","k":5}'
```

## Errores

Una expresión mal formada devuelve `400` con un mensaje, por ejemplo:
- `tag legal` → falta el `:` (`expected `field:value``).
- `doc:abc` → el id de `doc` debe ser numérico.
- `(tag:a` → falta el `)`.
- `weird:x` → campo desconocido (usa `tag`, `doc`, `meta.*`).

## Notas

- `tag:<nombre>` desconocido simplemente **no coincide** (conjunto vacío), no es error.
- Para filtros por **subdominio** usa el campo `subdomain` de la búsqueda (no el
  lenguaje de consulta).
- Un lenguaje de query más rico (rangos, comparadores) es trabajo futuro.
