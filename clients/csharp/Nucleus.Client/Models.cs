using System.Collections.Generic;
using System.Text.Json.Serialization;

namespace Nucleus.Client
{
    public sealed class Domain
    {
        [JsonPropertyName("id")] public long Id { get; set; }
        [JsonPropertyName("name")] public string Name { get; set; } = "";
        [JsonPropertyName("model")] public string Model { get; set; } = "";
        [JsonPropertyName("dim")] public int Dim { get; set; }
        [JsonPropertyName("created_at")] public long CreatedAt { get; set; }
    }

    public sealed class Subdomain
    {
        [JsonPropertyName("id")] public long Id { get; set; }
        [JsonPropertyName("domain_id")] public long DomainId { get; set; }
        [JsonPropertyName("name")] public string Name { get; set; } = "";
        [JsonPropertyName("description")] public string Description { get; set; } = "";
        [JsonPropertyName("created_at")] public long CreatedAt { get; set; }
    }

    public sealed class Document
    {
        [JsonPropertyName("id")] public long Id { get; set; }
        [JsonPropertyName("domain_id")] public long DomainId { get; set; }
        [JsonPropertyName("subdomain_id")] public long? SubdomainId { get; set; }
        [JsonPropertyName("title")] public string Title { get; set; } = "";
        [JsonPropertyName("source")] public string? Source { get; set; }
        [JsonPropertyName("metadata")] public Dictionary<string, string> Metadata { get; set; } = new Dictionary<string, string>();
        [JsonPropertyName("tags")] public List<long> Tags { get; set; } = new List<long>();
        [JsonPropertyName("created_at")] public long CreatedAt { get; set; }
    }

    public sealed class Chunk
    {
        [JsonPropertyName("id")] public long Id { get; set; }
        [JsonPropertyName("document_id")] public long DocumentId { get; set; }
        [JsonPropertyName("domain_id")] public long DomainId { get; set; }
        [JsonPropertyName("subdomain_id")] public long? SubdomainId { get; set; }
        [JsonPropertyName("ordinal")] public int Ordinal { get; set; }
        [JsonPropertyName("text")] public string Text { get; set; } = "";
        [JsonPropertyName("tags")] public List<long> Tags { get; set; } = new List<long>();
        [JsonPropertyName("metadata")] public Dictionary<string, string> Metadata { get; set; } = new Dictionary<string, string>();
        [JsonPropertyName("prev")] public long? Prev { get; set; }
        [JsonPropertyName("next")] public long? Next { get; set; }
    }

    public sealed class Tag
    {
        [JsonPropertyName("id")] public long Id { get; set; }
        [JsonPropertyName("domain_id")] public long DomainId { get; set; }
        [JsonPropertyName("name")] public string Name { get; set; } = "";
        [JsonPropertyName("display_name")] public string DisplayName { get; set; } = "";
        [JsonPropertyName("description")] public string Description { get; set; } = "";
        [JsonPropertyName("parent")] public long? Parent { get; set; }
        [JsonPropertyName("created_at")] public long CreatedAt { get; set; }
    }

    /// <summary>Body for <c>IngestDocumentAsync</c>. Provide <see cref="Text"/> or <see cref="Chunks"/>.</summary>
    public sealed class IngestRequest
    {
        [JsonPropertyName("title")] public string Title { get; set; } = "";
        [JsonPropertyName("source")] public string? Source { get; set; }
        [JsonPropertyName("text")] public string? Text { get; set; }
        [JsonPropertyName("chunks")] public List<string>? Chunks { get; set; }
        [JsonPropertyName("subdomain")] public string? Subdomain { get; set; }
        [JsonPropertyName("labels")] public List<string> Labels { get; set; } = new List<string>();
        [JsonPropertyName("tags")] public List<long> Tags { get; set; } = new List<long>();
        [JsonPropertyName("metadata")] public Dictionary<string, string> Metadata { get; set; } = new Dictionary<string, string>();
    }

    public sealed class IngestResponse
    {
        [JsonPropertyName("document_id")] public long DocumentId { get; set; }
        [JsonPropertyName("job_id")] public long JobId { get; set; }
        [JsonPropertyName("duplicate")] public bool Duplicate { get; set; }
    }

    public sealed class UploadResponse
    {
        [JsonPropertyName("document_id")] public long DocumentId { get; set; }
        [JsonPropertyName("job_id")] public long JobId { get; set; }
        [JsonPropertyName("chars")] public int Chars { get; set; }
        [JsonPropertyName("duplicate")] public bool Duplicate { get; set; }
    }

    /// <summary>Body for <c>SearchAsync</c>. Provide <see cref="Query"/> or <see cref="QueryVector"/>.</summary>
    public sealed class SearchRequest
    {
        [JsonPropertyName("query")] public string? Query { get; set; }
        [JsonPropertyName("query_vector")] public List<float>? QueryVector { get; set; }
        [JsonPropertyName("k")] public int K { get; set; } = 10;
        [JsonPropertyName("tags")] public List<long> Tags { get; set; } = new List<long>();
        [JsonPropertyName("match_all")] public bool MatchAll { get; set; }
        [JsonPropertyName("document_ids")] public List<long> DocumentIds { get; set; } = new List<long>();
        [JsonPropertyName("subdomain")] public string? Subdomain { get; set; }
        [JsonPropertyName("filter")] public string? Filter { get; set; }
    }

    public sealed class Hit
    {
        [JsonPropertyName("chunk_id")] public long ChunkId { get; set; }
        [JsonPropertyName("document_id")] public long DocumentId { get; set; }
        [JsonPropertyName("score")] public float Score { get; set; }
        [JsonPropertyName("text")] public string Text { get; set; } = "";
        [JsonPropertyName("tags")] public List<long> Tags { get; set; } = new List<long>();
        [JsonPropertyName("metadata")] public Dictionary<string, string> Metadata { get; set; } = new Dictionary<string, string>();
    }

    public sealed class Job
    {
        [JsonPropertyName("id")] public long Id { get; set; }
        [JsonPropertyName("status")] public string Status { get; set; } = "";
        [JsonPropertyName("attempts")] public int Attempts { get; set; }
        [JsonPropertyName("error")] public string? Error { get; set; }
    }

    public sealed class CreateTokenResponse
    {
        [JsonPropertyName("id")] public long Id { get; set; }
        [JsonPropertyName("name")] public string Name { get; set; } = "";
        /// <summary>Plaintext token — returned only once.</summary>
        [JsonPropertyName("token")] public string Token { get; set; } = "";
    }

    public sealed class TokenInfo
    {
        [JsonPropertyName("id")] public long Id { get; set; }
        [JsonPropertyName("name")] public string Name { get; set; } = "";
        [JsonPropertyName("scopes")] public List<Scope> Scopes { get; set; } = new List<Scope>();
        [JsonPropertyName("created_at")] public long CreatedAt { get; set; }
        [JsonPropertyName("expires_at")] public long? ExpiresAt { get; set; }
    }

    public sealed class BackupRecord
    {
        [JsonPropertyName("id")] public string Id { get; set; } = "";
        /// <summary>"Full" or "Differential".</summary>
        [JsonPropertyName("kind")] public string Kind { get; set; } = "";
        [JsonPropertyName("created_at")] public long CreatedAt { get; set; }
        [JsonPropertyName("parent")] public string? Parent { get; set; }
        [JsonPropertyName("file")] public string File { get; set; } = "";
        [JsonPropertyName("bytes")] public long Bytes { get; set; }
    }

    public sealed class ScheduleConfig
    {
        [JsonPropertyName("enabled")] public bool Enabled { get; set; }
        [JsonPropertyName("interval_secs")] public long IntervalSecs { get; set; }
        [JsonPropertyName("full_every")] public int FullEvery { get; set; }
        [JsonPropertyName("keep_fulls")] public int KeepFulls { get; set; }
    }
}
