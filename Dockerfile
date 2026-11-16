# The runtime contains the compiled Rust service and native media tools.
FROM rust:1.100-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src/ ./src/
COPY resources/ ./resources/
RUN cargo build --locked --release --all-features --bin telegram-mcp

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg libgcc-s1 libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 telegram \
    && useradd --uid 10001 --gid telegram --create-home telegram \
    && mkdir -p /data/state/telegram-mcp \
    && chown -R telegram:telegram /data \
    && chmod 700 /data /data/state /data/state/telegram-mcp
COPY --from=builder /build/target/release/telegram-mcp /usr/local/bin/telegram-mcp
USER 10001:10001
WORKDIR /data
ENV XDG_STATE_HOME=/data/state
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/telegram-mcp"]
CMD ["serve"]
