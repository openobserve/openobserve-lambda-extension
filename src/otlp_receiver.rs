/// OTLP HTTP receiver — listens on localhost:4318
///
/// The Node.js OTel SDK (via AWS_LAMBDA_EXEC_WRAPPER + auto-instrumentations)
/// sends spans to http://localhost:4318/v1/traces.
/// This receiver accepts them and forwards to OpenObserve /v1/traces.
use anyhow::{anyhow, Result};
use http::{Request, Response, StatusCode};
use hyper::{body, Body, Server};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

use crate::config::Config;

// Shared buffer of raw OTLP JSON payloads received from the SDK
pub type SpanBuffer = Arc<Mutex<Vec<Vec<u8>>>>;

pub struct OtlpReceiver {
    port: u16,
    buffer: SpanBuffer,
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl OtlpReceiver {
    pub fn new(port: u16) -> (Self, SpanBuffer) {
        let buffer: SpanBuffer = Arc::new(Mutex::new(Vec::new()));
        let receiver = Self {
            port,
            buffer: Arc::clone(&buffer),
            server_handle: None,
        };
        (receiver, buffer)
    }

    pub async fn start(&mut self) -> Result<()> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));
        let buffer = Arc::clone(&self.buffer);

        let make_svc = hyper::service::make_service_fn(move |_conn| {
            let buffer = Arc::clone(&buffer);
            async move {
                Ok::<_, Infallible>(hyper::service::service_fn(move |req| {
                    handle_otlp_request(req, Arc::clone(&buffer))
                }))
            }
        });

        let server = Server::bind(&addr).serve(make_svc);
        debug!("🔌 OTLP receiver listening on localhost:{}", self.port);

        let handle = tokio::spawn(async move {
            if let Err(e) = server.await {
                error!("❌ OTLP receiver error: {}", e);
            }
        });

        self.server_handle = Some(handle);
        Ok(())
    }

    pub async fn shutdown(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
    }
}

async fn handle_otlp_request(
    req: Request<Body>,
    buffer: SpanBuffer,
) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path().to_string();

    match (req.method(), path.as_str()) {
        (&hyper::Method::POST, "/v1/traces") => {
            match buffer_otlp_payload(req, buffer).await {
                Ok(bytes) => {
                    debug!("📥 Buffered {} bytes of trace data", bytes);
                    Ok(Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "application/json")
                        .body(Body::from("{}"))
                        .unwrap())
                }
                Err(e) => {
                    error!("❌ Failed to buffer OTLP traces: {}", e);
                    Ok(Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from("{}"))
                        .unwrap())
                }
            }
        }
        // Silently accept metrics/logs sent to wrong port — return OK so SDK doesn't error
        (&hyper::Method::POST, _) => {
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::from("{}"))
                .unwrap())
        }
        _ => Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::from("{}"))
            .unwrap()),
    }
}

async fn buffer_otlp_payload(req: Request<Body>, buffer: SpanBuffer) -> Result<usize> {
    let body_bytes = body::to_bytes(req.into_body())
        .await
        .map_err(|e| anyhow!("Failed to read OTLP body: {}", e))?;

    let len = body_bytes.len();
    if len == 0 {
        return Ok(0);
    }

    {
        let mut guard = buffer.lock().await;
        guard.push(body_bytes.to_vec());
    }

    Ok(len)
}

/// Drain all buffered span payloads and forward to OpenObserve /v1/traces.
pub async fn flush_spans(
    client: &reqwest::Client,
    config: &Config,
    buffer: &SpanBuffer,
) -> Result<u64> {
    let payloads: Vec<Vec<u8>> = {
        let mut guard = buffer.lock().await;
        std::mem::take(&mut *guard)
    };

    if payloads.is_empty() {
        return Ok(0);
    }

    let traces_url = config.traces_url();
    let auth = config.effective_auth_header();
    let mut total: u64 = 0;

    for payload in payloads {
        debug!("🔍 Forwarding {} bytes to {}", payload.len(), traces_url);

        // Determine content type — SDK sends protobuf by default,
        // but we configure it to send JSON via OTEL_EXPORTER_OTLP_PROTOCOL=http/json
        let content_type = "application/json";

        let mut req = client
            .post(&traces_url)
            .header("Content-Type", content_type)
            .header("Authorization", &auth);

        // Forward any extra OTLP headers
        for (k, v) in config.parsed_otlp_headers() {
            if k.to_lowercase() != "authorization" {
                req = req.header(k, v);
            }
        }

        match req.body(payload).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    total += 1;
                    debug!("✅ Traces forwarded successfully");
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    warn!("⚠️ OpenObserve traces rejected {}: {}", status, body);
                }
            }
            Err(e) => {
                error!("❌ Failed to forward traces: {}", e);
            }
        }
    }

    Ok(total)
}
