FROM rust:1.98 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock* rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release -p restask

FROM debian:bookworm-slim
COPY --from=build /src/target/release/restask /usr/local/bin/restask
USER 1000:1000
ENV HOME=/home/restask
ENV RESTASK_VAULT=/vault
CMD ["restask", "daemon"]
