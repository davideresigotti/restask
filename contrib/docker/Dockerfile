# syntax=docker/dockerfile:1
# Build from the repository root:  docker build -f contrib/docker/Dockerfile .
# The tag is the toolchain pin of rust-toolchain.toml; that file is left out of the
# build so the image's own toolchain is used as it is.
FROM rust:1.98.1 AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# The cache mounts keep the registry and the build directory between builds, so an
# update compiles what changed and not every dependency again.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p restask \
    && cp target/release/restask /usr/local/bin/restask

# The same Debian release as the build image: the binary links against its glibc.
FROM debian:trixie-slim
# Lets `contrib/update.sh` remove the images an update replaced, and no others.
LABEL org.opencontainers.image.title=restask
# tzdata: the daemon converts timed due dates to local wall time and stamps local dates,
# so TZ must resolve inside the container (the slim image ships without zone files).
RUN apt-get update \
    && apt-get install -y --no-install-recommends tzdata \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/restask /usr/local/bin/restask
USER 1000:1000
ENV HOME=/home/restask
ENV RESTASK_VAULT=/vault
CMD ["restask", "daemon"]
