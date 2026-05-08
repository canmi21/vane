# CGI Request

Serialize an HTTP request into the RFC 3875 §4 environment-variable
list a CGI child expects on `execve`. The other half of a CGI
gateway — turning the child's stdout into an HTTP response — lives
in the [`cgi-response`](https://crates.io/crates/cgi-response) crate; pair them when you need both
directions.

## Features

- [`build_env`] — pure function, takes a [`CgiRequestMeta`] and
  returns `Vec<(String, String)>` ready for
  `std::process::Command::envs(...)`. Computes the RFC 3875 §4.1
  meta-variables (`CONTENT_LENGTH` / `REMOTE_ADDR` / `SCRIPT_NAME`
  / …), the common-extension set (`REMOTE_PORT` / `REQUEST_URI` /
  `HTTPS` / …), and the `HTTP_*` passthrough for inbound headers.
- [`is_reserved_env_key`] — predicate for operator-config
  validation: returns `true` when an extra-env key would collide
  with what `build_env` computes per request.
- [`RFC_3875_REQUIRED`] / [`COMMON_EXTENSIONS`] — the literal name
  lists, exposed for downstream introspection / docs.

## Example

```rust
use std::net::SocketAddr;
use std::path::Path;
use cgi_request::{build_env, CgiRequestMeta};

let headers = http::HeaderMap::new();
let server: SocketAddr = "127.0.0.1:8080".parse().unwrap();
let client: SocketAddr = "10.0.0.1:54321".parse().unwrap();

let env = build_env(&CgiRequestMeta {
    method: "GET",
    path: "/cgi-bin/app.cgi/users/42",
    query: Some("sort=asc"),
    headers: &headers,
    script_name: "/cgi-bin/app.cgi",
    working_dir: Path::new("/var/www/cgi-bin"),
    server_addr: server,
    remote_addr: client,
    is_tls: false,
    server_software: "myapp/1.0",
    block_headers: &[],
    extra_env: &[],
});

assert!(env.iter().any(|(k, v)| k == "SCRIPT_NAME" && v == "/cgi-bin/app.cgi"));
assert!(env.iter().any(|(k, v)| k == "PATH_INFO" && v == "/users/42"));
assert!(env.iter().any(|(k, v)| k == "QUERY_STRING" && v == "sort=asc"));
```

## License

Released under the MIT License © 2026 [Canmi](https://canmi.net)
