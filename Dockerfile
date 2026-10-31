# syntax=docker/dockerfile:1
#
# probe-server image: the Rust binary plus the built client, nothing else.
#   docker build -t rmx-backend-probe .
# Bun is only a build tool here; the runtime image has no Bun, shell or package manager.

# ---- 1. Client: bun build -> /client/dist ---------------------------------
FROM oven/bun:1.4.3 AS client
WORKDIR /client
COPY client/package.json client/bun.lock ./
RUN bun install --frozen-lockfile
COPY client/ ./
RUN bun run build

# ---- 2. Server: cargo-chef caches the dependency build in its own layer -----
# The Rust version must match rust-toolchain.toml.
FROM lukemathwalker/cargo-chef:0.1.78-rust-1.99.1-slim-trixie AS chef
WORKDIR /src
# Use the image's compiler instead of letting rustup download the pinned one again.
ENV RUSTUP_TOOLCHAIN=1.99.1

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS server
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked -p probe-server --recipe-path recipe.json
COPY . .
# Compile-time checked queries use the committed .sqlx data: no database needed.
ENV SQLX_OFFLINE=true
RUN cargo build --release --locked -p probe-server

# ---- 3. Runtime: distroless (glibc + CA certs), runs as uid 65532 ----------
FROM gcr.io/distroless/cc-debian13:nonroot
LABEL org.opencontainers.image.source="https://github.com/rmx-lt/rmx-backend-probe" \
      org.opencontainers.image.description="Blade of Grass backend probe (CRUD over HTTPS, WSS and MQTT)" \
      org.opencontainers.image.licenses="Apache-2.0"
WORKDIR /app
COPY --from=server /src/target/release/probe-server /app/probe-server
COPY --from=client /client/dist /app/static
ENV BIND_ADDR=0.0.0.0:8080 \
    STATIC_DIR=/app/static \
    RUST_LOG=info
USER nonroot
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app/probe-server", "healthcheck"]
ENTRYPOINT ["/app/probe-server"]
