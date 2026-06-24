using System;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Nucleus.Native;

/// <summary>
/// In-process binding to <c>nucleus.dll</c> via P/Invoke. Unlike <c>Nucleus.Client</c>
/// (which talks HTTP to the server), this embeds the engine directly: no network,
/// no sidecar. Drop <c>nucleus.dll</c> next to your executable (it is self-contained
/// on Windows — ONNX Runtime is linked statically); the embedding model is downloaded
/// to the model cache on first use.
/// </summary>
public sealed class NucleusEngine : IDisposable
{
    private const string Dll = "nucleus"; // resolves to nucleus.dll / libnucleus.so

    // Input: the engine's `#[serde(default)]` fields only kick in when a key is
    // *absent*, not when it is explicitly null — so omit nulls rather than emit them.
    private static readonly JsonSerializerOptions JsonIn = new()
    {
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
    };

    // Output: the engine serializes its structs in snake_case; map them onto the
    // PascalCase record properties in Models.cs.
    private static readonly JsonSerializerOptions JsonOut = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower,
        PropertyNameCaseInsensitive = true,
    };

    private IntPtr _handle;

    private NucleusEngine(IntPtr handle) => _handle = handle;

    /// <summary>Open (or create) a database. See the FFI docs for config fields.</summary>
    public static NucleusEngine Open(
        string dbPath,
        string? modelCache = null,
        string? indexDir = null,
        string indexKind = "flat",
        bool gpu = false)
    {
        var config = JsonSerializer.Serialize(new
        {
            db_path = dbPath,
            model_cache = modelCache,
            index_dir = indexDir,
            index_kind = indexKind,
            gpu,
        }, JsonIn);

        int code = nucleus_open(config, out IntPtr handle);
        if (code != 0)
            throw new NucleusException(code, LastError() ?? "nucleus_open failed");
        return new NucleusEngine(handle);
    }

    /// <summary>Create a domain (namespace).</summary>
    public Domain CreateDomain(string name, string? model = null) =>
        Call<Domain>(nucleus_create_domain, new { name, model });

    /// <summary>Ingest one document synchronously (chunk → embed → persist → index).</summary>
    public IngestResult IngestText(
        ulong domainId,
        string title,
        string text,
        string? source = null,
        IDictionary<string, string>? metadata = null,
        IEnumerable<string>? labels = null,
        string? subdomain = null) =>
        Call<IngestResult>(nucleus_ingest_text, new
        {
            domain_id = domainId,
            title,
            text,
            source,
            metadata,
            labels,
            subdomain,
        });

    /// <summary>Search a domain by text.</summary>
    /// <param name="diversity">MMR diversity in [0,1]; 0 = pure relevance.</param>
    public IReadOnlyList<SearchHit> Search(
        ulong domainId,
        string query,
        int k = 10,
        IEnumerable<string>? labels = null,
        bool matchAll = false,
        IEnumerable<ulong>? documentIds = null,
        string? subdomain = null,
        string? filter = null,
        float diversity = 0f) =>
        Call<HitsEnvelope>(nucleus_search, new
        {
            domain_id = domainId,
            query,
            k,
            labels,
            match_all = matchAll,
            document_ids = documentIds,
            subdomain,
            filter,
            diversity,
        }).Hits;

    /// <summary>Search several domains at once (they must share a model).</summary>
    public IReadOnlyList<SearchHit> SearchMulti(
        IEnumerable<ulong> domainIds,
        string query,
        int k = 10,
        string? filter = null,
        float diversity = 0f) =>
        Call<HitsEnvelope>(nucleus_search_multi, new { domain_ids = domainIds, query, k, filter, diversity }).Hits;

    /// <summary>Persist on-disk (HNSW) index dumps. Returns how many were written.</summary>
    public int PersistIndexes() =>
        CallNoArg<PersistedEnvelope>(nucleus_persist_indexes).Persisted;

    /// <summary>List all domains.</summary>
    public IReadOnlyList<Domain> ListDomains() =>
        CallNoArg<DomainsEnvelope>(nucleus_list_domains).Domains;

    /// <summary>List labels (tags) in a domain.</summary>
    public IReadOnlyList<Tag> ListTags(ulong domainId) =>
        Call<TagsEnvelope>(nucleus_list_tags, new { domain_id = domainId }).Tags;

    /// <summary>List subdomains in a domain.</summary>
    public IReadOnlyList<Subdomain> ListSubdomains(ulong domainId) =>
        Call<SubdomainsEnvelope>(nucleus_list_subdomains, new { domain_id = domainId }).Subdomains;

    /// <summary>Paginated document listing.</summary>
    public IReadOnlyList<Document> ListDocuments(ulong domainId, int offset = 0, int limit = 100) =>
        Call<DocumentsEnvelope>(nucleus_list_documents, new { domain_id = domainId, offset, limit }).Documents;

    /// <summary>Fetch one document by id.</summary>
    public Document GetDocument(ulong documentId) =>
        Call<Document>(nucleus_get_document, new { document_id = documentId });

    /// <summary>Delete a document and its chunks.</summary>
    public void DeleteDocument(ulong documentId) =>
        Call<DeletedEnvelope>(nucleus_delete_document, new { document_id = documentId });

    /// <summary>A chunk plus its neighbours, in document order.</summary>
    public IReadOnlyList<Chunk> ChunkContext(ulong chunkId, int before = 1, int after = 1) =>
        Call<ChunksEnvelope>(nucleus_chunk_context, new { chunk_id = chunkId, before, after }).Chunks;

    // --- edit / delete (cascade) ------------------------------------------

    /// <summary>Rename a domain.</summary>
    public Domain RenameDomain(ulong domainId, string name) =>
        Call<Domain>(nucleus_rename_domain, new { domain_id = domainId, name });

    /// <summary>Delete a domain and everything under it.</summary>
    public void DeleteDomain(ulong domainId) =>
        Call<DeletedEnvelope>(nucleus_delete_domain, new { domain_id = domainId });

    /// <summary>Delete a subdomain and cascade to its documents.</summary>
    public void DeleteSubdomain(ulong subdomainId) =>
        Call<DeletedEnvelope>(nucleus_delete_subdomain, new { subdomain_id = subdomainId });

    /// <summary>Update a label's display name and/or description.</summary>
    public Tag UpdateTag(ulong tagId, string? displayName = null, string? description = null) =>
        Call<Tag>(nucleus_update_tag, new { tag_id = tagId, display_name = displayName, description });

    /// <summary>Delete a label, detaching it from chunks/documents (which survive).</summary>
    public void DeleteTag(ulong tagId) =>
        Call<DeletedEnvelope>(nucleus_delete_tag, new { tag_id = tagId });

    /// <summary>Re-assign a document's labels and/or subdomain.</summary>
    public Document UpdateDocument(
        ulong documentId,
        IEnumerable<string>? labels = null,
        string? subdomain = null,
        bool clearSubdomain = false) =>
        Call<Document>(nucleus_update_document, new
        {
            document_id = documentId,
            labels,
            subdomain,
            clear_subdomain = clearSubdomain,
        });

    /// <summary>Re-embed a domain and rebuild its index (blocking). Returns the chunk count.</summary>
    public int ReindexDomain(ulong domainId, string? model = null) =>
        Call<ReindexedEnvelope>(nucleus_reindex_domain, new { domain_id = domainId, model }).Reindexed;

    public void Dispose()
    {
        if (_handle != IntPtr.Zero)
        {
            nucleus_close(_handle);
            _handle = IntPtr.Zero;
        }
    }

    // --- internals ---------------------------------------------------------

    private delegate int Op(IntPtr handle, string inputJson, out IntPtr outJson);
    private delegate int NoArgOp(IntPtr handle, out IntPtr outJson);

    private T Call<T>(Op op, object input)
    {
        string json = JsonSerializer.Serialize(input, JsonIn);
        int code = op(_handle, json, out IntPtr outJson);
        return Finish<T>(code, outJson);
    }

    private T CallNoArg<T>(NoArgOp op)
    {
        int code = op(_handle, out IntPtr outJson);
        return Finish<T>(code, outJson);
    }

    private static T Finish<T>(int code, IntPtr outJson)
    {
        string? payload = outJson == IntPtr.Zero ? null : Marshal.PtrToStringUTF8(outJson);
        if (outJson != IntPtr.Zero) nucleus_string_free(outJson);

        if (code != 0)
            throw new NucleusException(code, ErrorMessage(payload) ?? LastError() ?? "engine call failed");
        return JsonSerializer.Deserialize<T>(payload ?? "null", JsonOut)
               ?? throw new NucleusException(code, "engine returned null payload");
    }

    /// <summary>Pull the message out of a <c>{"error":"..."}</c> failure payload.</summary>
    private static string? ErrorMessage(string? payload)
    {
        if (payload is null) return null;
        try
        {
            using var doc = JsonDocument.Parse(payload);
            return doc.RootElement.TryGetProperty("error", out var e) ? e.GetString() : null;
        }
        catch (JsonException) { return null; }
    }

    private static string? LastError()
    {
        IntPtr p = nucleus_last_error();
        return p == IntPtr.Zero ? null : Marshal.PtrToStringUTF8(p);
    }

    // Envelopes for the engine's keyed JSON outputs.
    private sealed record HitsEnvelope(List<SearchHit> Hits);
    private sealed record DomainsEnvelope(List<Domain> Domains);
    private sealed record TagsEnvelope(List<Tag> Tags);
    private sealed record SubdomainsEnvelope(List<Subdomain> Subdomains);
    private sealed record DocumentsEnvelope(List<Document> Documents);
    private sealed record ChunksEnvelope(List<Chunk> Chunks);
    private sealed record DeletedEnvelope(bool Deleted);
    private sealed record PersistedEnvelope(int Persisted);
    private sealed record ReindexedEnvelope(int Reindexed);

    // --- P/Invoke ----------------------------------------------------------

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_open(string configJson, out IntPtr outHandle);

    [DllImport(Dll)]
    private static extern void nucleus_close(IntPtr handle);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_create_domain(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_ingest_text(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_search(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll)]
    private static extern int nucleus_persist_indexes(IntPtr handle, out IntPtr outJson);

    [DllImport(Dll)]
    private static extern int nucleus_list_domains(IntPtr handle, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_list_tags(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_list_subdomains(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_list_documents(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_get_document(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_delete_document(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_chunk_context(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_search_multi(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_rename_domain(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_delete_domain(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_delete_subdomain(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_update_tag(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_delete_tag(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_update_document(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll, CharSet = CharSet.Ansi)]
    private static extern int nucleus_reindex_domain(IntPtr handle, string inputJson, out IntPtr outJson);

    [DllImport(Dll)]
    private static extern void nucleus_string_free(IntPtr s);

    [DllImport(Dll)]
    private static extern IntPtr nucleus_last_error();
}

/// <summary>Error from a Nucleus FFI call. <see cref="Code"/> is the C status code.</summary>
public sealed class NucleusException(int code, string message)
    : Exception($"nucleus error {code}: {message}")
{
    public int Code { get; } = code;
}
