# syntax=docker/dockerfile:1
#
# Multi-stage build for massif. The only system dependency is GDAL — everything
# else (SQLite, libwebp) is compiled in statically. Debian bookworm ships GDAL
# 3.6, within the supported 3.5–3.12 range, so the `gdal` crate's prebuilt
# bindings are used (no bindgen needed).
#
#   docker build -t massif .
#   docker run --rm -v "$PWD:/data" massif --compress 6 /data/in.tif /data/out.pmtiles

# ── build ────────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS build

# GDAL headers + a C toolchain (gdal-sys, libwebp-sys, bundled SQLite).
RUN apt-get update && apt-get install -y --no-install-recommends \
        libgdal-dev \
        build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
RUN cargo build --release && strip target/release/massif

# ── runtime ──────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

# GDAL runtime library only (same 3.6 the binary was built against).
RUN apt-get update && apt-get install -y --no-install-recommends \
        libgdal32 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/massif /usr/local/bin/massif

# Bind-mount your rasters here; paths passed to massif are resolved inside /data.
WORKDIR /data
ENTRYPOINT ["massif"]
