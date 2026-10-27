# Source cargo environment
source ~/.cargo/env

# Install Rust 1.92 (as specified in rust-toolchain.toml)
rustup install 1.92
rustup default 1.92

# Install nightly toolchain (required for some components)
rustup install nightly-2026-07-15

# Add required components for Rust 1.92
rustup component add rustfmt clippy rust-src --toolchain 1.92

# Add required components for nightly
rustup component add rustfmt clippy rust-src --toolchain nightly-2026-07-15

# Install taplo (TOML formatter, required for `make format`)
cargo install taplo-cli --version 0.9.3 --locked
