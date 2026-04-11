# ─── Stage 1: Rust builder ────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

WORKDIR /build

# Cache dependency compilation — only reruns when Cargo.toml or Cargo.lock change.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release --locked 2>&1 | tail -5
RUN rm -rf src

# Build the application
COPY src ./src
COPY config.json ./

# Touch main.rs so cargo re-links against the real source
RUN touch src/main.rs
RUN cargo build --release --locked

# ─── Stage 2: Runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/registry-mcp /app/registry-mcp
COPY --from=builder /build/config.json /app/config.json

USER nobody

EXPOSE 3000

HEALTHCHECK --interval=10s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/app/registry-mcp", "healthcheck"]

CMD ["/app/registry-mcp"]
