# ---- build stage ----
FROM rust:1.96-slim AS builder
WORKDIR /app

# Cache dependencies separately from source changes.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs \
    && echo "" > src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY migrations ./migrations
COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

# ---- runtime stage ----
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --home /data anonbookrec \
    && mkdir -p /data && chown anonbookrec:anonbookrec /data
WORKDIR /data

COPY --from=builder /app/target/release/anonbookrec /usr/local/bin/anonbookrec
COPY static /srv/static

ENV BIND=0.0.0.0:8080 \
    DATABASE_URL=sqlite:///data/anonbookrec.db \
    PEPPER_FILE=/data/.pepper \
    STATIC_DIR=/srv/static \
    RUST_LOG=info

USER anonbookrec
EXPOSE 8080
CMD ["anonbookrec"]
