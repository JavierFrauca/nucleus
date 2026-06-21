# Ejemplos de uso

Proyectos mínimos que muestran **cómo referenciar el SDK, inicializar el cliente y llamar a
cada operación** contra un servidor Nucleus en marcha. No son producción: son para aprender.

Requisitos previos: un servidor Nucleus arrancado y un **token** (de admin para crear
dominios/backups). Ver [guía rápida](../docs/guia-rapida.md).

```bash
# en otra terminal: arranca el servidor y copia el token admin que imprime
nucleus-server          # token en NUCLEUS_ADMIN_TOKEN_FILE
```

| Ejemplo | Lenguaje | Qué muestra |
|---|---|---|
| [`csharp/NucleusDemo`](csharp/NucleusDemo) | C# / .NET | Consola con **menú** (crear dominio, ingestar texto, buscar, listar, backup, **subir fichero crudo**). |
| [`javascript/node`](javascript/node) | Node (JS) | `demo.mjs`: flujo **headless** (crear → ingestar → esperar job → buscar). `upload.mjs`: **subida de fichero crudo** (PDF…). |
| [`javascript/browser`](javascript/browser) | Navegador | Mini-UI con **2 pantallas** (Ingesta —texto o **fichero**— y Búsqueda) usando el SDK por ESM. |

**Subida de fichero crudo** (los bytes; el motor extrae el texto): opción 6 del menú en C#,
`node upload.mjs <ruta.pdf>` en Node, y el selector de fichero en la pantalla de Ingesta del
navegador.

Todos leen la URL y el token de variables de entorno:
`NUCLEUS_URL` (def. `http://127.0.0.1:8080`) y `NUCLEUS_TOKEN`.

## Arrancar cada uno

```bash
# C#
cd csharp/NucleusDemo
$env:NUCLEUS_TOKEN="nuc_…"        # PowerShell (export NUCLEUS_TOKEN=… en bash)
dotnet run

# Node
cd javascript/node
npm install                        # resuelve el SDK local (file:)
$env:NUCLEUS_TOKEN="nuc_…"
node demo.mjs

# Navegador (necesita CORS: arranca el server con NUCLEUS_CORS_ANY=true)
cd javascript/browser
npx serve .                        # o cualquier servidor estático; abre la URL que indique
```
