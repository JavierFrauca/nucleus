# Cliente .NET (NuGet)

[`Nucleus.Client`](../clients/csharp/Nucleus.Client) es el cliente HTTP tipado
para .NET, que habla con `nucleus-server`. Se publica en **NuGet.org** como
`Nucleus.Client` automáticamente con cada *tag* de versión.

> Para el **modo embebido** (sin HTTP, P/Invoke sobre `nucleus.dll`), usa
> [`Nucleus.Native`](../clients/csharp/Nucleus.Native), que no va a NuGet sino
> que se referencia junto al bundle de la DLL.

## Instalar

### Desde NuGet (recomendado)

```bash
dotnet add package Nucleus.Client
```

```xml
<!-- O directamente en el .csproj -->
<PackageReference Include="Nucleus.Client" Version="1.1.0" />
```

> El paquete apunta a `netstandard2.0` y `net8.0`: funciona en .NET Framework
> 4.6.1+, .NET Core y .NET 5–10, además de Unity/Xamarin.

### Desde fuente (project reference)

```xml
<ItemGroup>
  <ProjectReference Include="path/to/clients/csharp/Nucleus.Client/Nucleus.Client.csproj" />
</ItemGroup>
```

### Compilar el paquete localmente

```bash
dotnet pack clients/csharp/Nucleus.Client -c Release
# -> clients/csharp/Nucleus.Client/bin/Release/Nucleus.Client.<version>.nupkg
```

## Uso rápido

```csharp
using Nucleus.Client;

using var nucleus = new NucleusClient("http://127.0.0.1:8080", "nuc_your_token");

// Crear un dominio (requiere scope Admin).
var domain = await nucleus.CreateDomainAsync("fiscal");

// Subir un fichero crudo (PDF/DOCX/XLSX/HTML/MD/TXT) — se extrae en el motor.
byte[] bytes = File.ReadAllBytes("IRPF_2026.pdf");
var up = await nucleus.UploadFileAsync(
    domain.Id, "IRPF_2026.pdf", bytes,
    subdomain: "irpf", labels: new[] { "2026", "irpf" });

// Ingestar texto/chunks directamente.
await nucleus.IngestDocumentAsync(domain.Id, new IngestRequest {
    Title = "nota",
    Text = "tipos de retención de IRPF para 2026…",
    Subdomain = "irpf",
    Labels = { "2026" }
});

// Buscar (recuperación híbrida; rerank opcional servidor).
var hits = await nucleus.SearchAsync(domain.Id, new SearchRequest {
    Query = "tipos de retención de IRPF en 2026",
    K = 5,
    Subdomain = "irpf"
});
foreach (var h in hits)
    Console.WriteLine($"{h.Score:F3}  {h.Text[..Math.Min(100, h.Text.Length)]}");

// Crear un token con scope limitado (Admin).
var token = await nucleus.CreateTokenAsync("app-lectura",
    new[] { Scope.ForDomain(domain.Id, Perm.Read) });
Console.WriteLine(token.Token); // el secreto se muestra una sola vez
```

Los errores llegan como `NucleusApiException` (con `.StatusCode` y mensaje). La
**ingesta es asíncrona**: sondea `GetJobAsync(jobId)` hasta `Status == "Done"`.

## Versiones y SemVer

- El paquete NuGet sigue la **misma versión** que el tag (`v1.1.0` → NuGet
  `1.1.0`).
- Desde la 1.0, la API HTTP (y por tanto este cliente) sigue
  [SemVer](camino-a-1.0.md): un cambio incompatible sube el *major*.

## Cómo se publica (Trusted Publishing, OIDC)

La publicación es **automática** al pushear un tag (`v*`) y no requiere guardar
una API key en el repo. El flujo (en
[`.github/workflows/release.yml`](../.github/workflows/release.yml)):

1. `dotnet pack` con la versión del tag.
2. **Trusted Publishing (OIDC)**: GitHub emite un token OIDC que NuGet.org
   canjea por una API key válida **1 hora** (no hay `NUGET_API_KEY` persistente).
3. `dotnet nuget push` publica en NuGet.org.

### Configuración one-time en nuget.org (solo la primera vez)

Para que Trusted Publishing funcione, una vez en
[nuget.org](https://www.nuget.org) → Account settings → Trusted Publishing:

- **Owner**: tu usuario/org de GitHub (p. ej. `JavierFrauca`).
- **Repository**: `nucleus`.
- **Workflow file**: `release.yml`.
- **Environment**: (vacío).

Y en el repo, añade el secret **`NUGET_USER`** con tu usuario de nuget.org (es
un nombre, **no** una clave).

> Hasta configurarlo, el job `nuget-publish` falla sin bloquear el GitHub
> Release (son independientes). Mientras tanto, referencia el proyecto
> directamente desde fuente.

## Reportar problemas con el paquete

Si el paquete no se publica o falla al instalar, abre un
[issue](https://github.com/JavierFrauca/nucleus/issues) con label `packaging` y
el log del workflow `release.yml`.
