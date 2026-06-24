using System.Text.Json;
using Nucleus.Native;

// --- default path probe: open with NO db_path -> per-user %LOCALAPPDATA%\Nucleus.
{
    using var def = NucleusEngine.Open(dbPath: ""); // empty -> engine picks the default
    var expected = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "Nucleus", "nucleus.redb");
    Console.WriteLine($"[default-path] opened with no db_path; expected at {expected}: exists={File.Exists(expected)}");
}

// End-to-end smoke test for nucleus.dll exercising the full embedded surface:
// open (HNSW) → create domain → ingest → search → browse → context → delete → persist.
var dbDir = Path.Combine(Path.GetTempPath(), "nucleus-ffi-smoke");
Directory.CreateDirectory(dbDir);
var dbPath = Path.Combine(dbDir, "smoke.redb");
var modelCache = Path.Combine(dbDir, "models");
var indexDir = Path.Combine(dbDir, "indexes");
if (File.Exists(dbPath)) File.Delete(dbPath);        // clean slate
if (Directory.Exists(indexDir)) Directory.Delete(indexDir, true);

Console.WriteLine($"Opening DB at {dbPath} (index=hnsw, dir={indexDir})");
using var engine = NucleusEngine.Open(dbPath, modelCache: modelCache, indexDir: indexDir, indexKind: "hnsw");

var domain = engine.CreateDomain("legal");
ulong domainId = domain.RootElement.GetProperty("id").GetUInt64();
Console.WriteLine($"Domain created: id={domainId}, dim={domain.RootElement.GetProperty("dim").GetInt32()}");

Console.WriteLine("Ingesting (first run downloads the embedding model, ~450MB)...");
var docs = new (string Title, string Text, string Label)[]
{
    ("Arrendamiento", "El arrendador podrá rescindir el contrato por impago de dos mensualidades consecutivas de renta.", "contratos"),
    ("Laboral", "El trabajador tiene derecho a vacaciones anuales retribuidas de treinta días naturales.", "laboral"),
    ("Compraventa", "La cláusula de rescisión permite resolver la compraventa si no se entrega la cosa en el plazo pactado.", "contratos"),
};
var docIds = new List<ulong>();
foreach (var d in docs)
{
    var r = engine.IngestText(domainId, d.Title, d.Text, labels: [d.Label]);
    docIds.Add(r.RootElement.GetProperty("document_id").GetUInt64());
    Console.WriteLine($"  ingested '{d.Title}': doc={docIds[^1]}, {r.RootElement.GetProperty("chunk_count").GetInt32()} chunk(s)");
}

void RunSearch(string label, string query, string[]? labels = null)
{
    Console.WriteLine($"\n[{label}] query: \"{query}\"" + (labels is null ? "" : $"  labels={string.Join(",", labels)}"));
    var res = engine.Search(domainId, query, k: 3, labels: labels);
    int i = 0;
    foreach (var hit in res.RootElement.GetProperty("hits").EnumerateArray())
        Console.WriteLine($"  #{++i} score={hit.GetProperty("score").GetSingle():F4}  {hit.GetProperty("chunk").GetProperty("text").GetString()}");
    if (i == 0) Console.WriteLine("  (no hits)");
}

// --- HNSW retrieval --------------------------------------------------------
RunSearch("hnsw-semantic", "cómo terminar un contrato antes de tiempo");
RunSearch("hnsw-filtered", "cómo terminar un contrato antes de tiempo", labels: ["contratos"]);

// --- browse ----------------------------------------------------------------
Console.WriteLine($"\n[list_domains] {engine.ListDomains().RootElement.GetProperty("domains").GetArrayLength()} domain(s)");
Console.WriteLine($"[list_documents] {engine.ListDocuments(domainId).RootElement.GetProperty("documents").GetArrayLength()} doc(s)");
var tags = engine.ListTags(domainId).RootElement.GetProperty("tags");
Console.WriteLine($"[list_tags] {string.Join(", ", tags.EnumerateArray().Select(t => t.GetProperty("name").GetString()))}");

// --- chunk context (find a chunk via search, then expand neighbours) --------
var first = engine.Search(domainId, "rescisión del contrato", k: 1).RootElement.GetProperty("hits")[0];
ulong chunkId = first.GetProperty("chunk").GetProperty("id").GetUInt64();
var ctx = engine.ChunkContext(chunkId, before: 1, after: 1).RootElement.GetProperty("chunks");
Console.WriteLine($"\n[chunk_context] chunk {chunkId} → {ctx.GetArrayLength()} chunk(s) in window");

// --- delete + verify -------------------------------------------------------
engine.DeleteDocument(docIds[1]); // remove the "Laboral" doc
int remaining = engine.ListDocuments(domainId).RootElement.GetProperty("documents").GetArrayLength();
Console.WriteLine($"[delete_document] removed doc {docIds[1]}; remaining = {remaining}");

// --- persist HNSW dumps ----------------------------------------------------
var persisted = engine.PersistIndexes().RootElement.GetProperty("persisted").GetInt32();
Console.WriteLine($"[persist_indexes] persisted {persisted} index(es)");
Console.WriteLine($"[index files] {(Directory.Exists(indexDir) ? string.Join(", ", Directory.GetFiles(indexDir).Select(Path.GetFileName)) : "(none)")}");

Console.WriteLine("\nSMOKE TEST OK");
