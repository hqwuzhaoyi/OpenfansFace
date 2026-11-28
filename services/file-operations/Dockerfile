# syntax=docker/dockerfile:1
ARG RUST_IMAGE=rust:1.95-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim
FROM ${RUST_IMAGE} AS build
WORKDIR /build
COPY services/file-operations/Cargo.toml services/file-operations/Cargo.lock ./
COPY services/file-operations/src ./src
RUN --mount=type=cache,id=face-file-ops-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=face-file-ops-target,target=/build/target,sharing=locked \
    CARGO_INCREMENTAL=1 cargo build --release --locked --bins \
    && mkdir -p /out \
    && cp /build/target/release/face-file-operations /out/face-file-operations \
    && cp /build/target/release/storage-probe /out/storage-probe

FROM ${RUNTIME_IMAGE} AS runtime
RUN sed -i 's|deb.debian.org|mirrors.aliyun.com|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends ffmpeg util-linux \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/face-file-operations /usr/local/bin/face-file-operations
COPY --from=build /out/storage-probe /usr/local/bin/storage-probe
USER 1000:1000
ENTRYPOINT ["face-file-operations"]

# Optional isolated acceptance image; no test driver is needed in the runtime image.
FROM runtime AS acceptance
USER root
RUN apt-get update && apt-get install -y --no-install-recommends python3 acl && rm -rf /var/lib/apt/lists/*
COPY tests/http-rust-nfs.py /acceptance/http-rust-nfs.py
COPY tests/fixtures/peach-contract-schema.sql /acceptance/peach-contract-schema.sql
USER 1000:1000
