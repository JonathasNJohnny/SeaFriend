FROM rust:1-bookworm AS builder

WORKDIR /app

COPY Cargo.toml ./
COPY Cargo.lock ./
COPY src ./src

RUN cargo build --release -j 2

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/seafriend-api /usr/local/bin/seafriend-api

RUN useradd --system --no-create-home --shell /usr/sbin/nologin seafriend
USER seafriend

EXPOSE 3000

CMD ["seafriend-api"]
