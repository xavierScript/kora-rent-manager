# ---------------------------------------------------
# 1. Builder Stage
# ---------------------------------------------------
FROM rust:1.75-slim-bookworm as builder

WORKDIR /app

RUN apt-get update && apt-get install -y pkg-config libssl-dev

COPY . .

# CHANGE: Build BOTH binaries (kora AND zombie_account_setup)
RUN cargo build --release --bin kora --bin zombie_account_setup

# ---------------------------------------------------
# 2. Runtime Stage
# ---------------------------------------------------
FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the main bot
COPY --from=builder /app/target/release/kora /usr/local/bin/kora

# CHANGE: Copy the setup tool
COPY --from=builder /app/target/release/zombie_account_setup /usr/local/bin/zombie_account_setup

RUN mkdir -p /app/config

# Check main binary
RUN kora --version

ENTRYPOINT ["kora"]