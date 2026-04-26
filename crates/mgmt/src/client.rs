//! Typed management client — what `vane` CLI / TUI link against. Same
//! verb set, same frame shapes as `server`. One Unix-socket connection
//! per call: the API stays a simple `call(verb, args) -> result`, and
//! a future multiplexed transport can be slotted in without changing
//! the call shape.
//!
//! See `spec/architecture/10-management.md`.

use std::path::{Path, PathBuf};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::protocol::{Request, Response, ResponseOutcome, WireError, encode_line};

/// Single-shot typed Unix mgmt client. Each `call` opens a fresh
/// connection. Re-using one client for many calls works too — the
/// struct holds no persistent state across invocations.
pub struct UnixMgmtClient {
	socket_path: PathBuf,
}

impl UnixMgmtClient {
	pub fn new(socket_path: impl AsRef<Path>) -> Self {
		Self { socket_path: socket_path.as_ref().to_path_buf() }
	}

	/// Send a verb + typed args, await typed result.
	///
	/// `id` is fixed at `1` for the single-request-per-connection
	/// transport — there is no need for cross-process uniqueness on a
	/// freshly-opened socket. A future multiplexed transport will own
	/// its own id-allocation scheme.
	///
	/// # Errors
	/// I/O failure ([`MgmtClientError::Io`]); a structured server-side
	/// error ([`MgmtClientError::Server`]); or a JSON shape mismatch
	/// when decoding either the response frame or the result payload
	/// ([`MgmtClientError::Decode`]).
	pub async fn call<A, R>(&self, verb: &str, args: &A) -> Result<R, MgmtClientError>
	where
		A: serde::Serialize,
		R: for<'de> serde::Deserialize<'de>,
	{
		let stream = UnixStream::connect(&self.socket_path).await?;
		let (read, mut write) = stream.into_split();

		let req = Request {
			id: 1,
			verb: verb.to_string(),
			args: serde_json::to_value(args).map_err(MgmtClientError::Encode)?,
		};
		let bytes = encode_line(&req).map_err(MgmtClientError::Encode)?;
		write.write_all(&bytes).await?;
		// Half-close the write half so the server's `next_line` returns
		// `None` after the response — the server task can then drop the
		// connection cleanly.
		write.shutdown().await.ok();

		let mut lines = BufReader::new(read).lines();
		let line = lines.next_line().await?.ok_or(MgmtClientError::EmptyResponse)?;
		let response: Response = serde_json::from_str(&line).map_err(MgmtClientError::Decode)?;
		match response.outcome {
			ResponseOutcome::Result { result } => {
				serde_json::from_value(result).map_err(MgmtClientError::Decode)
			}
			ResponseOutcome::Error { error } => Err(MgmtClientError::Server(error)),
		}
	}
}

#[derive(Debug, thiserror::Error)]
pub enum MgmtClientError {
	#[error("io: {0}")]
	Io(#[from] std::io::Error),
	#[error("encode: {0}")]
	Encode(serde_json::Error),
	#[error("decode: {0}")]
	Decode(serde_json::Error),
	#[error("empty response")]
	EmptyResponse,
	#[error("server: {kind:?} {message}", kind = .0.kind, message = .0.message)]
	Server(WireError),
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::server::{Handler, handle_conn};
	use crate::verb::PingResult;
	use async_trait::async_trait;
	use std::sync::Arc;

	struct StubHandler;

	#[async_trait]
	impl Handler for StubHandler {
		async fn dispatch(
			&self,
			req: Request,
		) -> Result<serde_json::Value, crate::protocol::WireError> {
			match req.verb.as_str() {
				"ping" => Ok(serde_json::json!({ "pong": true, "version": "test" })),
				"bad_shape" => Ok(serde_json::json!({ "unrelated": 1 })),
				_ => Err(WireError {
					kind: crate::protocol::WireErrorKind::UnknownVerb,
					message: format!("unknown {}", req.verb),
				}),
			}
		}
	}

	/// Connect a duplex pair and run the server's `handle_conn` against
	/// the stub. Returns a closure-like helper bound to the test stream
	/// — used by the typed-call decode tests below.
	async fn drive_call<A, R>(verb: &str, args: A) -> Result<R, MgmtClientError>
	where
		A: serde::Serialize,
		R: for<'de> serde::Deserialize<'de>,
	{
		let (c2s_r, mut c2s_w) = tokio::io::duplex(8192);
		let (s2c_w, s2c_r) = tokio::io::duplex(8192);
		let server = tokio::spawn(handle_conn(c2s_r, s2c_w, Arc::new(StubHandler)));

		// Client side: write the request line, half-close, read one response line.
		let req = Request {
			id: 1,
			verb: verb.to_string(),
			args: serde_json::to_value(&args).expect("args serialize"),
		};
		let bytes = encode_line(&req).expect("encode");
		c2s_w.write_all(&bytes).await.expect("write");
		drop(c2s_w);

		let mut lines = BufReader::new(s2c_r).lines();
		let line = lines
			.next_line()
			.await
			.map_err(MgmtClientError::Io)?
			.ok_or(MgmtClientError::EmptyResponse)?;
		let response: Response = serde_json::from_str(&line).map_err(MgmtClientError::Decode)?;
		// Drain the server task. (`handle_conn` returns once `next_line` returns
		// None on the read half, which happens on `drop(c2s_w)`.)
		let _ = server.await;
		match response.outcome {
			ResponseOutcome::Result { result } => {
				serde_json::from_value(result).map_err(MgmtClientError::Decode)
			}
			ResponseOutcome::Error { error } => Err(MgmtClientError::Server(error)),
		}
	}

	#[tokio::test]
	async fn client_call_decodes_typed_result() {
		let result: PingResult = drive_call("ping", crate::verb::NoArgs {}).await.expect("ok");
		assert!(result.pong);
		assert_eq!(result.version, "test");
	}

	#[tokio::test]
	async fn client_surfaces_server_error_as_mgmt_client_error_server() {
		let err = drive_call::<_, PingResult>("nope", crate::verb::NoArgs {}).await.expect_err("err");
		match err {
			MgmtClientError::Server(w) => {
				assert_eq!(w.kind, crate::protocol::WireErrorKind::UnknownVerb);
			}
			other => panic!("expected Server, got {other:?}"),
		}
	}

	#[tokio::test]
	async fn client_decode_error_when_result_shape_mismatches() {
		let err =
			drive_call::<_, PingResult>("bad_shape", crate::verb::NoArgs {}).await.expect_err("err");
		assert!(matches!(err, MgmtClientError::Decode(_)), "unexpected variant: {err:?}");
	}

	#[tokio::test]
	async fn client_io_error_on_missing_socket() {
		let tmp = tempfile::tempdir().expect("tempdir");
		let path = tmp.path().join("not-there.sock");
		let client = UnixMgmtClient::new(&path);
		let err = client
			.call::<_, PingResult>("ping", &crate::verb::NoArgs {})
			.await
			.expect_err("must fail without a server");
		assert!(matches!(err, MgmtClientError::Io(_)), "unexpected variant: {err:?}");
	}
}
