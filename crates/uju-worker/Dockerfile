FROM rust:1.97.1-trixie AS chef

RUN cargo install cargo-chef
WORKDIR /build


FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates/uju crates/uju
COPY crates/uju-worker crates/uju-worker
RUN cargo chef prepare --recipe-path recipe.json


FROM chef AS builder

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY crates/uju crates/uju
COPY crates/uju-worker crates/uju-worker
RUN cargo build --release --bin uju-worker


FROM debian:trixie-slim AS runtime

RUN groupadd server && \
    useradd -m -g server server

WORKDIR /app
COPY --from=builder /build/target/release/uju-worker /app/
RUN chown -R server:server /app

USER server
CMD ["./uju-worker"]
