#!/usr/bin/env bash
set -euo pipefail

# Administrator step; on a shared host ask its administrator to install these.
sudo apt update
sudo apt install -y \
    build-essential \
    libssl-dev \
    libpcap-dev \
    pkg-config \
    clang \
    cmake \
    protobuf-compiler \
    git \
    curl \
    ca-certificates \
    python3 \
    time \
    util-linux \
    iproute2 \
    zstd \
    openssl \
    tar \
    shellcheck

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
