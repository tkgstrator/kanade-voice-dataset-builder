FROM ubuntu:24.04

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl ffmpeg git pkg-config \
    && rm -rf /var/lib/apt/lists/*

USER ubuntu
ENV HOME=/home/ubuntu \
    CARGO_HOME=/home/ubuntu/.cargo \
    RUSTUP_HOME=/home/ubuntu/.rustup \
    PATH=/home/ubuntu/.cargo/bin:${PATH}
RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain 1.98.1 --profile minimal \
      --component rustfmt --component clippy --component rust-src
WORKDIR /workspace
