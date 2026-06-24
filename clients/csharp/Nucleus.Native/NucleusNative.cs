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

    // The engine's `#[serde(default)]` fields only kick in when a key is *absent*,
    // not when it is explicitly null — so omit nulls rather than emitting them.
    private static readonly JsonSerializerOptions JsonOpts = new()
    {
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
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
        }, JsonOpts);

        int code = nucleus_open(config, out IntPtr handle);
        if (code != 0)
            throw new NucleusException(code, LastError() ?? "nucleus_open failed");
        return new NucleusEngine(handle);
    }

    /// <summary>Create a domain (namespace). Returns the raw JSON Domain object.</summary>
    public JsonDocument CreateDomain(string name, string? model = null) =>
        Call(nucleus_create_domain, new { name, model });

    /// <summary>Ingest one document synchronously (chunk → embed → persist → index).</summary>
    public JsonDocument IngestText(
        ulong domainId,
        string title,
        string text,
        string? source = null,
        IDictionary<string, string>? metadata = null,
        IEnumerable<string>? labels = null,
        string? subdomain = null) =>
        Call(nucleus_ingest_text, new
        {
            domain_id = domainId,
            title,
            text,
            source,
            metadata,
            labels,
            subdomain,
        });

    /// <summary>Search a domain by text. Returns <c>{ "hits": [ { chunk, score, snippet? } ] }</c>.</summary>
    /// <param name="diversity">MMR diversity in [0,1]; 0 = pure relevance.</param>
    public JsonDocument Search(
        ulong domainId,
        string query,
        int k = 10,
        IEnumerable<string>? labels = null,
        bool matchAll = false,
        IEnumerable<ulong>? documentIds = null,
        string? subdomain = null,
        string? filter = null,
        float diversity = 0f) =>
        Call(nucleus_search, new
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
        });

    /// <summary>Search several domains at once (they must share a model).</summary>
    public JsonDocument SearchMulti(
        IEnumerable<ulong> domainIds,
        string query,
        int k = 10,
        string? filter = null,
        float diversity = 0f) =>
        Call(nucleus_search_multi, new { domain_ids = domainIds, query, k, filter, diversity });

    /// <summary>Persist on-disk (HNSW) index dumps. No-op for the flat index.</summary>
    public JsonDocument PersistIndexes()
    {
        int code = nucleus_persist_indexes(_handle, out IntPtr outJson);
        return Finish(code, outJson);
    }

    /// <summary>List all domains. Returns <c>{ "domains": [ ... ] }</c>.</summary>
    public JsonDocument ListDomains()
    {
        int code = nucleus_list_domains(_handle, out IntPtr outJson);
        return Finish(code, outJson);
    }

    /// <summary>List labels (tags) in a domain. Returns <c>{ "tags": [ ... ] }</c>.</summary>
    public JsonDocument ListTags(ulong domainId) =>
        Call(nucleus_list_tags, new { domain_id = domainId });

    /// <summary>List subdomains in a domain. Returns <c>{ "subdomains": [ ... ] }</c>.</summary>
    public JsonDocument ListSubdomains(ulong domainId) =>
        Call(nucleus_list_subdomains, new { domain_id = domainId });

    /// <summary>Paginated document listing. Returns <c>{ "documents": [ ... ] }</c>.</summary>
    public JsonDocument ListDocuments(ulong domainId, int offset = 0, int limit = 100) =>
        Call(nucleus_list_documents, new { domain_id = domainId, offset, limit });

    /// <summary>Fetch one document by id.</summary>
    public JsonDocument GetDocument(ulong documentId) =>
        Call(nucleus_get_document, new { document_id = documentId });

    /// <summary>Delete a document and its chunks. Returns <c>{ "deleted": true }</c>.</summary>
    public JsonDocument DeleteDocument(ulong documentId) =>
        Call(nucleus_delete_document, new { document_id = documentId });

    /// <summary>A chunk plus its neighbours. Returns <c>{ "chunks": [ ... ] }</c>.</summary>
    public JsonDocument ChunkContext(ulong chunkId, int before = 1, int after = 1) =>
        Call(nucleus_chunk_context, new { chunk_id = chunkId, before, after });

    // --- edit / delete (cascade) ------------------------------------------

    /// <summary>Rename a domain. Returns the updated <c>Domain</c>.</summary>
    public JsonDocument RenameDomain(ulong domainId, string name) =>
        Call(nucleus_rename_domain, new { domain_id = domainId, name });

    /// <summary>Delete a domain and everything under it. Returns <c>{ "deleted": true }</c>.</summary>
    public JsonDocument DeleteDomain(ulong domainId) =>
        Call(nucleus_delete_domain, new { domain_id = domainId });

    /// <summary>Delete a subdomain and cascade to its documents. Returns <c>{ "deleted": true }</c>.</summary>
    public JsonDocument DeleteSubdomain(ulong subdomainId) =>
        Call(nucleus_delete_subdomain, new { subdomain_id = subdomainId });

    /// <summary>Update a label's display name and/or description. Returns the <c>Tag</c>.</summary>
    public JsonDocument UpdateTag(ulong tagId, string? displayName = null, string? description = null) =>
        Call(nucleus_update_tag, new { tag_id = tagId, display_name = displayName, description });

    /// <summary>Delete a label, detaching it from chunks/documents. Returns <c>{ "deleted": true }</c>.</summary>
    public JsonDocument DeleteTag(ulong tagId) =>
        Call(nucleus_delete_tag, new { tag_id = tagId });

    /// <summary>Re-assign a document's labels and/or subdomain. Returns the <c>Document</c>.</summary>
    public JsonDocument UpdateDocument(
        ulong documentId,
        IEnumerable<string>? labels = null,
        string? subdomain = null,
        bool clearSubdomain = false) =>
        Call(nucleus_update_document, new
        {
            document_id = documentId,
            labels,
            subdomain,
            clear_subdomain = clearSubdomain,
        });

    /// <summary>Re-embed a domain and rebuild its index (blocking). Returns <c>{ "reindexed": N }</c>.</summary>
    public JsonDocument ReindexDomain(ulong domainId, string? model = null) =>
        Call(nucleus_reindex_domain, new { domain_id = domainId, model });

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

    private JsonDocument Call(Op op, object input)
    {
        string json = JsonSerializer.Serialize(input, JsonOpts);
        int code = op(_handle, json, out IntPtr outJson);
        return Finish(code, outJson);
    }

    private static JsonDocument Finish(int code, IntPtr outJson)
    {
        string? payload = outJson == IntPtr.Zero ? null : Marshal.PtrToStringUTF8(outJson);
        if (outJson != IntPtr.Zero) nucleus_string_free(outJson);

        if (code != 0)
            throw new NucleusException(code, payload ?? LastError() ?? "engine call failed");
        return JsonDocument.Parse(payload ?? "null");
    }

    private static string? LastError()
    {
        IntPtr p = nucleus_last_error();
        return p == IntPtr.Zero ? null : Marshal.PtrToStringUTF8(p);
    }

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
