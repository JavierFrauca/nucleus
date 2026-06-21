// Demo headless del SDK de Nucleus en Node.
//
// Flujo de principio a fin: inicializar → crear dominio → ingestar → esperar el
// job → buscar. Cada bloque es una "pantalla"/operación.
//
//   npm install            # resuelve el SDK local (file:)
//   $env:NUCLEUS_TOKEN="nuc_…"
//   node demo.mjs

import { NucleusClient } from "nucleus-client"; // <- referencia al SDK (ver package.json)

const baseUrl = process.env.NUCLEUS_URL ?? "http://127.0.0.1:8080";
const token = process.env.NUCLEUS_TOKEN ?? process.argv[2];
if (!token) {
  console.error("Falta el token: define NUCLEUS_TOKEN o pásalo como argumento.");
  process.exit(1);
}

// --- inicialización ---
const nucleus = new NucleusClient({ baseUrl, token });
console.log(`Conectado a ${baseUrl} — ¿listo? ${await nucleus.isReady()}`);

// --- crear dominio (admin) ---
const domain = await nucleus.createDomain("demo-js");
console.log(`dominio ${domain.id} (${domain.model}, dim ${domain.dim})`);

// --- ingestar un texto ---
const { document_id, job_id } = await nucleus.ingestDocument(domain.id, {
  title: "nota",
  text: "Los tipos de retención de IRPF para 2026 se publican en el cuadro oficial.",
  labels: ["demo", "2026"],
});
console.log(`documento ${document_id}, job ${job_id}`);

// --- esperar a que el job termine (la ingesta es asíncrona) ---
process.stdout.write("esperando ingesta");
for (let i = 0; i < 100; i++) {
  const job = await nucleus.getJob(job_id);
  if (job.status === "Done") { console.log(" ✓"); break; }
  if (job.status === "Failed") { console.log(` ✗ ${job.error}`); process.exit(1); }
  process.stdout.write(".");
  await new Promise((r) => setTimeout(r, 300));
}

// --- buscar ---
const hits = await nucleus.search(domain.id, { query: "retención IRPF 2026", k: 5 });
console.log(`\n${hits.length} resultado(s):`);
for (const h of hits) {
  console.log(`  • ${h.score.toFixed(3)}  ${h.text.slice(0, 90)}`);
}
