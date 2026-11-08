ARG RUST_IMAGE=rust:1.95-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim
FROM ${RUST_IMAGE} AS build
WORKDIR /build
COPY services/file-operations/Cargo.toml services/file-operations/Cargo.lock ./
RUN mkdir -p src/bin \
  && echo "fn main() {}" > src/main.rs \
  && echo "fn main() {}" > src/bin/storage-probe.rs \
  && cargo build --release --locked --bins \
  && rm -rf src target/release/deps/face_file_operations* target/release/deps/storage_probe* target/release/face-file-operations target/release/storage-probe
COPY services/file-operations/src ./src
RUN touch src/main.rs src/bin/storage-probe.rs && cargo build --release --locked --bins

FROM ${RUNTIME_IMAGE} AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg util-linux && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/target/release/face-file-operations /usr/local/bin/face-file-operations
COPY --from=build /build/target/release/storage-probe /usr/local/bin/storage-probe
USER 1000:1000
ENTRYPOINT ["face-file-operations"]

# Optional isolated acceptance image; no test driver is needed in the runtime image.
FROM runtime AS acceptance
USER root
RUN apt-get update && apt-get install -y --no-install-recommends python3 acl && rm -rf /var/lib/apt/lists/*
COPY tests/http-rust-nfs.py /acceptance/http-rust-nfs.py
COPY tests/fixtures/peach-contract-schema.sql /acceptance/peach-contract-schema.sql
USER 1000:1000
