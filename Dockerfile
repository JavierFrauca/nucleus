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

# ---- runtime ----
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libgomp1 curl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/nucleus-server /usr/local/bin/nucleus-server
COPY --from=builder /artifacts/lib/ /usr/local/lib/
RUN ldconfig
# Run as a non-root user; /data holds the DB, model cache and admin token.
RUN useradd --system --create-home --uid 10001 nucleus \
 && mkdir -p /data && chown -R nucleus:nucleus /data
ENV NUCLEUS_ADDR=0.0.0.0:8080 \
    NUCLEUS_DB=/data/nucleus.redb \
    NUCLEUS_MODEL_CACHE=/data/models \
    NUCLEUS_INDEX_DIR=/data/indexes \
    NUCLEUS_ADMIN_TOKEN_FILE=/data/admin_token.txt \
    RUST_LOG=nucleus_server=info,warn
USER nucleus
VOLUME ["/data"]
EXPOSE 8080
# /readyz checks storage reachability (cheap); model download happens lazily.
HEALTHCHECK --interval=30s --timeout=3s --start-period=15s --retries=3 \
  CMD curl -fsS http://localhost:8080/readyz || exit 1
ENTRYPOINT ["nucleus-server"]
