FROM rustlang/rust:nightly-slim AS builder

ARG TARGETARCH
ARG UPX_VERSION=5.0.2
ARG PROXY_URL="http://host.docker.internal:7890"

WORKDIR /app

# Install Dependencies & Tools
RUN if [ -n "$PROXY_URL" ]; then export http_proxy="$PROXY_URL" https_proxy="$PROXY_URL" all_proxy="socks5://${PROXY_URL#*//}"; fi \
    && apt-get update && apt-get install -y musl-tools pkg-config libssl-dev curl xz-utils cmake clang git \
    && rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

# Install UPX
RUN if [ -n "$PROXY_URL" ]; then export http_proxy="$PROXY_URL" https_proxy="$PROXY_URL" all_proxy="socks5://${PROXY_URL#*//}"; fi \
    && case "$TARGETARCH" in \
        "amd64") UPX_ARCH="amd64" ;; \
        "arm64") UPX_ARCH="arm64" ;; \
        *) echo "Unsupported architecture: $TARGETARCH" && exit 1 ;; \
    esac \
    && curl -Lo upx.tar.xz "https://github.com/upx/upx/releases/download/v${UPX_VERSION}/upx-${UPX_VERSION}-${UPX_ARCH}_linux.tar.xz" \
    && tar -xf upx.tar.xz \
    && mv upx-${UPX_VERSION}-${UPX_ARCH}_linux/upx /usr/local/bin/ \
    && rm -rf upx.tar.xz upx-${UPX_VERSION}-${UPX_ARCH}_linux

# Cache Rust Dependencies
COPY Cargo.toml Cargo.lock ./
COPY src/core/Cargo.toml src/core/Cargo.toml
COPY src/primitives/Cargo.toml src/primitives/Cargo.toml
COPY src/engine/Cargo.toml src/engine/Cargo.toml
COPY src/app/Cargo.toml src/app/Cargo.toml
COPY src/transport/Cargo.toml src/transport/Cargo.toml
COPY src/extra/Cargo.toml src/extra/Cargo.toml
COPY src/api/Cargo.toml src/api/Cargo.toml
RUN mkdir -p src/core/src && echo "fn main() {}" > src/core/src/main.rs \
    && for d in primitives engine app transport extra api; do \
       mkdir -p src/$d/src && touch src/$d/src/lib.rs; done
# Build dependencies only
RUN if [ -n "$PROXY_URL" ]; then export http_proxy="$PROXY_URL" https_proxy="$PROXY_URL" all_proxy="socks5://${PROXY_URL#*//}"; fi \
    && case "$TARGETARCH" in \
        "amd64") cargo build --release --target x86_64-unknown-linux-musl ;; \
        "arm64") cargo build --release --target aarch64-unknown-linux-musl ;; \
    esac

COPY . .
# Touch main.rs to force cargo to rebuild the binary
RUN touch src/core/src/main.rs \
    && if [ -n "$PROXY_URL" ]; then export http_proxy="$PROXY_URL" https_proxy="$PROXY_URL" all_proxy="socks5://${PROXY_URL#*//}"; fi \
		&& export CC_x86_64_unknown_linux_musl=musl-gcc \
    && export CC_aarch64_unknown_linux_musl=musl-gcc \
    && case "$TARGETARCH" in \
        "amd64") \
						export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
            && cargo build --release --target x86_64-unknown-linux-musl \
            && upx --best --lzma target/x86_64-unknown-linux-musl/release/vane \
            && cp target/x86_64-unknown-linux-musl/release/vane /app/vane ;; \
        "arm64") \
						export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
            && cargo build --release --target aarch64-unknown-linux-musl \
            && upx --best --lzma target/aarch64-unknown-linux-musl/release/vane \
            && cp target/aarch64-unknown-linux-musl/release/vane /app/vane ;; \
    esac

FROM scratch

WORKDIR /app
COPY --from=builder /app/vane ./vane

WORKDIR /root/vane
COPY --from=builder /app/LICENSE /app/README.md /app/CHANGELOG.md /app/SECURITY.md ./

WORKDIR /app
CMD ["./vane"]