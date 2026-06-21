/**
 * Typed client for the Nucleus RAG database HTTP API.
 *
 * Works in Node 18+ and modern browsers (uses the global `fetch`). A custom
 * `fetch` can be injected for older runtimes or testing.
 *
 * Note: ids are 64-bit on the server; JavaScript numbers are safe up to 2^53,
 * which is ample for sequence ids in practice.
 */

// --- model types -----------------------------------------------------------

export interface Domain {
  id: number;
  name: string;
  model: string;
  dim: number;
  created_at: number;
}

export interface Subdomain {
  id: number;
  domain_id: number;
  name: string;
  description: string;
  created_at: number;
}

export interface Document {
  id: number;
  domain_id: number;
  subdomain_id: number | null;
  title: string;
  source: string | null;
  metadata: Record<string, string>;
  tags: number[];
  created_at: number;
}

export interface Chunk {
  id: number;
  document_id: number;
  domain_id: number;
  subdomain_id: number | null;
  ordinal: number;
  text: string;
  tags: number[];
  metadata: Record<string, string>;
  prev: number | null;
  next: number | null;
}

export interface Tag {
  id: number;
  domain_id: number;
  name: string;
  display_name: string;
  description: string;
  parent: number | null;
  created_at: number;
}

export interface Hit {
  chunk_id: number;
  document_id: number;
  score: number;
  text: string;
  tags: number[];
  metadata: Record<string, string>;
}

export interface Job {
  id: number;
  status: "Pending" | "Running" | "Done" | "Failed" | string;
  attempts: number;
  error: string | null;
}

export type Perm = "Read" | "Write" | "Admin";

/** Either all domains (`"All"`) or a single one (`{ One: id }`). */
export type DomainScope = "All" | { One: number };

export interface Scope {
  domain: DomainScope;
  perm: Perm;
}

export interface CreateTokenResponse {
  id: number;
  name: string;
  /** Plaintext token — returned only once. */
  token: string;
}

export interface TokenInfo {
  id: number;
  name: string;
  scopes: Scope[];
  created_at: number;
  expires_at: number | null;
}

export interface BackupRecord {
  id: string;
  kind: "Full" | "Differential" | string;
  created_at: number;
  parent: string | null;
  file: string;
  bytes: number;
}

export interface ScheduleConfig {
  enabled: boolean;
  interval_secs: number;
  full_every: number;
  keep_fulls: number;
}

export interface RestoreResponse {
  restored: string;
  active_db: string;
}

// --- request bodies --------------------------------------------------------

/** Provide either `text` or `chunks`. */
export interface IngestRequest {
  title: string;
  source?: string;
  text?: string;
  chunks?: string[];
  subdomain?: string;
  labels?: string[];
  tags?: number[];
  metadata?: Record<string, string>;
}

export interface IngestResponse {
  document_id: number;
  job_id: number;
  duplicate: boolean;
}

export interface UploadResponse {
  document_id: number;
  job_id: number;
  chars: number;
  duplicate: boolean;
}

/** Provide either `query` or `query_vector`. */
export interface SearchRequest {
  query?: string;
  query_vector?: number[];
  k?: number;
  tags?: number[];
  match_all?: boolean;
  document_ids?: number[];
  subdomain?: string;
  filter?: string;
}

export interface UploadOptions {
  title?: string;
  subdomain?: string;
  labels?: string[];
  tags?: number[];
}

export type FetchLike = (input: string, init?: any) => Promise<any>;

export interface NucleusOptions {
  baseUrl: string;
  token: string;
  /** Override the global fetch (Node <18, tests, proxies). */
  fetch?: FetchLike;
}

/** Thrown when the API returns a non-success status. */
export class NucleusError extends Error {
  constructor(public readonly status: number, message: string) {
    super(message);
    this.name = "NucleusError";
  }
}

// --- client ----------------------------------------------------------------

export class NucleusClient {
  private readonly baseUrl: string;
  private readonly token: string;
  private readonly fetchImpl: FetchLike;

  constructor(opts: NucleusOptions) {
    if (!opts.baseUrl) throw new Error("baseUrl required");
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.token = opts.token ?? "";
    const f = opts.fetch ?? (globalThis as any).fetch;
    if (!f) throw new Error("no fetch available; pass opts.fetch");
    this.fetchImpl = f.bind(globalThis);
  }

  // domains
  createDomain(name: string, model?: string): Promise<Domain> {
    return this.request("POST", "/v1/domains", { name, model });
  }
  listDomains(): Promise<Domain[]> {
    return this.request("GET", "/v1/domains");
  }
  getDomain(id: number): Promise<Domain> {
    return this.request("GET", `/v1/domains/${id}`);
  }

  // documents & ingest
  ingestDocument(domainId: number, req: IngestRequest): Promise<IngestResponse> {
    return this.request("POST", `/v1/domains/${domainId}/documents`, req);
  }
  listDocuments(domainId: number, offset = 0, limit = 50): Promise<Document[]> {
    return this.request("GET", `/v1/domains/${domainId}/documents?offset=${offset}&limit=${limit}`);
  }
  getDocument(id: number): Promise<Document> {
    return this.request("GET", `/v1/documents/${id}`);
  }
  deleteDocument(id: number): Promise<void> {
    return this.request("DELETE", `/v1/documents/${id}`);
  }

  /** Upload raw file bytes; Nucleus extracts the text in-engine. */
  uploadFile(
    domainId: number,
    filename: string,
    content: Uint8Array | ArrayBuffer | Blob,
    opts: UploadOptions = {}
  ): Promise<UploadResponse> {
    const q = new URLSearchParams({ filename });
    if (opts.title) q.set("title", opts.title);
    if (opts.subdomain) q.set("subdomain", opts.subdomain);
    if (opts.labels) q.set("labels", opts.labels.join(","));
    if (opts.tags) q.set("tags", opts.tags.join(","));
    return this.request(
      "POST",
      `/v1/domains/${domainId}/files?${q.toString()}`,
      content,
      "application/octet-stream"
    );
  }

  // search
  search(domainId: number, req: SearchRequest): Promise<Hit[]> {
    return this.request("POST", `/v1/domains/${domainId}/search`, req);
  }

  // tags & subdomains
  createTag(
    domainId: number,
    name: string,
    opts: { display_name?: string; description?: string; parent?: number } = {}
  ): Promise<Tag> {
    return this.request("POST", `/v1/domains/${domainId}/tags`, { name, ...opts });
  }
  listTags(domainId: number): Promise<Tag[]> {
    return this.request("GET", `/v1/domains/${domainId}/tags`);
  }
  createSubdomain(domainId: number, name: string, description = ""): Promise<Subdomain> {
    return this.request("POST", `/v1/domains/${domainId}/subdomains`, { name, description });
  }
  listSubdomains(domainId: number): Promise<Subdomain[]> {
    return this.request("GET", `/v1/domains/${domainId}/subdomains`);
  }

  // chunks
  getChunk(id: number): Promise<Chunk> {
    return this.request("GET", `/v1/chunks/${id}`);
  }
  getChunkContext(id: number, before = 1, after = 1): Promise<Chunk[]> {
    return this.request("GET", `/v1/chunks/${id}/context?before=${before}&after=${after}`);
  }

  // jobs
  listJobs(offset = 0, limit = 50): Promise<Job[]> {
    return this.request("GET", `/v1/jobs?offset=${offset}&limit=${limit}`);
  }
  getJob(id: number): Promise<Job> {
    return this.request("GET", `/v1/jobs/${id}`);
  }

  // tokens
  createToken(name: string, scopes: Scope[], expiresAt?: number): Promise<CreateTokenResponse> {
    return this.request("POST", "/v1/tokens", { name, scopes, expires_at: expiresAt ?? null });
  }
  listTokens(): Promise<TokenInfo[]> {
    return this.request("GET", "/v1/tokens");
  }
  deleteToken(id: number): Promise<void> {
    return this.request("DELETE", `/v1/tokens/${id}`);
  }

  // backups
  createBackup(kind: "full" | "differential" = "full"): Promise<BackupRecord> {
    return this.request("POST", "/v1/backups", { kind });
  }
  listBackups(): Promise<BackupRecord[]> {
    return this.request("GET", "/v1/backups");
  }
  restoreBackup(id: string): Promise<RestoreResponse> {
    return this.request("POST", "/v1/backups/restore", { id });
  }
  getSchedule(): Promise<ScheduleConfig> {
    return this.request("GET", "/v1/backups/schedule");
  }
  setSchedule(cfg: ScheduleConfig): Promise<ScheduleConfig> {
    return this.request("POST", "/v1/backups/schedule", cfg);
  }

  // maintenance & health
  persistIndexes(): Promise<{ persisted: number }> {
    return this.request("POST", "/v1/maintenance/persist");
  }
  async isReady(): Promise<boolean> {
    const res = await this.fetchImpl(this.baseUrl + "/readyz", { method: "GET" });
    return res.ok;
  }

  // plumbing
  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    contentType = "application/json"
  ): Promise<T> {
    const headers: Record<string, string> = { authorization: `Bearer ${this.token}` };
    let payload: any = undefined;
    if (body !== undefined && body !== null) {
      if (contentType === "application/json") {
        headers["content-type"] = "application/json";
        payload = JSON.stringify(body);
      } else {
        headers["content-type"] = contentType;
        payload = body;
      }
    }
    const res = await this.fetchImpl(this.baseUrl + path, { method, headers, body: payload });
    const text = await res.text();
    if (!res.ok) {
      throw new NucleusError(res.status, extractError(text, res.statusText));
    }
    return (text ? JSON.parse(text) : undefined) as T;
  }
}

function extractError(body: string, fallback: string): string {
  if (body) {
    try {
      const j = JSON.parse(body);
      if (j && typeof j.error === "string") return j.error;
    } catch {
      /* not JSON */
    }
    return body;
  }
  return fallback || "request failed";
}

/** Helpers to build common scopes. */
export const Scopes = {
  adminAll(): Scope {
    return { domain: "All", perm: "Admin" };
  },
  forDomain(domainId: number, perm: Perm): Scope {
    return { domain: { One: domainId }, perm };
  },
};
