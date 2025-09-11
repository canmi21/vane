# Vane

## Overview
Vane is a modern, high-performance web proxy and server written entirely in Rust. It serves as a reverse proxy, handling HTTP, HTTPS, and HTTP/3 traffic with built-in features like dynamic routing, failover, rate limiting, CORS management, and automatic TLS certificate renewal. Designed for simplicity and security, Vane draws inspiration from tools like Nginx, OpenResty, and Caddy but reimplements their capabilities in pure Rust for better safety, portability, and minimalism.

Unlike traditional servers, Vane avoids dependencies on external scripting languages or libraries like LuaJIT (used in OpenResty) or OpenSSL. Instead, it leverages Rust's ecosystem—such as Axum for routing, Tokio for async I/O, and rustls for TLS—to deliver a fully memory-safe, thread-safe implementation. This results in a tiny Docker image (~5MB) built on `scratch` (no shell, no OS utils), making it ideal for edge deployments, microservices, and security-conscious environments.

### Key Features
- **Advanced Routing**: Supports path-based routing with wildcard (`*`) matching, prefix-based specificity scoring, and multiple backend targets for automatic failover (tries backups on 5xx errors or connection failures).
- **Rate Limiting**: Configurable per-domain, per-route, and override rules using [governor.rs](https://crates.io/crates/governor) and [lazy-limit](https://crates.io/crates/lazy-limit). Includes a global "shield" limiter for DDoS protection (30 req/s default).
- **TLS and Certificates**: 100% rustls-based TLS (no OpenSSL). Supports self-signed certs for dev, or automatic ACME renewal via a configurable cert server (e.g., lazy-acme daemon). Background renewal task checks hourly and restarts on success.
- **HTTP/3 Support**: QUIC-based HTTP/3 with Alt-Svc header for discovery.
- **CORS and Security Headers**: Fine-grained CORS per-origin/method, HSTS enforcement, and method filtering (e.g., allow only GET/POST).
- **Proxying and Middleware**: Cleans spoofable headers, injects X-Forwarded-For, handles HTTP upgrades/rejects, and chains middleware for extensibility.
- **Configuration**: TOML-based configs for main and per-domain settings. Environment variables for ports, paths, and logging.
- **Logging and MOTD**: Custom fancy-log for colored output, with lazy-motd for startup banners.
- **Error Handling**: Custom status pages (embedded or file-based) for 4xx/5xx errors.
- **Performance**: Asynchronous, non-blocking design with Tokio. No JIT compilation overhead like Lua in OpenResty.
- **Security**: Runs in a minimal Docker container with no shell, reducing attack surface. Memory-safe Rust prevents common vulnerabilities.

## Comparison with Nginx, OpenResty, and Caddy
Vane positions itself as a Rust-native alternative to established web servers, focusing on security, minimalism, and modern features without sacrificing performance.

- **vs. Nginx**: Nginx is battle-tested and highly optimized for performance, often considered at its optimization ceiling for C-based servers. However, Vane reimplements key Nginx features (e.g., reverse proxying, routing, rate limiting) in Rust, avoiding C's memory issues. Vane adds built-in HTTP/3, automatic cert renewal, and wildcard path matching with specificity scoring—features that require modules or custom configs in Nginx. While Nginx excels in raw throughput for simple static serving, Vane's async Rust core handles dynamic workloads efficiently without plugins.

- **vs. OpenResty**: OpenResty extends Nginx with LuaJIT for scripting advanced logic like rate limiting, failover routing, wildcard matching, and custom middleware. Vane replicates these "plugins" natively in Rust: rate limiting with governor (no Lua scripts), failover via ordered target lists, wildcard (`*`) path matching with prefix scoring to resolve ambiguities, and middleware chains for CORS/HSTS/Alt-Svc. By ditching LuaJIT, Vane eliminates JIT overhead, potential scripting vulnerabilities, and dependency bloat. It's fully compiled, safer, and more predictable.

- **vs. Caddy**: Caddy is Go-based, automatic HTTPS-focused, and user-friendly with its Caddyfile config. Vane matches Caddy's auto-TLS but uses a separate cert daemon (lazy-acme) for renewal, allowing decoupled management. Vane's TOML configs are more structured for complex routing/rate limits, and its Rust foundation offers better performance in async I/O-heavy scenarios (e.g., proxies). Unlike Caddy's larger binary (~10-15MB), Vane's Docker image is ~5MB on scratch—no Go runtime bloat.

In summary, while Nginx/OpenResty shine in legacy ecosystems and Caddy in simplicity, Vane stands out for its 100% Rust purity, rustls TLS (no OpenSSL vulnerabilities), and extreme minimalism. It's perfect for environments prioritizing security and small footprints, like containers or edge computing.

## Architecture
Vane's architecture is modular, asynchronous, and built around Rust's ecosystem for web serving. Here's a breakdown based on the codebase:

### Core Components
- **Entry Point (`main.rs`)**: Initializes logging, environment variables (via dotenvy), and a global "shield" rate limiter (30 req/s). Loads configs, handles first-run setup (creates example configs/certs), spawns a background cert renewal task (hourly checks via Tokio interval, refreshes via ACME if >24h old, restarts on success), and starts servers.
  
- **Configuration (`config.rs`)**: Loads from `.env` and TOML files. Main `config.toml` maps domains to per-domain TOML files (e.g., `example.com.toml`). Supports env vars for ports, cert dir/server, log level. Validates HTTPS/TLS configs.

- **ACME Client (`acme_client.rs`)**: Fetches certs/keys from a lazy-acme server with retries (5 attempts, 5s delay). Decodes base64 responses and saves PEM files.

- **Middleware Chain (`middleware.rs`)**: A series of Axum middlewares for request processing:
  - **Method Filter**: Early rejection of disallowed HTTP methods per domain (e.g., only GET/POST).
  - **CORS**: Custom handler for preflight (OPTIONS) and actual requests. Supports per-origin method allowlists or `*` wildcard.
  - **Rate Limiting**: Two layers—global shield + per-domain/route/override rules. Uses IP-based keys with governor.
  - **Alt-Svc**: Adds HTTP/3 discovery header for HTTPS responses.
  - **HTTP Options**: Handles plain HTTP: upgrade to HTTPS, reject, or allow.
  - **HSTS**: Adds Strict-Transport-Security header if enabled.
  - **Host Injection**: Adds missing Host header from URI authority.

- **Routing and Path Matching (`routing.rs`, `path_matcher.rs`)**: Finds best route by scoring path patterns (exact segments > wildcards, longer prefixes preferred). Resolves ambiguities (equal scores = error). Returns ordered targets for failover.

- **Proxying (`proxy.rs`)**: Forwards requests to backends. Buffers body for reuse, cleans spoofable IP headers, adds X-Forwarded-For. Tries targets sequentially on failure (connection errors or 5xx). Uses hyper client with rustls.

- **Rate Limiting Details (`ratelimit.rs`)**: Finds best-matching limiter by path score. Separate maps for route/override rules.

- **TLS Resolver (`tls.rs`)**: Dynamic SNI-based cert resolution with rustls. Loads PEM certs/keys per domain.

- **Setup and Errors (`setup.rs`, `error.rs`)**: First-run creates dirs, self-signed certs (or ACME fetch), embedded status pages. Serves custom HTML error pages (400-500 range).

- **Servers (`server/mod.rs`)**: Runs HTTP (TCP), HTTPS (TCP with rustls), and HTTP/3 (QUIC with quinn/h3). Binds ports from env, layers middleware on Axum routers.

- **State (`state.rs`)**: Shared Arc<AppState> with config, hyper client, and pre-built rate limiters.

- **Models (`models.rs`)**: TOML-deserializable structs for configs (domains, routes, rate limits, CORS, etc.).

This design ensures scalability: async tasks for renewals, middleware for extensibility, and compiled logic for speed/security.

## Installation and Usage
We recommend running Vane via Docker Compose for easy deployment and management.

### Prerequisites
- Docker and Docker Compose installed.
- Optional: A lazy-acme server for automatic certs (see https://github.com/canmi21/lazy-acme).

### Quick Start with Docker Compose
1. Create a `docker-compose.yml` file:
   ```yaml
   services:
     vane:
       image: canmi/vane:latest
       container_name: vane
       networks:
         - internal
       env_file:
         - ./.env
       ports:
         - "80:80/tcp"
         - "443:443/tcp"
         - "443:443/udp"
       volumes:
         - /opt/vane:/root/vane
       restart: unless-stopped

   networks:
     internal:
       driver: bridge
   ```

2. Create a `.env` file with your settings:
   ```
   # Log level: debug, info, warn, error
   LOG_LEVEL=info

   # Ports to bind (HTTP and HTTPS/HTTP3)
   BIND_HTTP_PORT=80
   BIND_HTTPS_PORT=443

   # Absolute path to main config (use /root for Docker)
   CONFIG=/root/vane/config.toml

   # Directory for ACME certificates
   CERT_DIR=/root/vane/cert

   # URL to lazy-acme server for cert renewal (optional for self-signed mode)
   CERT_SERVER=http://your.certserver.com:port
   ```

3. Mount your configs/certs to `/opt/vane` on the host.

4. Run: `docker compose up -d`.

### Configuration
- **Main Config (`config.toml`)**: Maps domains to files, e.g.:
  ```toml
  [domains]
  "example.com" = "example.com.toml"
  ```
- **Domain Config (e.g., `example.com.toml`)**: See embedded example in code for full options.
- On first run (no domains), Vane auto-sets up examples and exits—restart to apply.

For development, build from source: `cargo build --release`.

## License
AGPL-3.0 (see LICENSE file).

For more details, visit the repository: https://github.com/canmi21/vane.