# Nucleus.Client (.NET)

Typed C# client for the [Nucleus](../../README.md) RAG database HTTP API.
Targets `netstandard2.0` and `net8.0` (works on .NET Framework 4.6.1+, .NET Core,
.NET 5–10, Unity, Xamarin).

## Install

From source (project reference):

```xml
<ItemGroup>
  <ProjectReference Include="path/to/clients/csharp/Nucleus.Client/Nucleus.Client.csproj" />
</ItemGroup>
```

Or build a NuGet package:

```bash
dotnet pack clients/csharp/Nucleus.Client -c Release   # -> bin/Release/Nucleus.Client.0.1.0.nupkg
```

## Usage

```csharp
using Nucleus.Client;

using var nucleus = new NucleusClient("http://127.0.0.1:8080", "nuc_your_token");

// Admin: create a domain.
var domain = await nucleus.CreateDomainAsync("fiscal");

// Upload a raw file (PDF/DOCX/XLSX/HTML/MD/TXT) — extracted in-engine.
byte[] bytes = File.ReadAllBytes("IRPF_2026.pdf");
var up = await nucleus.UploadFileAsync(
    domain.Id, "IRPF_2026.pdf", bytes,
    subdomain: "irpf", labels: new[] { "2026", "irpf" });

// …or ingest text/chunks directly.
await nucleus.IngestDocumentAsync(domain.Id, new IngestRequest
{
    Title = "nota",
    Text = "tipos de retención de IRPF para 2026…",
    Subdomain = "irpf",
    Labels = { "2026" }
});

// Search (hybrid retrieval; optional rerank server-side).
var hits = await nucleus.SearchAsync(domain.Id, new SearchRequest
{
    Query = "tipos de retención de IRPF en 2026",
    K = 5,
    Subdomain = "irpf"
});
foreach (var h in hits)
    Console.WriteLine($"{h.Score:F3}  {h.Text[..Math.Min(100, h.Text.Length)]}");

// Create a scoped token (admin).
var token = await nucleus.CreateTokenAsync("app-lectura",
    new[] { Scope.ForDomain(domain.Id, Perm.Read) });
Console.WriteLine(token.Token); // shown once
```

Errors surface as `NucleusApiException` (`.StatusCode` + message). Ingestion is
asynchronous: poll `GetJobAsync(jobId)` until `Status == "Done"`.
