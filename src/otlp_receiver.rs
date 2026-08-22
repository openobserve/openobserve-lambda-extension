/// OTLP HTTP receiver — listens on localhost:4318
///
/// The OTel SDK (via AWS_LAMBDA_EXEC_WRAPPER + auto-instrumentations)
/// posts spans to http://localhost:4318/v1/traces and metrics to
/// http://localhost:4318/v1/metrics. This receiver buffers both — preserving
/// each request's Content-Type so protobuf and JSON payloads round-trip
/// intact — and forwards them (gzip-compressed) to OpenObserve's /v1/traces
/// and /v1/metrics endpoints.
use anyhow::{anyhow, Result};
use http::{Request, Response, StatusCode};
use hyper::{body, Body, Server};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

use crate::compress::gzip;
use crate::config::Config;

/// A single buffered OTLP payload plus the Content-Type it arrived with.
/// Preserving content type lets us forward protobuf as protobuf and JSON as JSON.
#[derive(Debug, Clone)]
pub struct OtlpPayload {
    pub content_type: String,
    pub body: Vec<u8>,
}

impl OtlpPayload {
    pub fn json(body: Vec<u8>) -> Self {
        Self {
            content_type: "application/json".into(),
            body,
        }
    }
}

// Shared buffers of OTLP payloads received from the SDK
pub type SpanBuffer = Arc<Mutex<Vec<OtlpPayload>>>;
pub type MetricBuffer = Arc<Mutex<Vec<OtlpPayload>>>;

pub struct OtlpReceiver {
    port: u16,
    span_buffer: SpanBuffer,
    metric_buffer: MetricBuffer,
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl OtlpReceiver {
    pub fn new(port: u16) -> (Self, SpanBuffer, MetricBuffer) {
        let span_buffer: SpanBuffer = Arc::new(Mutex::new(Vec::new()));
        let metric_buffer: MetricBuffer = Arc::new(Mutex::new(Vec::new()));
        let receiver = Self {
            port,
            span_buffer: Arc::clone(&span_buffer),
            metric_buffer: Arc::clone(&metric_buffer),
            server_handle: None,
        };
        (receiver, span_buffer, metric_buffer)
    }

    pub async fn start(&mut self) -> Result<()> {
        let addr = SocketAddr::from(([127, 0, 0, 1], self.port));
        let spans = Arc::clone(&self.span_buffer);
        let metrics = Arc::clone(&self.metric_buffer);

        let make_svc = hyper::service::make_service_fn(move |_conn| {
            let spans = Arc::clone(&spans);
            let metrics = Arc::clone(&metrics);
            async move {
                Ok::<_, Infallible>(hyper::service::service_fn(move |req| {
                    handle_otlp_request(req, Arc::clone(&spans), Arc::clone(&metrics))
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
    spans: SpanBuffer,
    metrics: MetricBuffer,
) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path().to_string();

    match (req.method(), path.as_str()) {
        (&hyper::Method::POST, "/v1/traces") => match buffer_otlp_payload(req, spans).await {
            Ok(bytes) => {
                debug!("📥 Buffered {} bytes of trace data", bytes);
                ok_json()
            }
            Err(e) => {
                error!("❌ Failed to buffer OTLP traces: {}", e);
                err_json()
            }
        },
        (&hyper::Method::POST, "/v1/metrics") => match buffer_otlp_payload(req, metrics).await {
            Ok(bytes) => {
                debug!("📥 Buffered {} bytes of metric data", bytes);
                ok_json()
            }
            Err(e) => {
                error!("❌ Failed to buffer OTLP metrics: {}", e);
                err_json()
            }
        },
        // Silently accept logs (not implemented yet) so SDK doesn't error
        (&hyper::Method::POST, _) => ok_json(),
        _ => Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::from("{}"))
            .unwrap()),
    }
}

fn ok_json() -> Result<Response<Body>, Infallible> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(Body::from("{}"))
        .unwrap())
}

fn err_json() -> Result<Response<Body>, Infallible> {
    Ok(Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from("{}"))
        .unwrap())
}

async fn buffer_otlp_payload(
    req: Request<Body>,
    buffer: Arc<Mutex<Vec<OtlpPayload>>>,
) -> Result<usize> {
    // Capture content type BEFORE consuming the body
    let content_type = req
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("application/x-protobuf")
        .to_string();

    let body_bytes = body::to_bytes(req.into_body())
        .await
        .map_err(|e| anyhow!("Failed to read OTLP body: {}", e))?;

    let len = body_bytes.len();
    if len == 0 {
        return Ok(0);
    }

    {
        let mut guard = buffer.lock().await;
        guard.push(OtlpPayload {
            content_type,
            body: body_bytes.to_vec(),
        });
    }

    Ok(len)
}

/// Drain all buffered span payloads and forward to OpenObserve /v1/traces.
pub async fn flush_spans(
    client: &reqwest::Client,
    config: &Config,
    buffer: &SpanBuffer,
) -> Result<u64> {
    flush_signal(
        client,
        buffer,
        &config.traces_url(),
        &config.effective_auth_header(),
        config.parsed_otlp_headers(),
        "traces",
    )
    .await
}

/// Drain all buffered metric payloads and forward to OpenObserve /v1/metrics.
pub async fn flush_metrics(
    client: &reqwest::Client,
    config: &Config,
    buffer: &MetricBuffer,
) -> Result<u64> {
    flush_signal(
        client,
        buffer,
        &config.metrics_url(),
        &config.effective_auth_header(),
        config.parsed_otlp_headers(),
        "metrics",
    )
    .await
}

async fn flush_signal(
    client: &reqwest::Client,
    buffer: &Arc<Mutex<Vec<OtlpPayload>>>,
    url: &str,
    auth: &str,
    extra_headers: Vec<(String, String)>,
    signal: &str,
) -> Result<u64> {
    let payloads: Vec<OtlpPayload> = {
        let mut guard = buffer.lock().await;
        std::mem::take(&mut *guard)
    };

    if payloads.is_empty() {
        return Ok(0);
    }

    let mut total: u64 = 0;

    for payload in payloads {
        let raw_len = payload.body.len();

        // gzip once — OTLP endpoints accept Content-Encoding: gzip.
        let (body, encoding) = match gzip(&payload.body) {
            Ok(gz) => (gz, Some("gzip")),
            Err(e) => {
                warn!("gzip failed for {}, sending uncompressed: {}", signal, e);
                (payload.body.clone(), None)
            }
        };

        debug!(
            "🔍 Forwarding {} bytes of {} ({}→{} bytes gzipped) to {}",
            raw_len,
            signal,
            raw_len,
            body.len(),
            url
        );

        let mut req = client
            .post(url)
            .header("Content-Type", &payload.content_type)
            .header("Authorization", auth);
        if let Some(enc) = encoding {
            req = req.header("Content-Encoding", enc);
        }

        // Forward any extra OTLP headers (except Authorization, already set above)
        for (k, v) in &extra_headers {
            if k.to_lowercase() != "authorization" {
                req = req.header(k, v);
            }
        }

        match req.body(body).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    total += 1;
                    debug!("✅ {} forwarded successfully", signal);
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    warn!("⚠️ OpenObserve {} rejected {}: {}", signal, status, body);
                }
            }
            Err(e) => {
                error!("❌ Failed to forward {}: {}", signal, e);
            }
        }
    }

    Ok(total)
}
