# NucleusDatabase.Client

Typed .NET client for the [Nucleus](https://github.com/JavierFrauca/nucleus) RAG database HTTP API.

Nucleus is an all-in-one RAG engine: it stores, chunks, embeds (in-process) and indexes your
documents, then answers natural-language searches with hybrid (vector + BM25) retrieval and
optional reranking. This client talks to a running `nucleus-server` instance over HTTP.

Targets `netstandard2.0` and `net8.0` — works on .NET Framework 4.6.1+, .NET Core, .NET 5–10,
Unity and Xamarin.

## Install

```bash
dotnet add package NucleusDatabase.Client
```

## Quick start

```csharp
using Nucleus.Client;

using var nucleus = new NucleusClient("http://127.0.0.1:8080", "nuc_your_token");

// Admin: create a domain (each domain pins an embedding model).
var domain = await nucleus.CreateDomainAsync("fiscal");

// Upload a raw file (PDF/DOCX/XLSX/HTML/MD/TXT) — extracted in-engine.
byte[] bytes = File.ReadAllBytes("IRPF_2026.pdf");
await nucleus.UploadFileAsync(
    domain.Id, "IRPF_2026.pdf", bytes,
    subdomain: "irpf", labels: new[] { "2026", "irpf" });

// Search (hybrid retrieval; optional rerank server-side).
var hits = await nucleus.SearchAsync(domain.Id, new SearchRequest
{
    Query = "tipos de retención de IRPF en 2026",
    K = 5,
    Subdomain = "irpf"
});

foreach (var h in hits)
    Console.WriteLine($"{h.Score:F3}  {h.Text}");
```

## Highlights

- **Hybrid search**: dense (vector) + sparse (BM25) fused with RRF, plus optional cross-encoder
  reranking and MMR diversity.
- **Transparent ingest**: upload raw files (PDF, DOCX, XLSX, HTML, MD, TXT…) or text; the engine
  extracts, chunks, embeds and indexes.
- **Structure**: organize by domain → subdomain → document → chunk, with labels for filtering.
- **Async ingest**: ingestion returns a `job_id`; poll `GetJobAsync` until `Done`.
- **Scoped tokens**: the token you pass determines read/write/admin access per domain.

## Error handling

Errors surface as `NucleusApiException` with `.StatusCode` and a message:

```csharp
try
{
    var hits = await nucleus.SearchAsync(domain.Id, req);
}
catch (NucleusApiException ex) when (ex.StatusCode == 403)
{
    // token lacks Read scope on this domain
}
```

## Links

- 📦 [NuGet](https://www.nuget.org/packages/NucleusDatabase.Client)
- 📖 [Nucleus docs](https://github.com/JavierFrauca/nucleus/tree/main/docs)
- 🚀 [Quick start (curl/PowerShell)](https://github.com/JavierFrauca/nucleus/blob/main/docs/guia-rapida.md)
- 🔌 [RAG integration examples (LangChain, LlamaIndex, .NET, Node)](https://github.com/JavierFrauca/nucleus/blob/main/docs/integrations.md)
- 🏠 [Repository](https://github.com/JavierFrauca/nucleus)

## License

MIT OR Apache-2.0.
