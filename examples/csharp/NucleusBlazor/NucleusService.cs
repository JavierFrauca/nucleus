using Nucleus.Native;

namespace NucleusBlazor;

/// <summary>
/// Owns a single embedded <see cref="NucleusEngine"/> for the whole app (registered
/// as a singleton). The engine runs in-process — no HTTP server, no network. The
/// handle is thread-safe, so all Blazor circuits share it.
/// </summary>
public sealed class NucleusService : IDisposable
{
    private readonly NucleusEngine _engine;

    /// <summary>The demo domain all operations target.</summary>
    public ulong DomainId { get; }

    public NucleusService()
    {
        var dir = Path.Combine(Path.GetTempPath(), "nucleus-blazor");
        Directory.CreateDirectory(dir);
        // Opening (and creating the domain) does NOT download the model — only the
        // first ingest/search does, since that is the first time we embed text.
        _engine = NucleusEngine.Open(
            Path.Combine(dir, "demo.redb"),
            modelCache: Path.Combine(dir, "models"));

        Domain domain = _engine.ListDomains().FirstOrDefault(d => d.Name == "demo")
                        ?? _engine.CreateDomain("demo");
        DomainId = domain.Id;
    }

    /// <summary>Ingest one document (chunk → embed → index). Blocking — call off the UI thread.</summary>
    public IngestResult Ingest(string title, string text, string? label) =>
        _engine.IngestText(
            DomainId,
            string.IsNullOrWhiteSpace(title) ? "Sin título" : title,
            text,
            labels: string.IsNullOrWhiteSpace(label) ? null : [label.Trim()]);

    /// <summary>Hybrid search. Blocking — call off the UI thread.</summary>
    public IReadOnlyList<SearchHit> Search(string query, int k = 5) =>
        _engine.Search(DomainId, query, k: k);

    /// <summary>How many documents are currently indexed.</summary>
    public int DocumentCount() => _engine.ListDocuments(DomainId).Count;

    public void Dispose() => _engine.Dispose();
}
