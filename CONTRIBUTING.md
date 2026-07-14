# Contribuir a Nucleus

¡Gracias por tu interés en mejorar Nucleus! Este documento describe cómo hacerlo.

> ¿Tienes una pregunta, un bug o una idea y no sabes si merece un PR? Abre un
> [issue](https://github.com/JavierFrauca/nucleus/issues) y lo hablamos.

## Antes de empezar

Nucleus es un motor de base de datos para RAG escrito en Rust. Tener leído el
[dossier técnico](docs/dossier-tecnico.md) (las decisiones de diseño y los
límites) ayuda a que las contribuciones encajen. En particular:

- **El motor va dentro del proceso**: no hay servicios externos de embeddings ni
  un LLM generativo embebido. Mantén esa frontera.
- **`nucleus-core` no depende de HTTP**: la capa de servidor es `nucleus-server`.
  Una feature nueva va en `core` si es del dominio del motor (indexación,
  búsqueda, storage…); va en `server` si es de transporte (rate limiting,
  endpoints, DTOs).
- **Stabilidad SemVer desde 1.0**: la API HTTP y el C-ABI del FFI siguen
  [SemVer](docs/camino-a-1.0.md). Un cambio incompatible exige un *major*.

## Entorno de desarrollo

Requisitos: **Rust 1.82+** (ver `rust-toolchain` / `rust-version` en
`Cargo.toml`) y, opcionalmente, Node 18+ para el [script de
carga](scripts/loadtest.mjs).

```bash
git clone https://github.com/JavierFrauca/nucleus.git
cd nucleus

# Compilar todo (core + server + ffi)
cargo build --workspace

# Tests (no descargan el modelo de embeddings; usan MockEmbedder)
cargo test --workspace

# Benchmarks (criterion)
cargo bench -p nucleus-core --bench search
```

> La **primera** compilación tarda: descarga y compila ONNX Runtime, fastembed,
> redb, etc. Las siguientes son incrementales.

## Flujo de trabajo

1. **Fork + rama** desde `main`:
   ```bash
   git checkout -b feat/descripcion-corta
   ```
   Convención de nombres: `feat/...`, `fix/...`, `docs/...`, `test/...`,
   `perf/...`, `refactor/...`.

2. **Escribe código** que encaje con el estilo circundante. El proyecto usa
   `cargo fmt` y `cargo clippy -D warnings`; el CI los impone.

3. **Añade o actualiza tests.** El motor es fácil de testear aislado con
   `MockEmbedder` (sin descargar modelos). Ver [guía de
   testing](docs/testing.md) y los patrones en
   `crates/core/tests/engine_integration.rs`.

4. **Verifica en local** antes de pushear:
   ```bash
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

5. **Commits**: escribe mensajes claros, en presente e imperativo
   (`feat: soportar formato X`, `fix: race condition en search`, `docs: ...`).
   Mantén el *scope* de cada commit pequeño y autocontenido.

6. **Abre el Pull Request** contra `main` rellenando la plantilla (qué cambia,
   por qué, cómo se testea). Enlaza el issue si lo hubiera.

## Qué busca el CI

El workflow `.github/workflows/ci.yml` corre en cada push/PR:

- `cargo fmt --all --check` — formato.
- `cargo clippy --workspace --all-targets -- -D warnings` — sin warnings.
- `cargo test --workspace` en **Windows, Linux y macOS**.
- `cargo build -p nucleus-ffi --release` (la cdylib multiplataforma).

Tu PR debe pasar todo eso. Si añades una dependencia, justifica por qué ninguna
existente sirve y que no rompa la DLL autocontenida de Windows.

## Decisiones de diseño a respetar

- **Embeddings in-process** (fastembed/ONNX). No introduzcas dependencias de
  servicios externos para inferencia.
- **redb + bincode** para persistencia ACID; el índice vectorial es derivado y
  reconstruible. No inventes otro store.
- **IDs tipados** (`DomainId`, `ChunkId`…): no los eludas pasando `u64` crudos.
- **`Engine` es síncrono y `&self`**: la asincronía vive en la capa servidor
  (`spawn_blocking`). El FFI no usa tokio.
- **Errores con `thiserror`** y `Result<T, NucleusError>`; no `panic` en caminos
  esperables.

## Ámbitos donde se agradece ayuda

- **Clientes / SDKs** en más lenguajes (Python, Go, Rust sobre la cdylib).
- **Extractores** de más formatos.
- **Modelos**: más modelos de embeddings detrás del trait `Embedder`.
- **Recuperación**: tuning de BM25/RRF, nuevos rerankers.
- **Documentación y ejemplos**: casos de uso reales, guías paso a paso.
- **Benchmarks**: ampliar `crates/core/benches/` con escenarios nuevos.

## Reportar bugs de seguridad

No abras un issue público para vulnerabilidades. Escribe a
`javier.frauca` (vía GitHub) describiendo el problema y el impacto. Mira
[operación → seguridad](docs/operacion.md#seguridad) para el modelo de amenazas.

## Código de conducta

Sé respetuoso y constructivo. El objetivo es un proyecto útil y acogedor; no se
tolera comportamiento acosador o denigrante. Los maintainers se reservan el
derecho de cerrar hilos o bloquear usuarios que lo vulneren.

¡Gracias por contribuir!
