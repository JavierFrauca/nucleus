using Nucleus.Native;

// --- default path probe: open with NO db_path -> per-user %LOCALAPPDATA%\Nucleus.
{
    using var def = NucleusEngine.Open(dbPath: ""); // empty -> engine picks the default
    var expected = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "Nucleus", "nucleus.redb");
    Console.WriteLine($"[default-path] opened with no db_path; expected at {expected}: exists={File.Exists(expected)}");
}

// End-to-end smoke test for nucleus.dll exercising the full embedded surface via
// the strongly-typed binding: open (HNSW) → create → ingest → search → browse →
// context → delete → persist → edit/reindex/multi-domain.
var dbDir = Path.Combine(Path.GetTempPath(), "nucleus-ffi-smoke");
Directory.CreateDirectory(dbDir);
var dbPath = Path.Combine(dbDir, "smoke.redb");
var modelCache = Path.Combine(dbDir, "models");
var indexDir = Path.Combine(dbDir, "indexes");
if (File.Exists(dbPath)) File.Delete(dbPath);        // clean slate
if (Directory.Exists(indexDir)) Directory.Delete(indexDir, true);

Console.WriteLine($"Opening DB at {dbPath} (index=hnsw, dir={indexDir})");
using var engine = NucleusEngine.Open(dbPath, modelCache: modelCache, indexDir: indexDir, indexKind: "hnsw");

Domain domain = engine.CreateDomain("legal");
Console.WriteLine($"Domain created: id={domain.Id}, dim={domain.Dim}");

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
    IngestResult r = engine.IngestText(domain.Id, d.Title, d.Text, labels: [d.Label]);
    docIds.Add(r.DocumentId);
    Console.WriteLine($"  ingested '{d.Title}': doc={r.DocumentId}, {r.ChunkCount} chunk(s)");
}

void RunSearch(string label, string query, string[]? labels = null, float diversity = 0f)
{
    Console.WriteLine($"\n[{label}] query: \"{query}\"" + (labels is null ? "" : $"  labels={string.Join(",", labels)}"));
    foreach (var (hit, i) in engine.Search(domain.Id, query, k: 3, labels: labels, diversity: diversity).Select((h, i) => (h, i + 1)))
        Console.WriteLine($"  #{i} score={hit.Score:F4}  {hit.Chunk.Text}");
}

// --- HNSW retrieval --------------------------------------------------------
RunSearch("hnsw-semantic", "cómo terminar un contrato antes de tiempo");
RunSearch("hnsw-filtered", "cómo terminar un contrato antes de tiempo", labels: ["contratos"]);

// --- browse ----------------------------------------------------------------
Console.WriteLine($"\n[list_domains] {engine.ListDomains().Count} domain(s)");
Console.WriteLine($"[list_documents] {engine.ListDocuments(domain.Id).Count} doc(s)");
Console.WriteLine($"[list_tags] {string.Join(", ", engine.ListTags(domain.Id).Select(t => t.Name))}");

// --- chunk context (find a chunk via search, then expand neighbours) --------
ulong chunkId = engine.Search(domain.Id, "rescisión del contrato", k: 1)[0].Chunk.Id;
Console.WriteLine($"\n[chunk_context] chunk {chunkId} → {engine.ChunkContext(chunkId, before: 1, after: 1).Count} chunk(s) in window");

// --- delete + verify -------------------------------------------------------
engine.DeleteDocument(docIds[1]); // remove the "Laboral" doc
Console.WriteLine($"[delete_document] removed doc {docIds[1]}; remaining = {engine.ListDocuments(domain.Id).Count}");

// --- persist HNSW dumps ----------------------------------------------------
Console.WriteLine($"[persist_indexes] persisted {engine.PersistIndexes()} index(es)");
Console.WriteLine($"[index files] {(Directory.Exists(indexDir) ? string.Join(", ", Directory.GetFiles(indexDir).Select(Path.GetFileName)) : "(none)")}");

// --- new capabilities from the core upgrade --------------------------------
Console.WriteLine($"\n[rename_domain] -> {engine.RenameDomain(domain.Id, "legal-es").Name}");

Document updated = engine.UpdateDocument(docIds[0], labels: ["contratos", "arrendamiento"]);
Console.WriteLine($"[update_document] doc {updated.Id} now has {updated.Tags.Count} tag(s)");

Console.WriteLine($"[search_multi] {engine.SearchMulti([domain.Id], "rescisión", k: 2).Count} hit(s) across 1 domain");

var diverse = engine.Search(domain.Id, "contrato", k: 2, diversity: 0.7f);
Console.WriteLine($"[search diversity=0.7] {diverse.Count} hit(s); snippet: {diverse[0].Snippet ?? "(none)"}");

Console.WriteLine($"[reindex_domain] re-embedded {engine.ReindexDomain(domain.Id)} chunk(s)");

// --- file ingest (multi-format extraction inside the engine) ---------------
var md = System.Text.Encoding.UTF8.GetBytes("# Política de privacidad\n\nLos datos personales se tratan conforme al RGPD.");
IngestResult fr = engine.IngestFile(domain.Id, "privacidad.md", md, labels: ["legal"]);
Console.WriteLine($"[ingest_file] privacidad.md → doc {fr.DocumentId}, {fr.ChunkCount} chunk(s), {fr.Chars} chars");

// --- dedup: re-ingesting identical content returns the existing doc ---------
IngestResult dup = engine.IngestFile(domain.Id, "privacidad.md", md, labels: ["legal"]);
Console.WriteLine($"[dedup] mismo fichero otra vez → doc {dup.DocumentId}, duplicate={dup.Duplicate}, chunks={dup.ChunkCount}");

Console.WriteLine("\nSMOKE TEST OK");
