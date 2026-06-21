# Nucleus — packaging & installation

Ways to deploy the Nucleus server so other projects can consume it over HTTP.
For embedding the engine directly in a Rust project, use the
[`nucleus-core`](../crates/core/README.md) crate instead.

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

## Configuration

All options are environment variables — see
[docs/configuracion.md](../docs/configuracion.md). The API contract for client
SDKs is [docs/openapi.yaml](../docs/openapi.yaml); ready-made clients live in
[`clients/`](../clients).
