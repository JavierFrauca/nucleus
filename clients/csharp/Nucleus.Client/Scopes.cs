using System;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Nucleus.Client
{
    /// <summary>Permission level, ordered Read &lt; Write &lt; Admin.</summary>
    [JsonConverter(typeof(JsonStringEnumConverter))]
    public enum Perm
    {
        Read,
        Write,
        Admin
    }

    /// <summary>
    /// Which domain(s) a scope applies to: all domains, or a single one.
    /// Serializes as the string <c>"All"</c> or the object <c>{ "One": id }</c>.
    /// </summary>
    [JsonConverter(typeof(DomainScopeConverter))]
    public sealed class DomainScope
    {
        /// <summary>Domain id when this scope targets a single domain; otherwise null.</summary>
        public long? OneId { get; }

        /// <summary>True when this scope applies to all domains.</summary>
        public bool IsAll => OneId is null;

        private DomainScope(long? oneId) => OneId = oneId;

        /// <summary>A scope covering every domain.</summary>
        public static readonly DomainScope All = new DomainScope(null);

        /// <summary>A scope covering a single domain.</summary>
        public static DomainScope One(long domainId) => new DomainScope(domainId);

        public override string ToString() => IsAll ? "All" : $"One({OneId})";
    }

    /// <summary>A single grant: a permission level over one or all domains.</summary>
    public sealed class Scope
    {
        [JsonPropertyName("domain")]
        public DomainScope Domain { get; set; } = DomainScope.All;

        [JsonPropertyName("perm")]
        public Perm Perm { get; set; } = Perm.Read;

        public Scope() { }

        public Scope(DomainScope domain, Perm perm)
        {
            Domain = domain;
            Perm = perm;
        }

        /// <summary>A global administrator grant (all domains, Admin).</summary>
        public static Scope AdminAll() => new Scope(DomainScope.All, Perm.Admin);

        /// <summary>Read/Write/Admin over a single domain.</summary>
        public static Scope ForDomain(long domainId, Perm perm) => new Scope(DomainScope.One(domainId), perm);
    }

    internal sealed class DomainScopeConverter : JsonConverter<DomainScope>
    {
        public override DomainScope Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
        {
            if (reader.TokenType == JsonTokenType.String)
            {
                var s = reader.GetString();
                if (s == "All") return DomainScope.All;
                throw new JsonException($"Unknown DomainScope string: {s}");
            }
            if (reader.TokenType == JsonTokenType.StartObject)
            {
                long? one = null;
                while (reader.Read())
                {
                    if (reader.TokenType == JsonTokenType.EndObject) break;
                    if (reader.TokenType != JsonTokenType.PropertyName) continue;
                    var prop = reader.GetString();
                    reader.Read();
                    if (prop == "One") one = reader.GetInt64();
                    else reader.Skip();
                }
                if (one is long v) return DomainScope.One(v);
                throw new JsonException("DomainScope object missing 'One'");
            }
            throw new JsonException("Invalid DomainScope token");
        }

        public override void Write(Utf8JsonWriter writer, DomainScope value, JsonSerializerOptions options)
        {
            if (value.IsAll)
            {
                writer.WriteStringValue("All");
            }
            else
            {
                writer.WriteStartObject();
                writer.WriteNumber("One", value.OneId!.Value);
                writer.WriteEndObject();
            }
        }
    }
}
