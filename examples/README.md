# Ejemplos de uso

Proyectos mínimos para aprender. Hay **dos sabores** según el modo de Nucleus:

- **Embebido (DLL)** — el motor corre *dentro* de tu proceso vía `nucleus.dll`, sin red ni
  servidor. No necesitan token ni servidor arrancado; sí necesitan `nucleus.dll` compilada
  (`cargo build -p nucleus-ffi --release`, que los `.csproj` copian junto al ejecutable).
- **Cliente-servidor (HTTP)** — usan el SDK contra un `nucleus-server` en marcha + un **token**.

| Ejemplo | Lenguaje | Modo | Qué muestra |
|---|---|---|---|
| [`csharp/NucleusBlazor`](csharp/NucleusBlazor) | C# / Blazor | **Embebido** | Web app Blazor Server con UI de **ingesta (texto o fichero pdf/docx/…) + búsqueda**; el motor vive en el propio proceso .NET (P/Invoke a `nucleus.dll`). |
| [`ffi-smoke`](ffi-smoke) | C# / consola | **Embebido** | Smoke test end-to-end del binding nativo `Nucleus.Native` (open → ingestar → buscar → editar → reindex). |
| [`csharp/NucleusDemo`](csharp/NucleusDemo) | C# / .NET | HTTP | Consola con **menú** (crear dominio, ingestar texto, buscar, listar, backup, **subir fichero crudo**). |
| [`javascript/node`](javascript/node) | Node (JS) | HTTP | `demo.mjs`: flujo **headless** (crear → ingestar → esperar job → buscar). `upload.mjs`: **subida de fichero crudo** (PDF…). |
| [`javascript/browser`](javascript/browser) | Navegador | HTTP | Mini-UI con **2 pantallas** (Ingesta —texto o **fichero**— y Búsqueda) usando el SDK por ESM. |

Los ejemplos **HTTP** requieren un servidor arrancado y un token (de admin para crear
dominios/backups). Ver [guía rápida](../docs/guia-rapida.md):

```bash
# en otra terminal: arranca el servidor y copia el token admin que imprime
nucleus-server          # token en NUCLEUS_ADMIN_TOKEN_FILE
```

**Subida de fichero crudo** (los bytes; el motor extrae el texto): opción 6 del menú en C#,
`node upload.mjs <ruta.pdf>` en Node, y el selector de fichero en la pantalla de Ingesta del
navegador.

Todos leen la URL y el token de variables de entorno:
`NUCLEUS_URL` (def. `http://127.0.0.1:8080`) y `NUCLEUS_TOKEN`.

## Arrancar cada uno

```bash
# Blazor embebido (sin servidor ni token; primero compila la DLL)
cargo build -p nucleus-ffi --release
cd examples/csharp/NucleusBlazor
dotnet run                         # abre http://localhost:5099

# C# (HTTP)
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
