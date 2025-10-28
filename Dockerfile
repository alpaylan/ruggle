FROM rust:latest AS chef
RUN cargo install cargo-chef
WORKDIR /usr/src/ruggle

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /usr/src/ruggle/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

FROM debian:buster-slim AS runtime
WORKDIR /usr/src/ruggle
COPY --from=builder /usr/src/ruggle/target/release/ruggle /usr/local/bin
COPY --from=builder /usr/src/ruggle/ruggle-index ruggle-index

ARG ROCKET_ADDRESS=0.0.0.0
ENV ROCKET_ADDRESS=${ROCKET_ADDRESS}

CMD ["/usr/local/bin/ruggle"]
