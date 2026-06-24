# Nucleus — embedded (DLL) mode

This bundle is Nucleus as an **in-process native library**: no HTTP server, no
sidecar. Your application links `nucleus.dll` and calls the engine directly.

On Windows the DLL is **self-contained** (ONNX Runtime is linked statically), so
there is no `onnxruntime.dll` to ship. The only runtime download is the embedding
model (~450 MB), fetched to the model cache on first ingest/search.

## Contents

| File | Purpose |
|------|---------|
| `nucleus.dll` | The engine. Put it next to your executable (or on the loader path). |
| `nucleus.dll.lib` | Import library, for linking from C/C++ at build time. |
| `nucleus.h` | C header declaring the ABI. |
| `csharp/` | C# P/Invoke binding (`NucleusEngine`) — drop into a .NET project. |

## The ABI in one paragraph

An opaque handle from `nucleus_open` (release with `nucleus_close`). Data calls
take a **JSON** input string and write a **JSON** output string you must free with
`nucleus_string_free`. Every call returns `0` on success or a negative code on
failure, with the message in `{"error": "..."}` and in `nucleus_last_error()`.
See `nucleus.h` for the full list.

## Quick start (C#)

```csharp
using Nucleus.Native;

using var engine = NucleusEngine.Open("data/nucleus.redb", modelCache: "models");
Domain domain = engine.CreateDomain("legal");

engine.IngestText(domain.Id, "Contrato", "texto largo…", labels: ["contratos"]);

// Strongly typed: Search returns IReadOnlyList<SearchHit>, not raw JSON.
foreach (SearchHit hit in engine.Search(domain.Id, "cláusula de rescisión", k: 5, labels: ["contratos"]))
    Console.WriteLine($"{hit.Score:F3}  {hit.Chunk.Text}");
```

The binding maps the engine's snake_case JSON onto PascalCase records (`Domain`,
`Document`, `Chunk`, `SearchHit`…) and omits null fields when serializing inputs
(the engine's optional fields must be **absent** rather than explicitly `null`).

## Quick start (C/C++)

```c
#include "nucleus.h"

NucleusEngine *eng = NULL;
if (nucleus_open("{\"db_path\":\"data/nucleus.redb\"}", &eng) != NUCLEUS_OK) {
    fprintf(stderr, "open: %s\n", nucleus_last_error());
    return 1;
}
char *out = NULL;
nucleus_create_domain(eng, "{\"name\":\"legal\"}", &out);
/* parse `out` (JSON) ... */
nucleus_string_free(out);
nucleus_close(eng);
```

## Notes

- **Threading**: the handle is `Send + Sync`; share it across threads. Calls are
  synchronous (they block on CPU-bound embedding) — the host owns its threading.
- **Index backend**: `index_kind` is `"flat"` (exact, default) or `"hnsw"`
  (approximate, for large domains). With HNSW, set `index_dir` and call
  `nucleus_persist_indexes` to dump the graph to disk.
- **Storage**: a single ACID `.redb` file at `db_path`.
