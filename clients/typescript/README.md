# nucleus-client (JavaScript / TypeScript)

Typed client for the [Nucleus](../../README.md) RAG database HTTP API. Runs in
Node 18+ and modern browsers (uses the global `fetch`; injectable for older
runtimes). Ships ESM + `.d.ts` types.

## Install

```bash
# from the published package
npm install nucleus-client

# or from source
cd clients/typescript && npm install && npm run build
```

## Usage

```ts
import { NucleusClient, Scopes } from "nucleus-client";

const nucleus = new NucleusClient({
  baseUrl: "http://127.0.0.1:8080",
  token: "nuc_your_token",
});

// Admin: create a domain.
const domain = await nucleus.createDomain("fiscal");

// Upload a raw file (PDF/DOCX/XLSX/HTML/MD/TXT) — extracted in-engine.
import { readFileSync } from "node:fs";
const bytes = readFileSync("IRPF_2026.pdf");
await nucleus.uploadFile(domain.id, "IRPF_2026.pdf", bytes, {
  subdomain: "irpf",
  labels: ["2026", "irpf"],
});

// …or ingest text/chunks directly.
const { job_id } = await nucleus.ingestDocument(domain.id, {
  title: "nota",
  text: "tipos de retención de IRPF para 2026…",
  subdomain: "irpf",
  labels: ["2026"],
});

// Ingestion is async — poll the job.
let job = await nucleus.getJob(job_id);
while (job.status !== "Done" && job.status !== "Failed") {
  await new Promise((r) => setTimeout(r, 500));
  job = await nucleus.getJob(job_id);
}

// Search (hybrid retrieval; optional rerank server-side).
const hits = await nucleus.search(domain.id, {
  query: "tipos de retención de IRPF en 2026",
  k: 5,
  subdomain: "irpf",
});
for (const h of hits) console.log(h.score.toFixed(3), h.text.slice(0, 100));

// Create a scoped token (admin).
const tok = await nucleus.createToken("app-lectura", [Scopes.forDomain(domain.id, "Read")]);
console.log(tok.token); // shown once
```

Errors throw `NucleusError` (`.status` + message). In the browser, enable CORS
on the server with `NUCLEUS_CORS_ANY=true` (or a proxy).
