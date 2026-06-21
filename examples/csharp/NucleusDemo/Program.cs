// Demo de consola del SDK de Nucleus en C#.
//
// Muestra: cómo referenciar el SDK, inicializar el cliente, y un menú donde cada
// opción es una "pantalla" (operación) que llama a un método del cliente.
//
//   $env:NUCLEUS_TOKEN = "nuc_…"   # token admin
//   dotnet run

using Nucleus.Client;   // <- referencia al SDK (ver el ProjectReference del .csproj)

// --- inicialización -------------------------------------------------------
var baseUrl = Environment.GetEnvironmentVariable("NUCLEUS_URL") ?? "http://127.0.0.1:8080";
var token = Environment.GetEnvironmentVariable("NUCLEUS_TOKEN")
            ?? (args.Length > 0 ? args[0] : "");

if (string.IsNullOrWhiteSpace(token))
{
    Console.WriteLine("Falta el token. Define NUCLEUS_TOKEN o pásalo como primer argumento.");
    return;
}

// Un único cliente, reutilizable (mantiene un HttpClient dentro).
using var nucleus = new NucleusClient(baseUrl, token);

Console.WriteLine($"Conectado a {baseUrl} — ¿listo? {(await nucleus.IsReadyAsync())}");

long domainId = 0;

while (true)
{
    Console.WriteLine(
        "\n=== Nucleus demo ===\n" +
        $"   dominio actual: {(domainId == 0 ? "(ninguno)" : domainId.ToString())}\n" +
        "  1) Crear/usar dominio 'demo'\n" +
        "  2) Ingestar un texto\n" +
        "  3) Buscar\n" +
        "  4) Listar documentos\n" +
        "  5) Backup full\n" +
        "  6) Subir fichero crudo (PDF/DOCX/…)\n" +
        "  0) Salir\n" +
        "Elige opción: ");
    var choice = Console.ReadLine()?.Trim();

    try
    {
        switch (choice)
        {
            case "1": // --- pantalla: crear dominio (admin) ---
                var dom = await nucleus.CreateDomainAsync("demo");
                domainId = dom.Id;
                Console.WriteLine($"  → dominio {dom.Id} ({dom.Model}, dim {dom.Dim})");
                break;

            case "2": // --- pantalla: ingestar texto ---
                if (!EnsureDomain(ref domainId)) break;
                Console.Write("  texto a ingestar: ");
                var text = Console.ReadLine() ?? "";
                var ing = await nucleus.IngestDocumentAsync(domainId, new IngestRequest
                {
                    Title = "nota",
                    Text = text,
                    Labels = { "demo" },
                });
                Console.WriteLine($"  → documento {ing.DocumentId}, job {ing.JobId} (duplicado: {ing.Duplicate})");
                if (ing.JobId != 0) await WaitJob(nucleus, ing.JobId);
                break;

            case "3": // --- pantalla: buscar ---
                if (!EnsureDomain(ref domainId)) break;
                Console.Write("  consulta: ");
                var q = Console.ReadLine() ?? "";
                var hits = await nucleus.SearchAsync(domainId, new SearchRequest { Query = q, K = 5 });
                if (hits.Count == 0) Console.WriteLine("  (sin resultados)");
                foreach (var h in hits)
                    Console.WriteLine($"  • {h.Score:F3}  {Trim(h.Text, 90)}");
                break;

            case "4": // --- pantalla: listar documentos ---
                if (!EnsureDomain(ref domainId)) break;
                var docs = await nucleus.ListDocumentsAsync(domainId, 0, 20);
                Console.WriteLine($"  {docs.Count} documento(s):");
                foreach (var d in docs) Console.WriteLine($"  • #{d.Id} {d.Title}");
                break;

            case "5": // --- pantalla: backup (admin) ---
                var rec = await nucleus.CreateBackupAsync("full");
                Console.WriteLine($"  → backup {rec.Id} ({rec.Bytes} bytes)");
                break;

            case "6": // --- pantalla: subir fichero crudo ---
                if (!EnsureDomain(ref domainId)) break;
                Console.Write("  ruta del fichero: ");
                var path = (Console.ReadLine() ?? "").Trim('"', ' ');
                if (!File.Exists(path)) { Console.WriteLine("  no existe"); break; }
                // Se suben los BYTES crudos; Nucleus extrae el texto en el motor.
                var bytes = await File.ReadAllBytesAsync(path);
                var up = await nucleus.UploadFileAsync(
                    domainId, Path.GetFileName(path), bytes, labels: new[] { "demo" });
                Console.WriteLine($"  → documento {up.DocumentId}, job {up.JobId}, {up.Chars} chars extraídos (dup: {up.Duplicate})");
                if (up.JobId != 0) await WaitJob(nucleus, up.JobId);
                break;

            case "0":
                return;

            default:
                Console.WriteLine("  opción no válida");
                break;
        }
    }
    catch (NucleusApiException ex)
    {
        Console.WriteLine($"  ! error API {ex.StatusCode}: {ex.Message}");
    }
}

// --- helpers --------------------------------------------------------------
static bool EnsureDomain(ref long domainId)
{
    if (domainId != 0) return true;
    Console.WriteLine("  primero crea el dominio (opción 1)");
    return false;
}

static string Trim(string s, int n) => s.Length <= n ? s : s[..n] + "…";

static async Task WaitJob(NucleusClient nucleus, long jobId)
{
    Console.Write("  esperando ingesta");
    for (var i = 0; i < 100; i++)
    {
        var job = await nucleus.GetJobAsync(jobId);
        if (job.Status == "Done") { Console.WriteLine(" ✓"); return; }
        if (job.Status == "Failed") { Console.WriteLine($" ✗ {job.Error}"); return; }
        Console.Write(".");
        await Task.Delay(300);
    }
    Console.WriteLine(" (timeout)");
}
