# Build from the repository root:  docker build -f contrib/docker/Dockerfile .
FROM rust:1.98 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
RUN cargo build --release --locked -p restask

FROM debian:bookworm-slim
# tzdata: the daemon converts timed due dates to local wall time and stamps local dates,
# so TZ must resolve inside the container (the slim image ships without zone files).
RUN apt-get update \
    && apt-get install -y --no-install-recommends tzdata \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/restask /usr/local/bin/restask
USER 1000:1000
ENV HOME=/home/restask
ENV RESTASK_VAULT=/vault
CMD ["restask", "daemon"]
