# Camino a la 1.0 → 1.0.0 alcanzada

> **Estado: 1.0.0.** Este documento fue la checklist para llegar aquí; se conserva como
> historial de cómo se llegó y ahora también fija el **compromiso de estabilidad** que
> `1.0.0` implica. Para nuevas features, ver el README y `docs/`; para bugs, los issues
> de GitHub.

## Compromiso de estabilidad (SemVer desde 1.0.0)

Nucleus sigue [SemVer](https://semver.org/). A partir de `1.0.0`, lo que sigue es un
**contrato**, no una intención — un cambio que lo rompa exige un major (`2.0.0`):

- **API HTTP** (`docs/api.md`, `docs/openapi.yaml`): rutas, campos de petición/respuesta
  y códigos de estado documentados. Romper esto es: quitar/renombrar un endpoint o un
  campo, cambiar el tipo de un campo, o cambiar un comportamiento por defecto del que
  un cliente ya dependa. **No** rompe: añadir un endpoint nuevo, añadir un campo
  opcional, mejorar el ranking/latencia sin cambiar la forma de la respuesta.
- **C-ABI del FFI** (`crates/ffi/include/nucleus.h`): firmas de las funciones `extern
  "C"` existentes. Romper esto es cambiar la firma de una función ya publicada.
  Añadir funciones nuevas no rompe nada.
- **Compatibilidad de esquema de BD**: cubierta aparte y con su propio contrato en
  [`compatibilidad-esquema.md`](compatibilidad-esquema.md) (qué `SCHEMA_VERSION` migra
  solo y qué no) — independiente del SemVer de la API, pero igual de vinculante.

**Explícitamente fuera del contrato SemVer** (pueden cambiar en cualquier `1.x`):

- El crate `nucleus-core` como API de Rust: es un **crate interno del workspace, no
  publicado en crates.io**. Quien lo use directamente (en vez de vía HTTP o FFI) asume
  que su superficie pública puede moverse entre minors.
- Características de rendimiento (throughput, latencias, uso de RAM) — mejoran con el
  tiempo pero no son una garantía numérica.
- Cualquier cosa marcada explícitamente como experimental en el README o los docs.

## Cómo se llegó aquí (histórico)

### Estado en el momento del bump (v0.2.0 → v1.0.0)

- ✅ **114 tests** en verde (78 motor + 6 integración del motor + 15 C-ABI del FFI [1 ignorado,
  descarga el modelo] + 15 e2e HTTP) + clippy `-D warnings` + `cargo fmt --check`.
- ✅ Hardening del servidor: auth por token con scopes (con matriz de tests 403), dedup por
  hash, apagado ordenado con volcado de índices, versionado de esquema con gate de migración,
  `/healthz` + `/readyz` + `/metrics`, CORS opt-in, listados paginados, rate limiting por IP
  (token-bucket, opt-in a `X-Forwarded-For` tras proxy de confianza), Docker.
- ✅ **Cifrado en reposo siempre activo** (XChaCha20-Poly1305 + HMAC de índices, Argon2id o clave
  de máquina con DPAPI en Windows), con migración automática de bases antiguas sin cifrar.
- ✅ **CI multiplataforma**: lint en Linux + tests, build del cdylib y smoke de carga
  (concurrencia vs. baseline secuencial) en Linux x64, Windows x64 y macOS arm64.
- ✅ Modo embebido (DLL/so/dylib) con su binding C#, empaquetado en las 3 plataformas; modo
  servidor (HTTP) con axum, empaquetado en Windows y Linux.
- ✅ Guía de compatibilidad de esquema entre versiones.

### Alcance de plataformas (decidido 2026-07, vigente en 1.0.0)

- **Windows x64, Linux x64 y macOS arm64 (Apple Silicon).** macOS Intel (x64) queda fuera de
  alcance: GitHub ya no programa runners para `macos-13`. Revisar si aparece demanda real.
- El modo **embebido** se empaqueta y publica en las 3 plataformas. El modo **servidor**
  solo tiene bundle reproducible para Windows y Linux; un bundle de servidor para macOS
  (+ unidad `launchd`) queda para más adelante si se necesita.

### Bloqueantes resueltos antes del bump

1. ✅ CI en verde en las 3 plataformas (el bloqueo real era `macos-13` sin runner disponible,
   no un fallo de compilación — corregido quitándolo de las matrices).
2. ✅ Bundle de Windows reproducible (`packaging/build-dll.ps1`).
3. ✅ Bundles de Linux/macOS reproducibles (`packaging/build-lib.sh`).
4. ✅ Tests HTTP de autorización (matriz de scopes, no solo camino feliz + 401).
5. ✅ Rate limit tras proxy (`NUCLEUS_TRUST_PROXY`, opt-in).
6. ✅ Smoke de carga/concurrencia en CI (`concurrent_search_matches_sequential_baseline`).
7. ✅ Guía de compatibilidad de esquema (`compatibilidad-esquema.md`).

## Futuro (post-1.0.0)

- **macOS Intel (x64)**: solo si aparece demanda real y se resuelve el problema de runner
  (self-hosted o servicio de terceros).
- **Bundle de servidor para macOS** (+ unidad `launchd` equivalente a `nucleus.service`).
- **Auto-inducción** de subdominios/labels (clustering + reglas, sin LLM) — el diferencial de
  producto que figura en "próximos pasos" del README.

Cualquiera de estas es aditiva (nuevo `minor`), no rompe el contrato de `1.0.0`.

> Nota: el **tag y la publicación de una release** son acciones de cara al exterior; no las haré
> sin tu confirmación explícita.
