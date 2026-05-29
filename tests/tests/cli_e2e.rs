//! CLI-driven end-to-end scenarios: author config with the real `vane`
//! CLI, serve it with the real `vaned`, drive real traffic, assert the
//! wire response. Built on `VanedFixture` — the same scaffold an
//! operator walks by hand (scaffold -> author -> start). Turning today's
//! manual run into a permanent regression so the assembled product can't
//! silently regress at the seams.

use std::net::SocketAddr;
use std::time::Duration;

use vane_testutil::echo::EchoServer;
use vane_testutil::port::free_port;
use vane_testutil::vaned_fixture::{VanedFixture, http_get};

const READY: Duration = Duration::from_secs(10);

/// `static_site` synthesises a fixed response — no upstream involved.
#[test]
fn static_site_serves_fixed_response() {
	let listen = format!("127.0.0.1:{}", free_port());
	let addr: SocketAddr = listen.parse().expect("listen addr");

	let f = VanedFixture::new();
	f.add_static_site("site", &listen, 200, "hello-from-static-xyz");
	let vaned = f.start();
	vaned.wait_listener(addr, READY);

	let resp = http_get(addr, "/");
	assert!(resp.starts_with("HTTP/1.1 200"), "expected 200 status line, got:\n{resp}");
	assert!(resp.contains("hello-from-static-xyz"), "synthesised body missing:\n{resp}");
}

/// `port_forward` raw-L4-forwards bytes to an upstream that happens to
/// speak HTTP; the upstream's response must come back verbatim.
#[test]
fn port_forward_proxies_to_upstream() {
	let upstream = EchoServer::start("echo-via-l4-forward-xyz");
	let listen = format!("127.0.0.1:{}", free_port());
	let addr: SocketAddr = listen.parse().expect("listen addr");

	let f = VanedFixture::new();
	f.add_port_forward("fwd", &listen, &upstream.addr().to_string());
	let vaned = f.start();
	vaned.wait_listener(addr, READY);

	let resp = http_get(addr, "/");
	assert!(resp.contains("200 OK"), "expected upstream 200, got:\n{resp}");
	assert!(resp.contains("echo-via-l4-forward-xyz"), "upstream body missing:\n{resp}");
}

/// `reverse_proxy` parses the request and HTTP-proxies it to an upstream.
#[test]
fn reverse_proxy_forwards_http() {
	let upstream = EchoServer::start("echo-via-reverse-proxy-xyz");
	let listen = format!("127.0.0.1:{}", free_port());
	let addr: SocketAddr = listen.parse().expect("listen addr");

	let f = VanedFixture::new();
	f.add_reverse_proxy("proxy", &listen, &upstream.addr().to_string());
	let vaned = f.start();
	vaned.wait_listener(addr, READY);

	let resp = http_get(addr, "/");
	assert!(resp.contains("200"), "expected 200, got:\n{resp}");
	assert!(resp.contains("echo-via-reverse-proxy-xyz"), "upstream body missing:\n{resp}");
}
