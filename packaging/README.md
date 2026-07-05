# Nucleus — packaging & installation

Ways to deploy the Nucleus **server** (HTTP mode) so other projects can consume it over
the network. For **embedding** the engine directly in an app (the product's focus —
`nucleus.dll` / `libnucleus.so` / `libnucleus.dylib`, no HTTP), see
[Modo embebido en el README](../README.md#modo-embebido-dll) and `build-dll.ps1` /
`build-lib.sh` below instead of the options in this file.

> **Platform coverage differs by mode.** The **embedded** bundles (`build-dll.ps1` /
> `build-lib.sh`) ship for Windows x64, Linux x64 and macOS arm64. The **server**
> bundles below (Options B/C) currently only exist for **Windows and Linux** — there is
> no macOS server bundle / `launchd` unit yet (see
> [camino a la 1.0](../docs/camino-a-1.0.md)).

> **ONNX Runtime:** the server runs embeddings in-process via ONNX Runtime. The
> native library (`onnxruntime.dll` on Windows, `libonnxruntime.so` on Linux) is
> fetched at build time and **must ship next to the binary** (the bundles below
> do this). The first search/ingest also downloads the embedding model (~450 MB)
> into the model-cache directory.

## Option A — Docker (recommended)

```bash
docker compose up -d --build
docker compose exec nucleus cat /data/admin_token.txt   # bootstrap admin token (shown once)
curl -fsS http://localhost:8080/readyz
```

Data (DB, model cache, indexes, token) persists in the `nucleus-data` volume.
Tuning via environment in [`docker-compose.yml`](../docker-compose.yml).

## Option B — Windows bundle

Build a self-contained zip (`nucleus-server.exe` + ONNX Runtime DLLs + installer):

```powershell
pwsh packaging/build-release.ps1 -Version 0.1.0          # -> dist/nucleus-0.1.0-windows-x64.zip
# add -Gpu to bundle the DirectML build
```

Install on the target machine (unzip, then from the unzipped folder, as Admin):

```powershell
pwsh install.ps1 -RegisterService          # copies files, sets env, registers a startup task
# or, no service:
pwsh install.ps1
```

`install.ps1` installs to `%ProgramFiles%\Nucleus`, stores data in
`%ProgramData%\Nucleus`, sets machine env vars, and (with `-RegisterService`)
registers a Task Scheduler job that starts Nucleus at boot. The admin token is
written to `%ProgramData%\Nucleus\admin_token.txt`.

## Option C — Linux bundle + systemd

```bash
packaging/build-release.sh 0.1.0           # -> dist/nucleus-0.1.0-linux-x64.tar.gz
```

On the target host:

```bash
tar xzf nucleus-0.1.0-linux-x64.tar.gz && cd nucleus-0.1.0-linux-x64
sudo useradd --system --home /var/lib/nucleus --create-home nucleus
sudo mkdir -p /opt/nucleus && sudo cp nucleus-server libonnxruntime*.so* /opt/nucleus/
sudo cp nucleus.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now nucleus
sudo cat /var/lib/nucleus/admin_token.txt   # bootstrap admin token (once)
```

The unit sets `LD_LIBRARY_PATH=/opt/nucleus` so ONNX Runtime is found next to the
binary. Adjust paths/port via the `Environment=` lines in
[`nucleus.service`](nucleus.service).

## Option D — embedded mode (DLL / so / dylib)

For in-process embedding (no HTTP, no server binary), build the shared library
directly instead of the server bundles above:

```powershell
pwsh packaging/build-dll.ps1 -Version 0.1.0        # -> dist/nucleus-dll-0.1.0-windows-x64.zip
```

```bash
packaging/build-lib.sh 0.1.0                       # -> dist/nucleus-lib-0.1.0-<os>-<arch>.tar.gz
                                                    #    (linux-x64 or macos-arm64)
```

Both bundle `nucleus.h`, the C# P/Invoke binding, and a README; the Unix one also
bundles the ONNX Runtime shared library if it wasn't linked statically. See
[Modo embebido en el README](../README.md#modo-embebido-dll) for the API and usage.

## Configuration

All options are environment variables — see
[docs/configuracion.md](../docs/configuracion.md). The API contract for client
SDKs is [docs/openapi.yaml](../docs/openapi.yaml); ready-made clients live in
[`clients/`](../clients).
