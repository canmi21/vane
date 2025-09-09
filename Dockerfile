FROM rustlang/rust:nightly-slim AS builder

ARG TARGETARCH
WORKDIR /app
COPY . .

RUN unset HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy \
  && apt-get update && apt-get install -y musl-tools pkg-config libssl-dev \
  && case "$TARGETARCH" in \
       "amd64") rustup target add x86_64-unknown-linux-musl \
                && cargo build --release --target x86_64-unknown-linux-musl \
                && cp target/x86_64-unknown-linux-musl/release/jellyfish /app/jellyfish ;; \
       "arm64") rustup target add aarch64-unknown-linux-musl \
                && cargo build --release --target aarch64-unknown-linux-musl \
                && cp target/aarch64-unknown-linux-musl/release/jellyfish /app/jellyfish ;; \
     esac

FROM scratch
WORKDIR /app
COPY --from=builder /app/jellyfish ./jellyfish

CMD ["./jellyfish"]