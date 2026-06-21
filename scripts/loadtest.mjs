// Concurrent load / stress test for Nucleus search (no dependencies; Node 18+).
//
// Verifies throughput, latency percentiles AND correctness: the top-1 chunk for
// each query under concurrency must match a sequential baseline (hybrid search
// is deterministic, so any mismatch would expose a data race).
//
// Two modes:
//   * sweep (default): a wave of NUC_REQS requests at each concurrency in NUC_CONCS.
//   * sustained: set NUC_DURATION>0 to hammer at NUC_CONC for that many seconds.
//
// Env:
//   NUC_BASE   (http://127.0.0.1:8080)   NUC_TOKEN (required)   NUC_DOMAIN (1)
//   NUC_K (5)
//   NUC_CONCS  (1,4,8,16,32,64,128)      NUC_REQS (300)         # sweep
//   NUC_DURATION (0 = sweep)             NUC_CONC (64)          # sustained
//
// Usage (PowerShell):
//   $env:NUC_TOKEN = (Get-Content path\to\admin_token.txt -Raw).Trim()
//   node scripts/loadtest.mjs                       # sweep
//   $env:NUC_DURATION="90"; $env:NUC_CONC="128"; node scripts/loadtest.mjs   # sustained

const BASE = process.env.NUC_BASE ?? "http://127.0.0.1:8080";
const TOKEN = (process.env.NUC_TOKEN ?? "").trim();
const DOMAIN = Number(process.env.NUC_DOMAIN ?? 1);
const K = Number(process.env.NUC_K ?? 5);
const DURATION = Number(process.env.NUC_DURATION ?? 0);
const CONC = Number(process.env.NUC_CONC ?? 64);
const CONCS = (process.env.NUC_CONCS ?? "1,4,8,16,32,64,128").split(",").map(Number);
const REQS = Number(process.env.NUC_REQS ?? 300);
const H = { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" };

const QUERIES = [
  "tipos de retención de IRPF en 2026",
  "ayudas en el IRPF por los daños de la DANA",
  "plazos del calendario del contribuyente 2025",
  "real decreto-ley aprobado en 2026",
  "deducción por maternidad en el IRPF",
  "tipos de IVA aplicables",
  "régimen especial de agricultura y ganadería",
  "obligaciones de facturación electrónica",
  "deducciones por familia numerosa",
  "calendario de la campaña de la renta",
];

async function search(q) {
  const t = performance.now();
  let res;
  try {
    res = await fetch(`${BASE}/v1/domains/${DOMAIN}/search`, {
      method: "POST", headers: H, body: JSON.stringify({ query: q, k: K }),
    });
  } catch (e) {
    return { ok: false, ms: performance.now() - t, err: String(e) };
  }
  const ms = performance.now() - t;
  if (!res.ok) return { ok: false, ms, shed: res.status === 503, status: res.status };
  const hits = await res.json();
  if (!Array.isArray(hits)) return { ok: false, ms, err: "not an array" };
  return { ok: true, ms, top: hits[0]?.chunk_id ?? null, n: hits.length };
}

const pctl = (s, p) => (s.length ? s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))] : 0);

function summarize(label, lat, ok, errs, shed, mismatch, wallMs) {
  lat.sort((a, b) => a - b);
  return {
    label, ok, errs, shed, mismatch,
    rps: ok / (wallMs / 1000),
    p50: pctl(lat, 50), p90: pctl(lat, 90), p99: pctl(lat, 99), max: lat[lat.length - 1] ?? 0,
  };
}

async function waveByCount(total, conc, expected) {
  const lat = []; let ok = 0, errs = 0, shed = 0, mismatch = 0, idx = 0;
  const t0 = performance.now();
  const worker = async () => {
    while (true) {
      const i = idx++; if (i >= total) break;
      const q = QUERIES[i % QUERIES.length];
      const r = await search(q);
      if (!r.ok) { if (r.shed) shed++; else errs++; continue; }
      ok++; lat.push(r.ms);
      if (expected[q] !== undefined && r.top !== expected[q]) mismatch++;
    }
  };
  await Promise.all(Array.from({ length: conc }, worker));
  return summarize(String(conc), lat, ok, errs, shed, mismatch, performance.now() - t0);
}

async function sustained(durationMs, conc, expected) {
  const lat = []; let ok = 0, errs = 0, shed = 0, mismatch = 0, n = 0;
  const deadline = performance.now() + durationMs;
  const t0 = performance.now();
  const worker = async () => {
    while (performance.now() < deadline) {
      const q = QUERIES[n++ % QUERIES.length];
      const r = await search(q);
      if (!r.ok) { if (r.shed) shed++; else errs++; continue; }
      ok++; lat.push(r.ms);
      if (expected[q] !== undefined && r.top !== expected[q]) mismatch++;
    }
  };
  await Promise.all(Array.from({ length: conc }, worker));
  return summarize(`${conc}@${Math.round(durationMs / 1000)}s`, lat, ok, errs, shed, mismatch, performance.now() - t0);
}

function row(r) {
  return `${r.label.padStart(8)} | ${String(r.ok).padStart(5)} | ${String(r.shed).padStart(4)} | ` +
    `${String(r.errs).padStart(3)} | ${String(r.mismatch).padStart(4)} | ${r.rps.toFixed(0).padStart(6)} | ` +
    `${r.p50.toFixed(0).padStart(4)}ms | ${r.p90.toFixed(0).padStart(4)}ms | ` +
    `${r.p99.toFixed(0).padStart(5)}ms | ${r.max.toFixed(0)}ms`;
}

async function main() {
  if (!TOKEN) { console.error("NUC_TOKEN not set"); process.exit(2); }
  const expected = {};
  for (const q of QUERIES) {
    const r = await search(q);
    if (!r.ok) { console.error("baseline failed:", q, r); process.exit(2); }
    expected[q] = r.top;
  }
  console.log(`baseline OK (${QUERIES.length} queries) — mode: ${DURATION > 0 ? "sustained" : "sweep"}`);
  console.log("   label |    ok | shed | err | mism |   rps  |  p50  |  p90  |   p99  |  max");
  console.log("---------+-------+------+-----+------+--------+-------+-------+--------+------");

  let totalErr = 0, totalMis = 0;
  if (DURATION > 0) {
    const r = await sustained(DURATION * 1000, CONC, expected);
    console.log(row(r)); totalErr += r.errs; totalMis += r.mismatch;
  } else {
    for (const c of CONCS) {
      const r = await waveByCount(REQS, c, expected);
      console.log(row(r)); totalErr += r.errs; totalMis += r.mismatch;
    }
  }
  console.log("---------");
  console.log(totalErr === 0 && totalMis === 0
    ? "RESULT: PASS — 0 errores, 0 resultados incorrectos (los 'shed'/503 son load-shed esperado)"
    : `RESULT: FAIL — ${totalErr} errores, ${totalMis} mismatches`);
  process.exit(totalErr === 0 && totalMis === 0 ? 0 : 1);
}

main();
