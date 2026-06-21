// Subida de un fichero CRUDO (PDF/DOCX/XLSX/HTML/MD/TXT) con el SDK de Nucleus.
// Se mandan los BYTES; Nucleus extrae el texto dentro del motor.
//
//   npm install
//   $env:NUCLEUS_TOKEN="nuc_…"
//   node upload.mjs ruta\al\documento.pdf

import { NucleusClient } from "nucleus-client";
import { readFileSync } from "node:fs";
import { basename } from "node:path";

const baseUrl = process.env.NUCLEUS_URL ?? "http://127.0.0.1:8080";
const token = process.env.NUCLEUS_TOKEN;
const file = process.argv[2];
if (!token || !file) {
  console.error("Uso: NUCLEUS_TOKEN=… node upload.mjs <ruta-fichero>");
  process.exit(1);
}

const nucleus = new NucleusClient({ baseUrl, token });

// get-or-create del dominio por nombre (los dominios no son únicos por nombre).
async function domainId(name) {
  const existing = (await nucleus.listDomains()).find((d) => d.name === name);
  return (existing ?? (await nucleus.createDomain(name))).id;
}

const id = await domainId("demo-js");
const bytes = readFileSync(file); // Buffer (es un Uint8Array)

// uploadFile(domainId, filename, bytes, opts). La extensión del filename elige el extractor.
const up = await nucleus.uploadFile(id, basename(file), bytes, {
  subdomain: "docs",
  labels: ["demo"],
});
console.log(`documento ${up.document_id}, job ${up.job_id}, ${up.chars} chars extraídos (dup: ${up.duplicate})`);

// La ingesta es asíncrona: esperamos el job.
process.stdout.write("procesando");
for (let i = 0; i < 600; i++) {
  const job = await nucleus.getJob(up.job_id);
  if (job.status === "Done") { console.log(" ✓ indexado"); break; }
  if (job.status === "Failed") { console.log(` ✗ ${job.error}`); process.exit(1); }
  process.stdout.write(".");
  await new Promise((r) => setTimeout(r, 500));
}
