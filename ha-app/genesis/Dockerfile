# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM rust:1.98.1-bookworm AS builder

RUN rustup target add aarch64-unknown-linux-gnu \
    && apt-get update \
    && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*

ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc

WORKDIR /source
COPY core/ ./core/
RUN cd core && cargo build --release --target aarch64-unknown-linux-gnu

FROM --platform=$TARGETPLATFORM debian:bookworm-slim

ARG BUILD_VERSION=0.1.0
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates jq \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /source/core/target/aarch64-unknown-linux-gnu/release/genesis-core /usr/local/bin/genesis-core
COPY ha-app/genesis/run.sh /run.sh
RUN chmod 0755 /run.sh

LABEL io.hass.version="${BUILD_VERSION}" \
      io.hass.type="app" \
      io.hass.arch="aarch64"

EXPOSE 8080
ENTRYPOINT ["/run.sh"]
