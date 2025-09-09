/* src/server/http3_server.rs */

use crate::{config::AppConfig, proxy, state::AppState, tls::PerDomainCertResolver};
use anyhow::{Result, anyhow};
use axum::Router;
use axum::body::{Body, to_bytes};
use bytes::{Buf, Bytes, BytesMut};
use fancy_log::{LogLevel, log, set_log_level};
use http::{Response as HttpResponse, StatusCode};
use hyper::Request as HyperRequest;
use quinn::crypto::rustls::QuicServerConfig as QuinnRustlsServerConfig;
use rustls::ServerConfig as RustlsServerConfig;
use std::{net::SocketAddr, sync::Arc};
use tokio::task::JoinHandle;
use tower::ServiceExt;

pub async fn spawn(
    app_config: Arc<AppConfig>,
    state: Arc<AppState>,
) -> Result<Option<JoinHandle<Result<(), anyhow::Error>>>> {
    set_log_level(LogLevel::Debug);

    if !app_config.domains.values().any(|d| d.https && d.http3) {
        log(
            LogLevel::Info,
            &format!("No HTTP/3 domains configured, HTTP/3 server will not be started."),
        );
        return Ok(None);
    }

    let resolver = PerDomainCertResolver::new(app_config.clone());
    let mut server_config = RustlsServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));

    server_config.alpn_protocols = vec![b"h3".to_vec()];

    let quic_crypto_config = QuinnRustlsServerConfig::try_from(Arc::new(server_config))?;
    let quic_config = quinn::ServerConfig::with_crypto(Arc::new(quic_crypto_config));

    let https_addr = SocketAddr::from(([0, 0, 0, 0], app_config.https_port));

    log(
        LogLevel::Info,
        &format!("Vane HTTPS/UDP (H3) server listening on {}", https_addr),
    );

    let router = Router::new()
        .fallback(proxy::proxy_handler)
        .with_state(state.clone());

    let handle = tokio::spawn(async move {
        let endpoint = quinn::Endpoint::server(quic_config, https_addr)?;
        while let Some(conn) = endpoint.accept().await {
            log(
                LogLevel::Info,
                &format!("H3: New QUIC connection from: {}", conn.remote_address()),
            );
            let router_clone = router.clone();
            tokio::spawn(async move {
                let quinn_conn = match conn.await {
                    Ok(c) => c,
                    Err(e) => {
                        log(
                            LogLevel::Error,
                            &format!("H3: Accepting connection failed: {}", e),
                        );
                        return Err(anyhow!("H3 connection failed: {}", e));
                    }
                };

                let mut h3_conn =
                    h3::server::Connection::new(h3_quinn::Connection::new(quinn_conn)).await?;

                while let Ok(Some(resolver)) = h3_conn.accept().await {
                    let router_clone_inner = router_clone.clone();
                    tokio::spawn(async move {
                        match resolver.resolve_request().await {
                            Ok((req, mut stream)) => {
                                let mut req_body = BytesMut::new();
                                loop {
                                    match stream.recv_data().await {
                                        Ok(Some(mut chunk)) => {
                                            let b = chunk.copy_to_bytes(chunk.remaining());
                                            req_body.extend_from_slice(&b);
                                        }
                                        Ok(None) => break,
                                        Err(e) => {
                                            log(
                                                LogLevel::Error,
                                                &format!("H3: error reading request body: {}", e),
                                            );
                                            let _ = stream
                                                .send_response(
                                                    HttpResponse::builder()
                                                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                                                        .body(())
                                                        .unwrap(),
                                                )
                                                .await;
                                            let _ = stream.finish().await;
                                            return;
                                        }
                                    }
                                }

                                let mut builder = HyperRequest::builder()
                                    .method(req.method().clone())
                                    .uri(req.uri().clone());

                                for (k, v) in req.headers().iter() {
                                    builder = builder.header(k, v);
                                }

                                let hyper_req = match builder.body(Body::from(req_body.freeze())) {
                                    Ok(r) => r,
                                    Err(e) => {
                                        log(
                                            LogLevel::Error,
                                            &format!("H3: failed to build request: {}", e),
                                        );
                                        let _ = stream
                                            .send_response(
                                                HttpResponse::builder()
                                                    .status(StatusCode::BAD_REQUEST)
                                                    .body(())
                                                    .unwrap(),
                                            )
                                            .await;
                                        let _ = stream.finish().await;
                                        return;
                                    }
                                };

                                let resp = match router_clone_inner.oneshot(hyper_req).await {
                                    Ok(r) => r,
                                    Err(e) => {
                                        log(
                                            LogLevel::Error,
                                            &format!("H3: router call failed: {}", e),
                                        );
                                        let _ = stream
                                            .send_response(
                                                HttpResponse::builder()
                                                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                                                    .body(())
                                                    .unwrap(),
                                            )
                                            .await;
                                        let _ = stream.finish().await;
                                        return;
                                    }
                                };

                                let mut head_builder =
                                    HttpResponse::builder().status(resp.status());
                                for (k, v) in resp.headers().iter() {
                                    head_builder = head_builder.header(k, v);
                                }
                                let head = head_builder.body(()).unwrap();

                                if let Err(e) = stream.send_response(head).await {
                                    log(
                                        LogLevel::Error,
                                        &format!("H3: Failed to send response headers: {}", e),
                                    );
                                    let _ = stream.finish().await;
                                    return;
                                }

                                match to_bytes(resp.into_body(), 10 * 1024 * 1024).await {
                                    Ok(b) => {
                                        if !b.is_empty() {
                                            if let Err(e) = stream.send_data(Bytes::from(b)).await {
                                                log(
                                                    LogLevel::Error,
                                                    &format!(
                                                        "H3: Failed to send response body: {}",
                                                        e
                                                    ),
                                                );
                                                let _ = stream.finish().await;
                                                return;
                                            }
                                        }
                                        if let Err(e) = stream.finish().await {
                                            log(
                                                LogLevel::Error,
                                                &format!("H3: Failed to finish stream: {}", e),
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        log(
                                            LogLevel::Error,
                                            &format!("H3: Failed to read response body: {}", e),
                                        );
                                        let _ = stream.finish().await;
                                    }
                                }
                            }
                            Err(e) => {
                                log(
                                    LogLevel::Error,
                                    &format!("H3: resolve_request error: {}", e),
                                );
                            }
                        }
                    });
                }
                Ok::<(), anyhow::Error>(())
            });
        }
        Ok::<(), anyhow::Error>(())
    });

    Ok(Some(handle))
}
