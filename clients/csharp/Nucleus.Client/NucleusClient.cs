using System;
using System.Collections.Generic;
using System.Net.Http;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Threading;
using System.Threading.Tasks;

namespace Nucleus.Client
{
    /// <summary>Thrown when the Nucleus API returns a non-success status.</summary>
    public sealed class NucleusApiException : Exception
    {
        public int StatusCode { get; }

        public NucleusApiException(int statusCode, string message) : base(message)
        {
            StatusCode = statusCode;
        }
    }

    /// <summary>
    /// Typed client for the Nucleus HTTP API. Cheap to construct; reuse a single
    /// instance (it holds an <see cref="HttpClient"/>). Thread-safe.
    /// </summary>
    public sealed class NucleusClient : IDisposable
    {
        private readonly HttpClient _http;
        private readonly bool _ownsHttp;
        private readonly string _baseUrl;
        private readonly string _token;

        private static readonly JsonSerializerOptions Json = new JsonSerializerOptions
        {
            DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
            PropertyNameCaseInsensitive = true
        };

        /// <param name="baseUrl">Server base URL, e.g. http://127.0.0.1:8080</param>
        /// <param name="token">API token (the <c>nuc_…</c> string).</param>
        /// <param name="http">Optional HttpClient to reuse; one is created if null.</param>
        public NucleusClient(string baseUrl, string token, HttpClient? http = null)
        {
            if (string.IsNullOrWhiteSpace(baseUrl)) throw new ArgumentException("baseUrl required", nameof(baseUrl));
            _baseUrl = baseUrl.TrimEnd('/');
            _token = token ?? "";
            _ownsHttp = http is null;
            _http = http ?? new HttpClient();
        }

        // --- domains -------------------------------------------------------

        public Task<Domain> CreateDomainAsync(string name, string? model = null, CancellationToken ct = default)
            => SendJsonAsync<Domain>(HttpMethod.Post, "/v1/domains", new CreateDomainBody { Name = name, Model = model }, ct);

        public Task<List<Domain>> ListDomainsAsync(CancellationToken ct = default)
            => SendJsonAsync<List<Domain>>(HttpMethod.Get, "/v1/domains", null, ct);

        public Task<Domain> GetDomainAsync(long id, CancellationToken ct = default)
            => SendJsonAsync<Domain>(HttpMethod.Get, $"/v1/domains/{id}", null, ct);

        // --- documents & ingest -------------------------------------------

        public Task<IngestResponse> IngestDocumentAsync(long domainId, IngestRequest req, CancellationToken ct = default)
            => SendJsonAsync<IngestResponse>(HttpMethod.Post, $"/v1/domains/{domainId}/documents", req, ct);

        public Task<List<Document>> ListDocumentsAsync(long domainId, int offset = 0, int limit = 50, CancellationToken ct = default)
            => SendJsonAsync<List<Document>>(HttpMethod.Get, $"/v1/domains/{domainId}/documents?offset={offset}&limit={limit}", null, ct);

        public Task<Document> GetDocumentAsync(long id, CancellationToken ct = default)
            => SendJsonAsync<Document>(HttpMethod.Get, $"/v1/documents/{id}", null, ct);

        public Task DeleteDocumentAsync(long id, CancellationToken ct = default)
            => SendNoContentAsync(HttpMethod.Delete, $"/v1/documents/{id}", ct);

        /// <summary>Upload raw file bytes; Nucleus extracts the text in-engine.</summary>
        public async Task<UploadResponse> UploadFileAsync(
            long domainId,
            string filename,
            byte[] content,
            string? title = null,
            string? subdomain = null,
            IEnumerable<string>? labels = null,
            IEnumerable<long>? tags = null,
            CancellationToken ct = default)
        {
            var query = new StringBuilder();
            query.Append("?filename=").Append(Uri.EscapeDataString(filename));
            if (title != null) query.Append("&title=").Append(Uri.EscapeDataString(title));
            if (subdomain != null) query.Append("&subdomain=").Append(Uri.EscapeDataString(subdomain));
            if (labels != null) query.Append("&labels=").Append(Uri.EscapeDataString(string.Join(",", labels)));
            if (tags != null) query.Append("&tags=").Append(Uri.EscapeDataString(string.Join(",", tags)));

            using var req = new HttpRequestMessage(HttpMethod.Post, _baseUrl + $"/v1/domains/{domainId}/files" + query);
            req.Headers.TryAddWithoutValidation("Authorization", "Bearer " + _token);
            req.Content = new ByteArrayContent(content);
            req.Content.Headers.TryAddWithoutValidation("Content-Type", "application/octet-stream");
            return await SendAndParseAsync<UploadResponse>(req, ct).ConfigureAwait(false);
        }

        // --- search --------------------------------------------------------

        public Task<List<Hit>> SearchAsync(long domainId, SearchRequest req, CancellationToken ct = default)
            => SendJsonAsync<List<Hit>>(HttpMethod.Post, $"/v1/domains/{domainId}/search", req, ct);

        // --- tags & subdomains --------------------------------------------

        public Task<Tag> CreateTagAsync(long domainId, string name, string? displayName = null, string description = "", long? parent = null, CancellationToken ct = default)
            => SendJsonAsync<Tag>(HttpMethod.Post, $"/v1/domains/{domainId}/tags",
                new CreateTagBody { Name = name, DisplayName = displayName, Description = description, Parent = parent }, ct);

        public Task<List<Tag>> ListTagsAsync(long domainId, CancellationToken ct = default)
            => SendJsonAsync<List<Tag>>(HttpMethod.Get, $"/v1/domains/{domainId}/tags", null, ct);

        public Task<Subdomain> CreateSubdomainAsync(long domainId, string name, string description = "", CancellationToken ct = default)
            => SendJsonAsync<Subdomain>(HttpMethod.Post, $"/v1/domains/{domainId}/subdomains",
                new CreateSubdomainBody { Name = name, Description = description }, ct);

        public Task<List<Subdomain>> ListSubdomainsAsync(long domainId, CancellationToken ct = default)
            => SendJsonAsync<List<Subdomain>>(HttpMethod.Get, $"/v1/domains/{domainId}/subdomains", null, ct);

        // --- chunks --------------------------------------------------------

        public Task<Chunk> GetChunkAsync(long id, CancellationToken ct = default)
            => SendJsonAsync<Chunk>(HttpMethod.Get, $"/v1/chunks/{id}", null, ct);

        public Task<List<Chunk>> GetChunkContextAsync(long id, int before = 1, int after = 1, CancellationToken ct = default)
            => SendJsonAsync<List<Chunk>>(HttpMethod.Get, $"/v1/chunks/{id}/context?before={before}&after={after}", null, ct);

        // --- jobs ----------------------------------------------------------

        public Task<List<Job>> ListJobsAsync(int offset = 0, int limit = 50, CancellationToken ct = default)
            => SendJsonAsync<List<Job>>(HttpMethod.Get, $"/v1/jobs?offset={offset}&limit={limit}", null, ct);

        public Task<Job> GetJobAsync(long id, CancellationToken ct = default)
            => SendJsonAsync<Job>(HttpMethod.Get, $"/v1/jobs/{id}", null, ct);

        // --- tokens --------------------------------------------------------

        public Task<CreateTokenResponse> CreateTokenAsync(string name, IEnumerable<Scope> scopes, long? expiresAt = null, CancellationToken ct = default)
            => SendJsonAsync<CreateTokenResponse>(HttpMethod.Post, "/v1/tokens",
                new CreateTokenBody { Name = name, Scopes = new List<Scope>(scopes), ExpiresAt = expiresAt }, ct);

        public Task<List<TokenInfo>> ListTokensAsync(CancellationToken ct = default)
            => SendJsonAsync<List<TokenInfo>>(HttpMethod.Get, "/v1/tokens", null, ct);

        public Task DeleteTokenAsync(long id, CancellationToken ct = default)
            => SendNoContentAsync(HttpMethod.Delete, $"/v1/tokens/{id}", ct);

        // --- backups -------------------------------------------------------

        /// <summary>Take a backup now. <paramref name="kind"/> is "full" or "differential".</summary>
        public Task<BackupRecord> CreateBackupAsync(string kind = "full", CancellationToken ct = default)
            => SendJsonAsync<BackupRecord>(HttpMethod.Post, "/v1/backups", new BackupBody { Kind = kind }, ct);

        public Task<List<BackupRecord>> ListBackupsAsync(CancellationToken ct = default)
            => SendJsonAsync<List<BackupRecord>>(HttpMethod.Get, "/v1/backups", null, ct);

        /// <summary>Restore a backup, hot-swapping the engine.</summary>
        public Task<RestoreResponse> RestoreBackupAsync(string id, CancellationToken ct = default)
            => SendJsonAsync<RestoreResponse>(HttpMethod.Post, "/v1/backups/restore", new RestoreBody { Id = id }, ct);

        public Task<ScheduleConfig> GetScheduleAsync(CancellationToken ct = default)
            => SendJsonAsync<ScheduleConfig>(HttpMethod.Get, "/v1/backups/schedule", null, ct);

        public Task<ScheduleConfig> SetScheduleAsync(ScheduleConfig schedule, CancellationToken ct = default)
            => SendJsonAsync<ScheduleConfig>(HttpMethod.Post, "/v1/backups/schedule", schedule, ct);

        // --- maintenance & health -----------------------------------------

        public Task<PersistResponse> PersistIndexesAsync(CancellationToken ct = default)
            => SendJsonAsync<PersistResponse>(HttpMethod.Post, "/v1/maintenance/persist", null, ct);

        /// <summary>Returns true if the server is ready (storage reachable).</summary>
        public async Task<bool> IsReadyAsync(CancellationToken ct = default)
        {
            using var req = new HttpRequestMessage(HttpMethod.Get, _baseUrl + "/readyz");
            using var resp = await _http.SendAsync(req, ct).ConfigureAwait(false);
            return resp.IsSuccessStatusCode;
        }

        // --- plumbing ------------------------------------------------------

        private async Task<T> SendJsonAsync<T>(HttpMethod method, string path, object? body, CancellationToken ct)
        {
            using var req = new HttpRequestMessage(method, _baseUrl + path);
            req.Headers.TryAddWithoutValidation("Authorization", "Bearer " + _token);
            if (body != null)
            {
                var json = JsonSerializer.Serialize(body, body.GetType(), Json);
                req.Content = new StringContent(json, Encoding.UTF8, "application/json");
            }
            return await SendAndParseAsync<T>(req, ct).ConfigureAwait(false);
        }

        private async Task SendNoContentAsync(HttpMethod method, string path, CancellationToken ct)
        {
            using var req = new HttpRequestMessage(method, _baseUrl + path);
            req.Headers.TryAddWithoutValidation("Authorization", "Bearer " + _token);
            using var resp = await _http.SendAsync(req, ct).ConfigureAwait(false);
            await EnsureSuccessAsync(resp).ConfigureAwait(false);
        }

        private async Task<T> SendAndParseAsync<T>(HttpRequestMessage req, CancellationToken ct)
        {
            using var resp = await _http.SendAsync(req, ct).ConfigureAwait(false);
            var text = await resp.Content.ReadAsStringAsync().ConfigureAwait(false);
            if (!resp.IsSuccessStatusCode)
                throw new NucleusApiException((int)resp.StatusCode, ExtractError(text, resp.ReasonPhrase));
            if (string.IsNullOrEmpty(text))
                return default!;
            return JsonSerializer.Deserialize<T>(text, Json)!;
        }

        private static async Task EnsureSuccessAsync(HttpResponseMessage resp)
        {
            if (resp.IsSuccessStatusCode) return;
            var text = await resp.Content.ReadAsStringAsync().ConfigureAwait(false);
            throw new NucleusApiException((int)resp.StatusCode, ExtractError(text, resp.ReasonPhrase));
        }

        private static string ExtractError(string body, string? fallback)
        {
            if (!string.IsNullOrEmpty(body))
            {
                try
                {
                    using var doc = JsonDocument.Parse(body);
                    if (doc.RootElement.TryGetProperty("error", out var e) && e.ValueKind == JsonValueKind.String)
                        return e.GetString() ?? body;
                }
                catch (JsonException) { /* not JSON; fall through */ }
                return body;
            }
            return fallback ?? "request failed";
        }

        public void Dispose()
        {
            if (_ownsHttp) _http.Dispose();
        }

        // --- internal request bodies --------------------------------------

        private sealed class CreateDomainBody
        {
            [JsonPropertyName("name")] public string Name { get; set; } = "";
            [JsonPropertyName("model")] public string? Model { get; set; }
        }

        private sealed class CreateTagBody
        {
            [JsonPropertyName("name")] public string Name { get; set; } = "";
            [JsonPropertyName("display_name")] public string? DisplayName { get; set; }
            [JsonPropertyName("description")] public string Description { get; set; } = "";
            [JsonPropertyName("parent")] public long? Parent { get; set; }
        }

        private sealed class CreateSubdomainBody
        {
            [JsonPropertyName("name")] public string Name { get; set; } = "";
            [JsonPropertyName("description")] public string Description { get; set; } = "";
        }

        private sealed class CreateTokenBody
        {
            [JsonPropertyName("name")] public string Name { get; set; } = "";
            [JsonPropertyName("scopes")] public List<Scope> Scopes { get; set; } = new List<Scope>();
            [JsonPropertyName("expires_at")] public long? ExpiresAt { get; set; }
        }

        private sealed class BackupBody
        {
            [JsonPropertyName("kind")] public string Kind { get; set; } = "full";
        }

        private sealed class RestoreBody
        {
            [JsonPropertyName("id")] public string Id { get; set; } = "";
        }
    }

    public sealed class PersistResponse
    {
        [JsonPropertyName("persisted")] public int Persisted { get; set; }
    }

    public sealed class RestoreResponse
    {
        [JsonPropertyName("restored")] public string Restored { get; set; } = "";
        [JsonPropertyName("active_db")] public string ActiveDb { get; set; } = "";
    }
}
