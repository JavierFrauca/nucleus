# Nucleus

[![Release](https://img.shields.io/github/v/release/JavierFrauca/nucleus?sort=semver)](https://github.com/JavierFrauca/nucleus/releases/latest)
[![Descargar](https://img.shields.io/badge/descargar-windows%20%C2%B7%20linux%20%C2%B7%20macOS%20arm64-blue)](https://github.com/JavierFrauca/nucleus/releases/latest)

**Base de datos ad-hoc para RAG, embebible en tu aplicación.** Escrita en Rust.
Nucleus es un motor todo-en-uno: almacena, indexa y **genera los embeddings en
proceso**.

> **1.0.0 — API estable.** Desde esta versión, la API HTTP y el C-ABI del FFI siguen
> [SemVer](https://semver.org/): un cambio incompatible exige un major. Ver el
> compromiso exacto (qué cuenta como *breaking* y qué no) en
> [camino a la 1.0](docs/camino-a-1.0.md#compromiso-de-estabilidad-semver-desde-100).

> **Cambio de rumbo (2026-06):** el foco de Nucleus es ahora el **modo embebido** —
> una base de datos vectorial *ad-hoc* que tu app referencia como **librería nativa**
> (`nucleus.dll` / `libnucleus.so` / `libnucleus.dylib`), sin red, sin sidecar, sin
> servicio que desplegar: «SQLite, pero para RAG con embeddings dentro». Disponible para
> **Windows x64, Linux x64 y macOS arm64** (Apple Silicon; macOS Intel no se soporta).
> El servidor HTTP sigue existiendo como **segundo modo** para despliegues
> cliente-servidor.

## Dos modos, un mismo motor

| Modo | Cuándo | Cómo |
|------|--------|------|
| **Embebido (DLL)** — *foco* | RAG pequeño/rápido integrado en una app de escritorio o de servidor | Referencias `nucleus.dll` por C ABI (binding C# incluido). [Ver abajo](#modo-embebido-dll). |
| **Servidor (HTTP)** | Despliegue cliente-servidor, varios consumidores | Binario `nucleus-server` (axum). [Ver abajo](#ejecutar-el-servidor). |

Ambos comparten el crate del motor [`nucleus-core`](crates/core/README.md), que
está organizado en torno a dos ejes de primera clase:

- **Dominios** — colecciones/namespaces que segmentan la base. Cada dominio fija un
  modelo de embeddings (y por tanto una dimensión) y tiene su propio índice vectorial
  y su propio vocabulario de etiquetas.
- **Etiquetado** — taxonomía jerárquica por dominio, asociada a los chunks y usada
  para filtrar en la búsqueda.

En ambos modos se **recuperan chunks** mediante búsqueda **híbrida** (vectorial +
léxico BM25 fusionados con RRF), con **reranking** opcional, filtro de etiquetas y un
lenguaje de consulta para filtros ricos.

## Descargar

Los bundles del **modo embebido** están disponibles en
[**Releases**](https://github.com/JavierFrauca/nucleus/releases/latest) para **Windows x64**,
**Linux x64** y **macOS arm64** (Apple Silicon):

- **`nucleus-dll-<versión>-windows-x64.zip`** (~11 MB) — `nucleus.dll` autocontenida + import
  lib + header C [`nucleus.h`](crates/ffi/include/nucleus.h) + binding C# tipado + README. En
  Windows la DLL es autocontenida (ONNX Runtime enlazado estático).
- **`nucleus-lib-<versión>-linux-x64.tar.gz`** / **`nucleus-lib-<versión>-macos-arm64.tar.gz`** —
  `libnucleus.so` / `libnucleus.dylib` + header C + binding C#, con ONNX Runtime empaquetado al
  lado si no se enlazó estático.

En todas, la primera ingesta descarga el modelo de embeddings (~450 MB). Para compilar desde
fuente o regenerar los bundles, ver [requisitos de build](#requisitos-de-build) y
`packaging/build-dll.ps1` (Windows) / `packaging/build-lib.sh` (Linux/macOS).

## Documentación

Guías detalladas en [`docs/`](docs/):

- [Instalación](docs/instalacion.md) — toolchain, build, feature GPU, notas de Windows/disco.
- [Guía rápida](docs/guia-rapida.md) — de cero a buscar en 5 pasos (curl y PowerShell).
- [Conceptos](docs/conceptos.md) — dominio, subdominio, labels, documentos, chunks, embeddings, índices.
- [Configuración](docs/configuracion.md) — variables de entorno, índice flat/hnsw, GPU.
- [Referencia de la API](docs/api.md) — todos los endpoints con ejemplos.
- [Contrato OpenAPI](docs/openapi.yaml) — especificación formal (genera clientes/docs).
- [Lenguaje de consulta](docs/lenguaje-consulta.md) — el campo `filter`.
- [Operación](docs/operacion.md) — seguridad, **cifrado en reposo**, jobs, persistencia, memoria, backups.
- [Compatibilidad de esquema](docs/compatibilidad-esquema.md) — qué versión de Nucleus abre qué BD, y qué migra solo.
- [Rendimiento y carga](docs/rendimiento.md) — benchmarks reales (throughput, latencia, RAM/CPU, límites).
- [Instalación y empaquetado](packaging/README.md) — Docker, binario/instalador Windows/Linux.
- [Arquitectura](docs/arquitectura.md) — crates, módulos, flujos y decisiones.
- [Dossier técnico](docs/dossier-tecnico.md) — guía completa de defensa: decisiones, alternativas, límites y preguntas difíciles.
- [Resumen de defensa](docs/resumen-defensa.md) — one-pager para imprimir · [diagrama](docs/arquitectura.svg).
- [Camino a la 1.0](docs/camino-a-1.0.md) — historial de cómo se llegó a 1.0.0 y el compromiso de estabilidad (SemVer) vigente desde esa versión.

## Clientes / SDKs

**Embebido (DLL, foco)** — sin red, en proceso:

- **C# / .NET** — [`clients/csharp/Nucleus.Native`](clients/csharp/Nucleus.Native) (P/Invoke sobre `nucleus.dll`).
- **Rust** — el crate [`nucleus-core`](crates/core/README.md) directamente (sin FFI).
- **C / C++ / otros** — el C ABI de [`nucleus.dll`](crates/ffi) vía [`nucleus.h`](crates/ffi/include/nucleus.h).

**Cliente-servidor (HTTP)** — contra `nucleus-server`:

- **C# / .NET** — [`clients/csharp/Nucleus.Client`](clients/csharp) (`netstandard2.0` + `net8.0`). El
  `release.yml` lo empaqueta y publica en NuGet.org como `Nucleus.Client` en cada tag
  (requiere el secret `NUGET_API_KEY` del repo); hasta que se configure, referencia el
  proyecto directamente.
- **JavaScript / TypeScript** — [`clients/typescript`](clients/typescript) (ESM, Node y navegador).
- Otros lenguajes: genera un cliente desde [`docs/openapi.yaml`](docs/openapi.yaml).

**Ejemplos ejecutables** en [`examples/`](examples/README.md): demo de consola C# (menú),
demo headless de Node, y un mini-front de navegador con 2 pantallas (ingesta y búsqueda).

## Características

- **Embeddings in-process** con [`fastembed`](https://github.com/Anush008/fastembed-rs)
  (ONNX Runtime). Modelo por defecto **multilingüe** `multilingual-e5-small` (384d),
  configurable por dominio. La API también acepta vectores precomputados.
- **Almacenamiento embebido** con [`redb`](https://www.redb.org/) (ACID, puro Rust),
  valores serializados con **bincode 2**.
- **Índice vectorial** exacto (coseno, fuerza bruta) detrás del trait `VectorIndex`,
  **HNSW** aproximado y persistente para gran escala, o **int8 (cuantización escalar)**
  con ~4× menos RAM y recall casi exacto (mismo trait).
- **Chunking *boundary-aware***: corta en frontera de frase (con _fallback_ a espacio),
  nunca a mitad de palabra, con solapamiento configurable.
- **Diversidad (MMR)** opcional en la búsqueda (`diversity` ∈ [0,1]) para reducir
  redundancia entre resultados, y **snippet** resaltado por hit.
- **Búsqueda híbrida**: índice **léxico BM25** + vectorial fusionados con **RRF**, para
  recuperar tanto lo semánticamente parecido como las citas literales (códigos,
  artículos). **Reranking** opcional con *cross-encoder* in-process (`bge-reranker-base`).
- **Jobs** persistidos en redb + workers tokio para ingesta escalable; la inferencia
  corre en `spawn_blocking`. La cola sobrevive a reinicios.
- **Seguridad por token** tipo API-key (opaco, hasheado con SHA-256) con scopes por
  dominio (`Read` / `Write` / `Admin`).
- **Cifrado en reposo siempre activo**: cada valor se cifra con **XChaCha20-Poly1305**
  (post-cuántico-seguro) y las claves de índice sensibles (nombres de tags/subdominios,
  pares clave/valor de metadatos, hashes de contenido) se ofuscan con **HMAC con clave**,
  así no quedan en claro en disco pero los lookups exactos siguen funcionando. La clave se
  deriva de una passphrase con **Argon2id**, o, si no se da, es una **clave de máquina**
  automática protegida por el SO (DPAPI en Windows).
  El fichero de clave vive **separado de la base de datos** (nunca dentro del backup; se
  respalda aparte). Las bases sin cifrar de versiones previas se **migran solas** al abrirlas.
- **Copias de seguridad a nivel de motor**: full (snapshot consistente) y diferencial
  (delta binario, *full-fidelity*), programables (min/horas/días/semanas) con retención, y
  **restore en caliente** (swap del motor). Ver [operación](docs/operacion.md#backups-y-restauración).
- **API HTTP** con axum.

## Modo embebido (DLL)

El modo **prioritario**: Nucleus dentro de tu proceso, sin HTTP. Tu app enlaza la librería
nativa (`nucleus.dll` / `libnucleus.so` / `libnucleus.dylib`) y llama al motor directamente.
**Se distribuye para Windows x64, Linux x64 y macOS arm64** (Apple Silicon; macOS Intel no se
soporta). En Windows la DLL es **autocontenida** (~28 MB): `ort`/ONNX Runtime se enlaza
**estáticamente**, así que no hay que repartir `onnxruntime.dll`. En Linux/macOS el bundle
incluye la librería de ONNX Runtime al lado si no quedó enlazada estática. Lo único que se
descarga la primera vez (en cualquier plataforma) es el modelo de embeddings (~450 MB).

- **Crate**: [`crates/ffi`](crates/ffi) (`nucleus-ffi`, `crate-type = ["cdylib"]`).
- **C ABI**: handle opaco + borde **JSON** (entrada/salida son strings JSON; código
  de retorno `0` OK / `<0` error con `{"error":...}` y `nucleus_last_error()`).
  Header C en [`crates/ffi/include/nucleus.h`](crates/ffi/include/nucleus.h).
  Funciones: `open`/`close`, `create_domain`, `ingest_text`, `ingest_file` (extracción
  multi-formato: pdf/docx/xlsx/html/md/txt), `search` (con MMR
  `diversity` y `snippet`), `search_multi`, `list_domains`/`list_tags`/`list_subdomains`/`list_documents`,
  `get_document`, `chunk_context`, edición/cascada (`rename_domain`, `delete_domain`,
  `delete_subdomain`, `update_tag`, `delete_tag`, `update_document`, `delete_document`),
  `reindex_domain`, `persist_indexes`, `string_free`, `last_error`.
- **Índice**: `index_kind` = `"flat"` (exacto, por defecto), `"hnsw"` (aproximado) o
  `"sq"` (cuantización int8, **~4× menos RAM** con recall casi exacto — ideal para
  embeber en apps con poca huella de memoria).
- **Binding C#** listo para usar: [`clients/csharp/Nucleus.Native`](clients/csharp/Nucleus.Native)
  (`NucleusEngine : IDisposable`, P/Invoke).
- **Camino síncrono**: la ingesta (chunk → embed → persist → index) y la búsqueda
  corren en el hilo del llamante (sin tokio ni cola de jobs). La app controla su
  propio threading; el handle es `Send + Sync`.

```csharp
using Nucleus.Native;

// Sin db_path -> base por usuario en %LOCALAPPDATA%\Nucleus\nucleus.redb
using var engine = NucleusEngine.Open("data/nucleus.redb", modelCache: "models");
Domain domain = engine.CreateDomain("legal");

engine.IngestText(domain.Id, "Contrato", "El arrendador podrá rescindir…", labels: ["contratos"]);

// Devuelve objetos tipados (Domain, SearchHit, Chunk…), no JSON crudo.
foreach (SearchHit hit in engine.Search(domain.Id, "cómo terminar un contrato antes de tiempo", k: 5, labels: ["contratos"]))
    Console.WriteLine($"{hit.Score:F3}  {hit.Chunk.Text}");
```

**Ubicación por defecto**: si no se indica `db_path`, la BD se crea por usuario en
`%LOCALAPPDATA%\Nucleus\nucleus.redb` (Windows) o `$XDG_DATA_HOME`/`~/.local/share`
`/nucleus/nucleus.redb` (otros). El directorio se crea solo.

**Empaquetado**: `packaging/build-dll.ps1 -Version X` produce
`dist/nucleus-dll-X-windows-x64.zip` (~11 MB) con `nucleus.dll`, la import lib, el
header C, el binding C# y un README. Ejemplo end-to-end ejecutable en
[`examples/ffi-smoke`](examples/ffi-smoke).

## Arquitectura

```
crates/
├── core/   (nucleus-core)  — librería del motor, sin dependencias HTTP
│   ├── error.rs        NucleusError + Result
│   ├── id.rs           DomainId/DocumentId/ChunkId/TagId/JobId/TokenId (newtypes)
│   ├── model/          Domain, Document, Chunk, Tag
│   ├── storage/        redb: tablas + índices secundarios + códec bincode
│   ├── index/          trait VectorIndex + FlatIndex (coseno)
│   ├── embed/          trait Embedder + LocalEmbedder (fastembed) + MockEmbedder
│   ├── chunking.rs     Chunker + FixedSizeChunker
│   ├── jobs/           cola persistida + workers
│   ├── auth.rs         ApiToken, Scope, hashing/verificación
│   └── engine.rs       Engine: une todo (ingest / search / admin)
├── ffi/    (nucleus-ffi)    — C ABI (cdylib): nucleus.dll para embeber en apps
│   ├── src/lib.rs      funciones extern "C" + borde JSON
│   └── include/nucleus.h   header C para C/C++
└── server/ (nucleus-server) — binario HTTP (axum)
    └── src/{main,app,routes}.rs
```

## Requisitos de build

- **Rust** (toolchain MSVC) y los **VS C++ Build Tools** (necesarios para enlazar y
  para que `ort`/ONNX Runtime compile). En Windows:
  ```powershell
  winget install Rustlang.Rustup
  winget install Microsoft.VisualStudio.2022.BuildTools `
    --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  ```
- El perfil `dev` usa `debug = 0` (ver `Cargo.toml`) para reducir el tamaño de
  `target/`: el grafo de dependencias (ONNX, tokenizers, códecs de imagen) es grande.
- **Build sin salida directa a internet** (proxy con inspección TLS): `ort` descarga
  ONNX Runtime al compilar. Si la descarga falla con `UnknownIssuer`, baja el `.tgz`
  de ONNX Runtime aparte, descomprímelo y apunta `ORT_LIB_LOCATION` a la carpeta que
  contiene `lib/onnxruntime.lib` antes de compilar.

```bash
cargo build            # workspace
cargo test --workspace # 115 tests (core, integración del motor, C-ABI del FFI y e2e HTTP)
cargo clippy --workspace --all-targets
cargo build --features gpu  # opcional: inferencia por GPU (ONNX DirectML)

# Docker (multi-stage; ver Dockerfile)
docker build -t nucleus .
docker run -p 8080:8080 -v nucleus_data:/data nucleus
```

## Ejecutar el servidor

```bash
cargo run -p nucleus-server
```

Variables de entorno (con sus valores por defecto):

| Variable               | Defecto              | Descripción                              |
|------------------------|----------------------|------------------------------------------|
| `NUCLEUS_DB`           | `%ProgramData%\Nucleus\nucleus.redb` (Win) · `/var/lib/nucleus/nucleus.redb` | Ruta del fichero de base de datos |
| `NUCLEUS_ADDR`         | `127.0.0.1:8080`     | Dirección de escucha                     |
| `NUCLEUS_WORKERS`      | `2`                  | Nº de workers de jobs                    |
| `NUCLEUS_MODEL_CACHE`  | (caché de fastembed) | Directorio donde cachear los modelos     |
| `NUCLEUS_INDEX`        | `flat`               | Backend de índice: `flat` (exacto) o `hnsw` |
| `NUCLEUS_INDEX_DIR`    | `<dir BD>/nucleus_indexes` | Dónde se vuelca/carga el grafo HNSW |
| `NUCLEUS_GPU`          | `false`              | `true` para inferencia en GPU (requiere build `--features gpu`) |
| `NUCLEUS_RATE_LIMIT_RPM` | `0` (desactivado)  | Peticiones/min por IP (token-bucket); `0` lo desactiva |
| `NUCLEUS_TRUST_PROXY`  | `false`              | `true` para que el rate limit use `X-Forwarded-For` en vez de la IP del peer TCP. Solo si un proxy de confianza sobrescribe esa cabecera — ver [operación](docs/operacion.md#seguridad) |
| `NUCLEUS_PASSPHRASE`   | (vacío)              | Passphrase para el **cifrado en reposo** (siempre activo). Con frase, la clave se deriva con Argon2id (portable, reabre en cualquier máquina). Sin frase, se usa una **clave de máquina** automática protegida por el SO |
| `NUCLEUS_KEYFILE`      | (config de usuario)  | Ruta del fichero de clave de máquina (solo sin passphrase). Por defecto, un directorio de configuración del usuario **separado de la BD** (`%APPDATA%\Nucleus\nucleus.key` · `~/.config/nucleus/...`). La clave **nunca** se guarda con los datos: respáldala aparte |

Al primer arranque, si no hay tokens, se imprime **una sola vez** un token admin:

```
========================================================
 Nucleus bootstrap admin token (store it — shown once):
   nuc_xxxxxxxx...
========================================================
```

> La primera ingesta/búsqueda **descarga el modelo** de HuggingFace
> (`multilingual-e5-small`, ~450 MB) y lo cachea. Requiere red e espacio en disco esa
> primera vez.

## Dashboard web

**Prototipo.** `GET /dashboard` sirve un explorador/gestor mínimo (HTML+JS vainilla,
sin build ni dependencias, embebido en el binario — inspirado en el de
[Qdrant](https://qdrant.tech/)). No tiene autenticación propia: es markup inerte que
pide el token en el navegador (se guarda en `sessionStorage`, nunca en disco) y lo usa
como `Authorization: Bearer` contra la misma API `/v1/*` — mismos scopes, mismo 403 que
cualquier otro cliente. Cubre: alta/listado de dominios, búsqueda, listado/alta/borrado
básico de documentos, etiquetas y subdominios, y **backups** (crear copia full y
restaurar, requiere token Admin). No cubre (aún): ingesta de documentos ni gestión de
tokens desde la UI, ni editar/borrar dominios, etiquetas o subdominios (esas
operaciones existen en el motor y el FFI, pero la API HTTP todavía no las expone —
para eso, la API directamente). Ponlo tras TLS igual que el resto del servidor (ver
[operación](docs/operacion.md#seguridad)).

## API

Todas las rutas (salvo `/healthz`) requieren `Authorization: Bearer <token>`.

| Método & ruta                          | Permiso  | Descripción                          |
|----------------------------------------|----------|--------------------------------------|
| `GET /healthz`                         | —        | Health check                         |
| `GET /dashboard`                       | —        | Explorador/gestión web (prototipo). Sin auth propia: pide el token en el navegador y lo usa contra esta misma API — ver [más abajo](#dashboard-web) |
| `POST /v1/domains`                     | Admin    | Crear dominio                        |
| `GET /v1/domains`                      | auth     | Listar dominios                      |
| `GET /v1/domains/{id}`                 | Read     | Obtener dominio                      |
| `POST /v1/domains/{id}/documents`      | Write    | Ingestar documento (asíncrono)       |
| `POST /v1/domains/{id}/search`         | Read     | **Buscar chunks**                    |
| `POST /v1/domains/{id}/tags`           | Write    | Crear etiqueta (label)               |
| `GET /v1/domains/{id}/tags`            | Read     | Listar etiquetas                     |
| `POST /v1/domains/{id}/subdomains`     | Write    | Crear subdominio                     |
| `GET /v1/domains/{id}/subdomains`      | Read     | Listar subdominios                   |
| `GET /v1/documents/{id}`               | Read     | Obtener documento                    |
| `DELETE /v1/documents/{id}`            | Write    | Borrar documento + chunks            |
| `GET /v1/chunks/{id}`                  | Read     | Obtener un chunk                     |
| `GET /v1/chunks/{id}/context`          | Read     | Chunk + vecinos (`?before=&after=`)  |
| `GET /v1/jobs/{id}`                    | auth     | Estado de un job                     |
| `POST /v1/tokens`                      | Admin    | Crear token                          |
| `GET /v1/tokens`                       | Admin    | Listar tokens                        |
| `DELETE /v1/tokens/{id}`               | Admin    | Borrar token                         |
| `POST /v1/maintenance/persist`         | Admin    | Volcar los índices HNSW a disco      |

### Ejemplo (curl)

```bash
TOKEN=nuc_xxxxxxxx
BASE=http://127.0.0.1:8080

# Crear dominio (modelo por defecto: multilingual-e5-small)
curl -s -X POST $BASE/v1/domains -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"name":"docs"}'

# Crear una etiqueta
curl -s -X POST $BASE/v1/domains/1/tags -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"name":"legal","display_name":"Legal"}'

# Ingestar un documento (devuelve document_id y job_id)
curl -s -X POST $BASE/v1/domains/1/documents -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Manual","text":"El contrato laboral indefinido...","tags":[1]}'

# Consultar el job hasta que esté "Done"
curl -s $BASE/v1/jobs/1 -H "Authorization: Bearer $TOKEN"

# Buscar (recuperar chunks) filtrando por etiqueta
curl -s -X POST $BASE/v1/domains/1/search -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"query":"contrato laboral","k":5,"tags":[1]}'
```

El cuerpo de búsqueda admite `query` (texto, se embebe con el modelo del dominio) o
`query_vector` (vector precomputado), más `k`, `tags`, `match_all`, `document_ids`,
`subdomain` (nombre) y `filter` (ver abajo).

### Estructura en la ingesta (dominio → subdominio → labels)

La ingesta acepta la estructura **por nombre**, y el motor crea lo que falte (no hay
que pre-crear ni manejar ids):

- **dominio**: lo define el usuario (en la ruta).
- **subdominio**: campo `subdomain` (un nombre) en `/documents` y `/files`.
- **labels**: campo `labels` (lista de nombres) en `/documents`, o `?labels=a,b` en `/files`.

```bash
curl -s -X POST "$BASE/v1/domains/1/files?filename=IRPF_2026.pdf&subdomain=irpf&labels=2026,irpf" \
  -H "Authorization: Bearer $TOKEN" --data-binary @IRPF_2026.pdf
# luego, búsqueda acotada al subdominio:
curl -s -X POST $BASE/v1/domains/1/search -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"query":"retención IRPF 2026","subdomain":"irpf","k":5}'
```

La auto-inducción de subdominios/labels (clustering + reglas, **sin LLM**) es la fase
opcional siguiente; hoy la estructura la aporta quien ingesta.

### Lenguaje de query (`filter`)

El campo `filter` acepta una expresión booleana que se evalúa contra cada chunk
candidato (se interseca con el resto de filtros):

```text
tag:legal AND NOT tag:draft
tag:legal AND (meta.lang:es OR meta.lang:en)
doc:42 OR tag:"contrato marco"
```

- `tag:<nombre>` — el chunk lleva esa etiqueta (por nombre, dentro del dominio).
- `doc:<id>` — el chunk pertenece a ese documento.
- `meta.<clave>:<valor>` — metadato del chunk igual a `valor`.
- Operadores `AND`, `OR`, `NOT` (insensibles a mayúsculas), paréntesis y `"comillas"`
  para valores con espacios. `AND` liga más que `OR`.

```bash
curl -s -X POST $BASE/v1/domains/1/search -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"query":"contrato","k":5,"filter":"tag:legal AND NOT tag:draft"}'
```

### Contexto de un chunk (vecinos)

Los chunks de un documento se encadenan (`prev`/`next`). Para recuperar un chunk con
su contexto:

```bash
curl -s "$BASE/v1/chunks/12/context?before=1&after=2" -H "Authorization: Bearer $TOKEN"
```

### Backends de índice

Por defecto se usa un índice **exacto** (coseno por fuerza bruta), ideal para
exactitud y filtros precisos. Arrancando con `NUCLEUS_INDEX=hnsw` se usa **HNSW**
(aproximado) para gran escala; con pre-filtros los resultados son aproximados (HNSW
ordena globalmente y luego se interseca).

El grafo HNSW **persiste**: se vuelca a `NUCLEUS_INDEX_DIR` al apagar (Ctrl-C) o
mediante `POST /v1/maintenance/persist`, y se recarga al arrancar para no
reconstruirlo desde el almacenamiento (con _fallback_ a reconstrucción si no hay
volcado). El índice `flat` no persiste: se reconstruye, que es barato y exacto.

### Búsqueda híbrida y reranking

La búsqueda combina **siempre** el índice vectorial (semántico) con un índice **léxico
BM25** (términos literales), fusionando ambos con **RRF**. Así recupera tanto sinónimos
como citas exactas (un código, un artículo, un nombre propio). No requiere configuración.

Activando `NUCLEUS_RERANK_MODEL=bge-reranker-base` se añade una etapa final de
**reranking** con un *cross-encoder* in-process que re-puntúa los mejores candidatos
leyendo el par `(consulta, chunk)` completo: mejora el orden a cambio de algo de latencia.
Ver [configuración](docs/configuracion.md#búsqueda-híbrida-y-reranking).

### GPU

Compilando con `--features gpu` la inferencia de embeddings puede usar la GPU vía el
execution provider **DirectML** de ONNX Runtime (Windows), con _fallback_ automático
a CPU. Actívalo en runtime con `NUCLEUS_GPU=true`. Sin la feature, la build es solo CPU.

### Filtros (push-down)

El `filter` del query language se resuelve por **álgebra de conjuntos sobre los índices
secundarios** (lookups de `tag:`/`doc:`/`meta.*` combinados con ∩/∪/∖), no escaneando
cada chunk. Los chunks heredan la metadata de su documento, por lo que `meta.*` opera
sobre ella.

### Crear un token con scopes

```bash
curl -s -X POST $BASE/v1/tokens -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name":"app-lectura","scopes":[{"domain":{"One":1},"perm":"Read"}]}'
```

`domain` puede ser `"All"` o `{"One": <id>}`; `perm` es `"Read"`, `"Write"` o `"Admin"`.

## Modelos soportados

| id                       | dim | notas                          |
|--------------------------|-----|--------------------------------|
| `multilingual-e5-small`  | 384 | **defecto**, multilingüe       |
| `bge-small-en-v1.5`      | 384 | solo inglés                    |
| `all-minilm-l6-v2`       | 384 | solo inglés                    |

Los modelos e5 reciben automáticamente los prefijos `query:` / `passage:`.

## Estado de producción

Hardening hecho: búsqueda **híbrida léxico+vector** (RRF) con **reranking** in-process
opcional, transacción única por documento en la ingesta, cola de jobs con set de
pendientes + purga de terminados, locks sin envenenamiento (`parking_lot`), versionado de
esquema con gate de migración, deduplicación por hash de contenido, cotas de entrada,
apagado ordenado (Ctrl-C/SIGTERM) con volcado de índices, `/healthz` + `/readyz` +
`/metrics`, token admin a fichero (no a logs), CORS opt-in, **rate limiting** por IP,
listados paginados, Dockerfile y CI. El **borde C-ABI del FFI** (modo embebido) tiene
tests propios de su contrato (códigos de estado, JSON in/out, last-error, punteros).

## Próximos pasos

- **Calidad (foso)**: **auto-inducción** de subdominios/labels (clustering + reglas, sin LLM).
- **Operación**: borrado en cascada de dominios/subdominios/labels; rate limiting.
- **Escala**: mmap del grafo HNSW; workers de jobs distribuidos; multi-nodo si SaaS.
