# Roadmap

Este es el roadmap público de Nucleus. No es un compromiso de fechas, sino una
declaración de intenciones y prioridades. Las casillas reflejan el estado en el
momento de escribir; los issues concretos viven en el tracker.

> ¿Echas algo en falta? Abre un [issue](https://github.com/JavierFrauca/nucleus/issues)
> con label `enhancement`.

## Estado actual (1.x)

- ✅ Motor **todo-en-uno**: extracción multi-formato + chunking + embeddings
  in-process + indexación + recuperación.
- ✅ **Dos modos**: embebido (DLL/lib, foco) y servidor HTTP (axum).
- ✅ Búsqueda **híbrida** (vector + BM25 fusionados con RRF) + **reranking**
  opcional (cross-encoder) + **diversidad** (MMR).
- ✅ **Cifrado en reposo** siempre activo (XChaCha20-Poly1305 + HMAC en claves).
- ✅ **Backups** full + diferencial (bsdiff), hot-swap en restore.
- ✅ **Autenticación** por token con scopes por dominio (`Read`/`Write`/`Admin`).
- ✅ **Lenguaje de consulta** (`tag:`, `doc:`, `meta.*`, AND/OR/NOT).
- ✅ Clientes **C# (.NET)** y **TypeScript/JavaScript**; SDK Rust directo.
- ✅ Suite de **benchmarks** con Criterion.

## Corto plazo (próximas versiones minor)

Objetivo: cerrar huecos funcionales sin romper la API estable.

- 🔲 **Auto-inducción de subdominios y labels** (sin LLM): clustering de
  embeddings con centrado para sugerir subdominios; reglas + zero-shot para
  labels. Capa opcional sobre la clasificación actual (que aporta quien ingesta).
- 🔲 **Re-ranking por defecto más accesible**: tunear la cota de candidatos y
  documentar el trade-off latencia/calidad.
- 🔲 **`mmap` del grafo HNSW** para que la carga de índices grandes no pague
  todo el RSS al arranque.
- 🔲 **Más modelos** de embeddings detrás del trait (modelos multilingües más
  grandes, modelos de dominio).
- 🔲 **Clientes en más lenguajes**: Python, Go (sobre la cdylib o HTTP).

## Medio plazo (próximo major)

Objetivo: operabilidad y escala para producción seria.

- 🔲 **Recuperación distribuida**: workers de ingesta separados del servidor;
  shard por dominio.
- 🔲 **Observabilidad**: métricas Prometheus estructuradas (hoy es texto),
  trazas OTel, healthchecks más ricos.
- 🔲 **Streaming de ingesta**: ingesta por lotes grande con backpressure y
  checkpointing, para cargar corpus de millones de documentos.
- 🔲 **Reindexado online** sin degradar búsquedas (hoy es bloqueante por dominio).
- 🔲 **Políticas de retención** de backups (hoy es manual).

## Largo plazo (visión)

- 🔲 **Empaquetado cloud**: Helm chart / Operator para Kubernetes; bundle de
  servidor como contenedor ligero.
- 🔲 **Capa de auto-inducción con LLM opcional**: cuando el despliegue lo
  permita, usar un LLM para enriquecer etiquetas y resúmenes — siempre como
  capa opt-in, manteniendo el modo offline por defecto.
- 🔲 **Multi-tenancy** con aislamiento más fuerte (hoy los dominios son
  namespaces lógicos; no hay cuotas ni aislamiento de recursos por tenant).

## Cómo influir

El roadmap se reordena según el feedback real. Lo más útil que puedes hacer:

1. **Abrir issues** con casos de uso concretos (nos dice qué priorizar).
2. **Reportar cuellos de botella** con datos (benchmark + perfil).
3. **Proponer contribuciones** en las áreas de corto plazo, donde hay más
   acuerdo.

Ver también el [camino a 1.0](docs/camino-a-1.0.md) para el compromiso de
estabilidad que rige los cambios de versión.
