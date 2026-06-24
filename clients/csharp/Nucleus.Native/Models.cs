namespace Nucleus.Native;

// Strongly-typed mirrors of the engine's serialized entities. Field names map to
// the Rust structs' snake_case JSON via JsonNamingPolicy.SnakeCaseLower (see
// NucleusEngine.JsonOut). Ids are plain numbers (serde `transparent` newtypes).

/// <summary>A namespace/collection. Pins an embedding model (and its dimension).</summary>
public sealed record Domain(ulong Id, string Name, string Model, int Dim, long CreatedAt);

/// <summary>A topic within a domain.</summary>
public sealed record Subdomain(ulong Id, ulong DomainId, string Name, string Description, long CreatedAt);

/// <summary>A label (tag) within a domain.</summary>
public sealed record Tag(
    ulong Id,
    ulong DomainId,
    string Name,
    string DisplayName,
    string Description,
    ulong? Parent,
    long CreatedAt);

/// <summary>An ingested document (its chunks are stored separately).</summary>
public sealed record Document(
    ulong Id,
    ulong DomainId,
    ulong? SubdomainId,
    string Title,
    string? Source,
    Dictionary<string, string> Metadata,
    List<ulong> Tags,
    long CreatedAt);

/// <summary>A retrievable unit of text. Chunks are chained via Prev/Next.</summary>
public sealed record Chunk(
    ulong Id,
    ulong DocumentId,
    ulong DomainId,
    ulong? SubdomainId,
    uint Ordinal,
    string Text,
    List<ulong> Tags,
    Dictionary<string, string> Metadata,
    ulong? Prev,
    ulong? Next);

/// <summary>One ranked search result.</summary>
public sealed record SearchHit(Chunk Chunk, float Score, string? Snippet);

/// <summary>Outcome of a synchronous ingest. <c>Chars</c> is set for file ingests.</summary>
public sealed record IngestResult(ulong DocumentId, int ChunkCount, int Chars = 0);
