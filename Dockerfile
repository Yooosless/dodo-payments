FROM rust:latest AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    gcc \
    && rm -rf /var/lib/apt/lists/*

COPY . .

RUN cargo build --release

RUN cargo install sqlx-cli \
    --no-default-features \
    --features postgres

FROM rust:latest

WORKDIR /app

RUN apt-get update && apt-get install -y \
    libssl-dev \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/dodo-payments /app/dodo-payments
COPY --from=builder /app/migrations /app/migrations
COPY --from=builder /usr/local/cargo/bin/sqlx /usr/local/bin/sqlx

EXPOSE 3000

CMD ["/app/dodo-payments"]