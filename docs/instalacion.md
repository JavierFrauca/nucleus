# Instalación

Nucleus es un workspace de Rust con dos crates: `nucleus-core` (la librería del motor)
y `nucleus-server` (el binario del servidor HTTP). No necesita servicios externos: el
almacenamiento (redb) y los embeddings (fastembed/ONNX Runtime) van **dentro** del
binario.

## 1. Requisitos

| Requisito | Detalle |
|-----------|---------|
| **Rust** (toolchain) | Estable, host **MSVC** en Windows. |
| **Compilador C/C++** | En Windows, los **VS C++ Build Tools** (linker `link.exe`). `ort`/ONNX Runtime y otras dependencias compilan C/C++. |
| **Red (1ª vez)** | La primera ingesta/búsqueda descarga el modelo de embeddings desde HuggingFace (~450 MB para el multilingüe por defecto). |
| **Disco** | El `target/` de depuración es grande (grafo de dependencias con ONNX, tokenizers, códecs). Ver "Notas de disco". |
| **RAM** | Suficiente para el modelo + el índice en memoria. Para corpus grandes, ajustar workers (ver [operación](operacion.md)). |

## 2. Instalar el toolchain

### Windows (recomendado: MSVC)

```powershell
# Rust (rustup) y VS C++ Build Tools
winget install Rustlang.Rustup
winget install Microsoft.VisualStudio.2022.BuildTools `
  --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Tras instalar, abre una terminal nueva y comprueba:

```powershell
cargo --version
rustc --version
```

> Si `cargo` no está en el PATH, está en `%USERPROFILE%\.cargo\bin`. Añádelo:
> `$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"`.

### Linux / macOS

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Linux: instala build-essential / clang y pkg-config según tu distro.
```

## 3. Compilar

```bash
cargo build --release            # binario optimizado en target/release/nucleus-server
# o, para desarrollo:
cargo build
```

Verificación de la build (tests + linter):

```bash
cargo test --workspace           # 106 tests (core + integración del motor + C-ABI del FFI + e2e HTTP)
cargo clippy --workspace --all-targets
```

### Build con GPU (opcional)

```bash
cargo build --release --features gpu
```

Compila el execution provider **DirectML** de ONNX Runtime (Windows) con *fallback*
automático a CPU. Requiere GPU/driver compatibles en tiempo de ejecución. Sin esta
feature, la build es solo CPU. Se activa además en runtime con `NUCLEUS_GPU=true`
(ver [configuración](configuracion.md)).

## 4. Ejecutar

```bash
cargo run --release -p nucleus-server
# o directamente el binario:
./target/release/nucleus-server      # Windows: target\release\nucleus-server.exe
```

Al primer arranque (base de datos vacía) imprime **una sola vez** un token de
administración. Guárdalo:

```
========================================================
 Nucleus bootstrap admin token (store it — shown once):
   nuc_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
========================================================
```

Sigue con la [guía rápida](guia-rapida.md).

## Notas de disco

- El `target/` de depuración puede superar varios GB. El `Cargo.toml` ya fija
  `[profile.dev] debug = 0` para reducirlo; puedes reactivar la info de depuración
  localmente si necesitas un debugger.
- Para compilaciones puntuales sin caché incremental: `CARGO_INCREMENTAL=0`.
- `cargo clean` libera todo el `target/`.

## Solución de problemas

- **`link.exe` no encontrado / error de enlazado en Windows**: faltan los VS C++ Build
  Tools (workload "Desktop development with C++"/VCTools). Reinstálalos.
- **Falla la descarga del modelo**: la primera ingesta necesita red. Define
  `NUCLEUS_MODEL_CACHE` a un directorio persistente para no volver a descargar.
- **Build con `--features gpu` pesado/lento**: descarga un binario de ONNX Runtime con
  DirectML; necesita espacio y red. Si no usas GPU, no actives la feature.
