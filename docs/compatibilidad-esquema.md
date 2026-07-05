# Compatibilidad de esquema entre versiones

Qué versión de Nucleus puede abrir qué base de datos, y qué pasa cuando no coinciden.
El número relevante es el **`SCHEMA_VERSION`** interno (no la versión de release), guardado
en la tabla `meta` de cada fichero `.redb`.

## Tabla de compatibilidad

| Versión de Nucleus | `SCHEMA_VERSION` | Cifrado | Notas |
|---------------------|:---:|---------|-------|
| v0.1.0 – v0.1.2      | 1   | Sin cifrar | Índices secundarios (tags, subdominios, hashes de contenido) en claro. |
| v0.2.0+              | 2   | **Siempre activo** | Índices secundarios ofuscados con HMAC con clave (ver [operación](operacion.md#cifrado-en-reposo)). |

## Qué migra solo

- **Abrir una BD v1 (de v0.1.x) con Nucleus v0.2.0+**: se **migra automáticamente** la
  primera vez que se abre. El motor reescribe el fichero completo cifrado y con los
  índices con hash con clave (`migrate_legacy_if_needed` en
  [`storage/mod.rs`](../crates/core/src/storage/mod.rs)); no hace falta ninguna acción
  manual. Es un proceso atómico (fichero temporal + rename final), así que un fallo a
  mitad de migración no corrompe ni pierde la BD original.
- Tras la migración, el fichero queda en `SCHEMA_VERSION = 2` y ya no se puede volver a
  abrir con un binario v0.1.x (ver siguiente punto).

## Qué NO migra en caliente

- **Abrir una BD `SCHEMA_VERSION` más nueva que la que soporta el binario**: rechazado
  explícitamente con error (`database schema vN is newer than supported vM; upgrade
  Nucleus`). Nunca se intenta leer una BD de un futuro que el binario no entiende.
- **Una BD *cifrada* con un `SCHEMA_VERSION` inferior al soportado**: hoy esto no puede
  ocurrir en un release oficial (el cifrado se introdujo en el mismo cambio que
  `SCHEMA_VERSION = 2`), pero el motor lo rechaza igualmente en vez de intentar
  adivinar cómo reescribirla: `encrypted database schema vN cannot be upgraded in
  place to v2; recreate the database (e.g. via an encrypted backup) to keyed-hash its
  index keys`. Es una salvaguarda a propósito — **no hay un migrador genérico
  multi-paso**; cada salto de `SCHEMA_VERSION` futuro (v2 → v3, etc.) necesitará su
  propia función de migración explícita en `storage::migrate`, igual que se hizo para
  v1 → v2.

## Qué hacer antes de actualizar Nucleus

- Si vienes de v0.1.x: la migración a v2 es automática, pero al reescribir todo el
  fichero conviene tener un backup reciente por si el proceso se interrumpe por falta
  de disco u otra causa externa (aunque el diseño es atómico). Ver
  [backups y restauración](operacion.md#backups-y-restauración).
- Si en el futuro aparece un `SCHEMA_VERSION = 3` (u otro salto), este documento y el
  changelog de la release lo señalarán explícitamente junto con si migra sola o exige
  recrear desde backup, siguiendo el mismo criterio que v1 → v2.
- No hay forma de "bajar" el esquema (abrir una BD v2 con un binario v1): si necesitas
  volver a una versión antigua de Nucleus, hazlo desde un backup tomado antes de
  actualizar.
