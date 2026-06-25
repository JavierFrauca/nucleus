# Camino a la 1.0

Checklist priorizada para que Nucleus pueda anunciarse como **estable / production-ready**
sin asteriscos. Marca lo hecho y lo que falta; al final, las decisiones que dependen de ti.

## Estado actual (v0.1.2)

- ✅ **90 tests** en verde (motor, integración del motor, **C-ABI del FFI**, e2e HTTP) + clippy `-D warnings` + `cargo fmt --check`.
- ✅ Hardening del servidor: auth por token con scopes, dedup por hash, apagado ordenado con volcado de índices, versionado de esquema con gate de migración, `/healthz` + `/readyz` + `/metrics`, CORS opt-in, listados paginados, **rate limiting por IP** (token-bucket, `NUCLEUS_RATE_LIMIT_RPM`), Docker.
- ✅ **CI multiplataforma**: lint en Linux + tests en Linux/Windows/macOS.
- ✅ Modo embebido (DLL) con su binding C#; modo servidor (HTTP) con axum.

## Alcance decidido (2026-06)

- **Plataforma de la 1.0: solo Windows x64.** Linux/macOS (`.so`/`.dylib`) quedan para versiones
  futuras. La documentación se ha alineado a este alcance.
- **Versionado: seguimos en `0.x`** por ahora (sin congelar API). El bump a `1.0.0` se valorará más
  adelante; mientras tanto `0.x` permite iterar la API sin coste SemVer.

## Bloqueantes para 1.0 (must)

1. **Verificar la matriz de CI en verde** (Linux + Windows). El workflow aún no se ha ejecutado en
   GitHub; confirmar que `ort` descarga ONNX Runtime en el runner de Windows.
2. **Pulir el bundle de Windows**: que `packaging/build-dll.ps1` y el `.zip` de release sigan
   reproducibles, con `nucleus.h`, import lib, binding C# y README actualizado.

## Recomendado antes de presumir de "production" (should)

4. **Tests HTTP de autorización**: cubrir scopes `Read`/`Write`/`Admin` por endpoint (hoy el e2e
   valida el camino feliz y el 401, pero no la matriz de permisos).
5. **Rate limit tras proxy**: honrar `X-Forwarded-For` de forma opt-in (cuando se confía en el
   proxy), porque hoy se limita por IP del peer directo.
6. **Pruebas de carga/concurrencia** reproducibles en CI (un smoke de `rendimiento.md`).
7. **Guía de compatibilidad de esquema**: qué versiones de BD abre cada release.

## Futuro (post-1.0)

- **Binarios multiplataforma** (Linux/macOS): ampliar `packaging/` y un workflow de release.
- **Auto-inducción** de subdominios/labels (clustering + reglas, sin LLM) — el diferencial de
  producto que figura en "próximos pasos" del README.
- Honrar `X-Forwarded-For` (opt-in) en el rate limit cuando se está tras un proxy de confianza.

> Nota: el **tag y la publicación de una release** son acciones de cara al exterior; no las haré
> sin tu confirmación explícita.
