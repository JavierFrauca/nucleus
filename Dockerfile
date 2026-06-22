# Multi-stage build for nucleus-server.
#
# Note: fastembed/ort download the ONNX Runtime shared library at build time. We
# copy it into the runtime image and run ldconfig. If you hit a runtime linker
# error for libonnxruntime, set ORT_DYLIB_PATH or adjust the copy path below.

# ---- builder ----
FROM rust:1-bookworm AS builder
WORKDIR /build
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*
COPY . .
ENV CARGO_INCREMENTAL=0
RUN cargo build --release -p nucleus-server
# Collect the ONNX Runtime lib that ort downloaded during the build.
RUN mkdir -p /artifacts/lib \
 && find target -name 'libonnxruntime*.so*' -exec cp -v {} /artifacts/lib/ \; || true

# Pre-download the default embedding model into the image so the first
# ingest/search doesn't pay the ~450 MB download. Set PREFETCH_MODEL=false to
# skip it (e.g. network-restricted builds). The step is tolerant: a failed
# download leaves an empty dir and the server falls back to a lazy download.
ARG PREFETCH_MODEL=true
RUN mkdir -p /opt/nucleus/models \
 && if [ "$PREFETCH_MODEL" = "true" ]; then \
      cargo build --release -p nucleus-core --example prefetch_model \
      && LD_LIBRARY_PATH=/artifacts/lib NUCLEUS_MODEL_CACHE=/opt/nucleus/models \
         target/release/examples/prefetch_model || echo "prefetch skipped/failed; model will download lazily"; \
    fi

# ---- runtime ----
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libgomp1 curl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/nucleus-server /usr/local/bin/nucleus-server
COPY --from=builder /artifacts/lib/ /usr/local/lib/
# The model baked at build time (may be empty if PREFETCH_MODEL=false).
COPY --from=builder /opt/nucleus/models /opt/nucleus/models
RUN ldconfig
# Run as a non-root user; /data holds the DB, indexes and admin token. The model
# cache lives in the image (/opt/nucleus/models) so first run needs no download;
# other models requested at runtime download there (writable by the user).
RUN useradd --system --create-home --uid 10001 nucleus \
 && mkdir -p /data && chown -R nucleus:nucleus /data \
 && chown -R nucleus:nucleus /opt/nucleus
ENV NUCLEUS_ADDR=0.0.0.0:8080 \
    NUCLEUS_DB=/data/nucleus.redb \
    NUCLEUS_MODEL_CACHE=/opt/nucleus/models \
    NUCLEUS_INDEX_DIR=/data/indexes \
    NUCLEUS_ADMIN_TOKEN_FILE=/data/admin_token.txt \
    RUST_LOG=nucleus_server=info,warn
USER nucleus
VOLUME ["/data"]
EXPOSE 8080
# /readyz checks storage reachability (cheap); the model is baked into the image.
HEALTHCHECK --interval=30s --timeout=3s --start-period=15s --retries=3 \
  CMD curl -fsS http://localhost:8080/readyz || exit 1
ENTRYPOINT ["nucleus-server"]
