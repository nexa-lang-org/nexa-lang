# ── Stage 1 : build ──────────────────────────────────────────────────────────
FROM rust:slim AS builder

WORKDIR /app

RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*

# Copy manifests first — lets Docker cache the dep-compilation layer
COPY Cargo.toml Cargo.lock ./
COPY crates/compiler/Cargo.toml  crates/compiler/Cargo.toml
COPY crates/server/Cargo.toml    crates/server/Cargo.toml
COPY crates/cli/Cargo.toml       crates/cli/Cargo.toml
COPY crates/registry/Cargo.toml  crates/registry/Cargo.toml

# Stub sources so cargo can resolve and compile all dependencies.
# registry has BOTH lib.rs and main.rs — both must be stubbed or the
# stub build fails and leaves corrupted artifacts that break the real build.
RUN mkdir -p \
      crates/compiler/src \
      crates/server/src \
      crates/cli/src \
      crates/registry/src \
 && echo 'pub fn main(){}' > crates/compiler/src/lib.rs \
 && echo 'pub fn main(){}' > crates/server/src/lib.rs \
 && echo 'fn main(){}' > crates/cli/src/main.rs \
 && echo '' > crates/registry/src/lib.rs \
 && echo 'fn main(){}' > crates/registry/src/main.rs

RUN cargo build --release -p nexa-registry 2>/dev/null || true
RUN rm -rf crates/*/src

# Real build — touch ALL .rs files so Cargo does a full recompile of the
# registry crate (incremental from a stub lib can cause linker confusion).
COPY crates/ crates/
RUN find crates/registry/src -name "*.rs" | xargs touch \
 && cargo build --release -p nexa-registry

# ── Stage 2 : runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 wget \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/nexa-registry /usr/local/bin/nexa-registry
COPY --from=builder /app/crates/registry/migrations   /migrations

RUN useradd --system --no-create-home --shell /bin/false nexa
USER nexa

EXPOSE 4000

ENV DATABASE_URL="" \
    JWT_SECRET="" \
    PORT="4000"

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- http://localhost:4000/health 2>/dev/null || exit 1

CMD ["nexa-registry"]
