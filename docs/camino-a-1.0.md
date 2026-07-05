# Camino a la 1.0

Checklist priorizada para que Nucleus pueda anunciarse como **estable / production-ready**
sin asteriscos. Marca lo hecho y lo que falta; al final, las decisiones que dependen de ti.

## Estado actual (v0.1.2 → en camino a v0.2.0)

- ✅ **106 tests** en verde (78 motor + 6 integración del motor + 15 C-ABI del FFI [1 ignorado,
  descarga el modelo] + 7 e2e HTTP) + clippy `-D warnings` + `cargo fmt --check`.
- ✅ Hardening del servidor: auth por token con scopes, dedup por hash, apagado ordenado con
  volcado de índices, versionado de esquema con gate de migración, `/healthz` + `/readyz` +
  `/metrics`, CORS opt-in, listados paginados, **rate limiting por IP** (token-bucket,
  `NUCLEUS_RATE_LIMIT_RPM`), Docker.
- ✅ **Cifrado en reposo siempre activo** (XChaCha20-Poly1305 + HMAC de índices, Argon2id o clave
  de máquina con DPAPI en Windows), con migración automática de bases antiguas sin cifrar.
- ✅ **CI multiplataforma**: lint en Linux + tests y build del cdylib en Linux x64, Windows x64 y
  macOS arm64.
- ✅ Modo embebido (DLL/so/dylib) con su binding C#; modo servidor (HTTP) con axum.
- ✅ Empaquetado embebido para las 3 plataformas soportadas: `build-dll.ps1` (Windows),
  `build-lib.sh` (Linux/macOS).

## Alcance decidido (2026-07)

- **Plataformas de la 1.0: Windows x64, Linux x64 y macOS arm64 (Apple Silicon).** macOS Intel
  (x64) queda **fuera de alcance**: GitHub ya no programa runners para la etiqueta `macos-13`
  (los jobs colgaban 24h "awaiting a runner" y bloqueaban `publish`), y no hay demanda que
  justifique un runner self-hosted para ello. Si en el futuro se necesita, requeriría una imagen
  self-hosted o un runner de terceros.
- El modo **embebido** (DLL/so/dylib) es el que se empaqueta y publica en las 3 plataformas. El
  modo **servidor** (`nucleus-server` binario) solo tiene bundle reproducible para Windows y
  Linux hoy; un bundle de servidor para macOS (+ unidad `launchd`) queda para más adelante si se
  necesita.
- **Versionado: seguimos en `0.x`** por ahora (sin congelar API). El bump a `1.0.0` se valorará más
  adelante; mientras tanto `0.x` permite iterar la API sin coste SemVer.

## Bloqueantes para 1.0 (must)

1. ~~Verificar la matriz de CI en verde (Linux + Windows).~~ **Resuelto.** La matriz sí se
   ejecutó, pero `macos-13` (Intel) colgaba 24h por falta de runner y cancelaba la ejecución
   completa — es la causa real por la que no se veía "verde". Se ha quitado `macos-13` de
   `ci.yml` y `release.yml`; con `[ubuntu-latest, windows-latest, macos-latest]` la matriz
   compila, testea y empaqueta sin bloqueos.
2. ✅ **Bundle de Windows pulido**: `packaging/build-dll.ps1` y el `.zip` de release son
   reproducibles, con `nucleus.h`, import lib, binding C# y README actualizado.
3. ✅ **Bundles de Linux/macOS**: `packaging/build-lib.sh` produce el `.tar.gz` embebido
   (`libnucleus.so`/`.dylib` + header + binding C#) para Linux x64 y macOS arm64, smoke-testado
   en cada run de CI.

## Recomendado antes de presumir de "production" (should)

4. **Tests HTTP de autorización**: cubrir scopes `Read`/`Write`/`Admin` por endpoint (hoy el e2e
   valida el camino feliz y el 401, pero no la matriz de permisos).
5. **Rate limit tras proxy**: honrar `X-Forwarded-For` de forma opt-in (cuando se confía en el
   proxy), porque hoy se limita por IP del peer directo.
6. **Pruebas de carga/concurrencia** reproducibles en CI (un smoke de `rendimiento.md`).
7. **Guía de compatibilidad de esquema**: qué versiones de BD abre cada release.

## Futuro (post-1.0)

- **macOS Intel (x64)**: solo si aparece demanda real y se resuelve el problema de runner
  (self-hosted o servicio de terceros).
- **Bundle de servidor para macOS** (+ unidad `launchd` equivalente a `nucleus.service`).
- **Auto-inducción** de subdominios/labels (clustering + reglas, sin LLM) — el diferencial de
  producto que figura en "próximos pasos" del README.
- Honrar `X-Forwarded-For` (opt-in) en el rate limit cuando se está tras un proxy de confianza.

> Nota: el **tag y la publicación de una release** son acciones de cara al exterior; no las haré
> sin tu confirmación explícita.
