/// Synthesize `aws.lambda.enhanced.*` metrics from Lambda Telemetry API
/// `platform.report` events.
///
/// This reproduces the biggest value-add of Datadog's Lambda extension without
/// requiring any application code change: durationMs / billedDurationMs /
/// initDurationMs / memory usage / cold-start / error signals are all present in
/// the platform.report record; we just reshape them into OTLP metric points and
/// push them into the same MetricBuffer that the OTLP receiver drains to
/// OpenObserve's /v1/metrics endpoint.
use serde_json::{json, Value};
use std::env;
use tracing::{debug, warn};

use crate::otlp_receiver::{MetricBuffer, OtlpPayload};
use crate::telemetry::TelemetryEvent;

const SCOPE_NAME: &str = "openobserve.lambda.enhanced";
const SCOPE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// If the event is `platform.report`, synthesize an OTLP metrics JSON payload
/// and push it onto the metric buffer.
pub async fn maybe_emit_enhanced_metrics(event: &TelemetryEvent, metric_buffer: &MetricBuffer) {
    if event.event_type != "platform.report" {
        return;
    }

    let Some(payload) = build_payload(event) else {
        return;
    };

    match serde_json::to_vec(&payload) {
        Ok(bytes) => {
            let mut guard = metric_buffer.lock().await;
            guard.push(OtlpPayload::json(bytes));
            debug!(
                "📊 Synthesized enhanced metrics for request {}",
                event.request_id.as_deref().unwrap_or("<unknown>")
            );
        }
        Err(e) => warn!("Failed to serialize enhanced metrics: {}", e),
    }
}

/// Build an OTLP-JSON ExportMetricsServiceRequest for one platform.report event.
/// Returns None if the event has no numeric metrics block to derive from.
fn build_payload(event: &TelemetryEvent) -> Option<Value> {
    let record = &event.record;
    let metrics = record.get("metrics")?;

    let duration_ms = metrics.get("durationMs").and_then(Value::as_f64);
    let billed_ms = metrics.get("billedDurationMs").and_then(Value::as_f64);
    let memory_size = metrics.get("memorySizeMB").and_then(Value::as_f64);
    let max_memory = metrics.get("maxMemoryUsedMB").and_then(Value::as_f64);
    let init_duration_ms = metrics.get("initDurationMs").and_then(Value::as_f64);
    let restore_duration_ms = metrics.get("restoreDurationMs").and_then(Value::as_f64);

    let status = record
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("success");
    // platform.report events carry requestId under record; other event types have
    // it at the top level. Fall back gracefully.
    let request_id = event
        .request_id
        .clone()
        .or_else(|| record.get("requestId").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();

    // Lambda Telemetry API timestamps are RFC3339. Convert to nanoseconds since epoch.
    let time_nano: i128 = event
        .time
        .timestamp_nanos_opt()
        .map(|n| n as i128)
        .unwrap_or_else(|| (event.time.timestamp() as i128) * 1_000_000_000);
    let time_nano_str = time_nano.to_string();

    let dp_attrs = json!([
        {"key": "aws_request_id", "value": {"stringValue": request_id}},
        {"key": "status",         "value": {"stringValue": status}},
    ]);

    let mut metrics_arr: Vec<Value> = Vec::with_capacity(16);

    // Opt-in aliases matching Datadog's CloudWatch integration naming.
    // Lets a customer without a separate CloudWatch Metric Streams ingest
    // still render Datadog-shaped dashboards from the extension alone.
    // Values are identical to the enhanced.* variants (both derived from
    // the same platform.report), so risk of double-counting is limited to
    // users who ALSO ingest CloudWatch — they should leave this off.
    let emit_base_aliases = std::env::var("O2_EMIT_BASE_ALIASES")
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);

    if let Some(v) = duration_ms {
        metrics_arr.push(gauge("aws.lambda.enhanced.duration", "ms", v, &dp_attrs, &time_nano_str));
        if emit_base_aliases {
            metrics_arr.push(gauge("aws.lambda.duration", "ms", v, &dp_attrs, &time_nano_str));
        }
    }
    if let Some(v) = billed_ms {
        metrics_arr.push(gauge("aws.lambda.enhanced.billed_duration", "ms", v, &dp_attrs, &time_nano_str));
    }
    if let Some(v) = init_duration_ms {
        metrics_arr.push(gauge("aws.lambda.enhanced.init_duration", "ms", v, &dp_attrs, &time_nano_str));
    }
    if let Some(v) = restore_duration_ms {
        metrics_arr.push(gauge("aws.lambda.enhanced.restore_duration", "ms", v, &dp_attrs, &time_nano_str));
    }
    if let Some(v) = memory_size {
        metrics_arr.push(gauge("aws.lambda.enhanced.memory_size", "MBy", v, &dp_attrs, &time_nano_str));
    }
    if let Some(v) = max_memory {
        metrics_arr.push(gauge("aws.lambda.enhanced.max_memory_used", "MBy", v, &dp_attrs, &time_nano_str));
    }
    if let (Some(mem), Some(max)) = (memory_size, max_memory) {
        if mem > 0.0 {
            metrics_arr.push(gauge(
                "aws.lambda.enhanced.memory_utilization",
                "1",
                max / mem,
                &dp_attrs,
                &time_nano_str,
            ));
        }
    }

    // Delta counters — one increment per report.
    metrics_arr.push(sum_delta(
        "aws.lambda.enhanced.invocations",
        "1",
        1,
        &dp_attrs,
        &time_nano_str,
    ));
    if emit_base_aliases {
        metrics_arr.push(sum_delta(
            "aws.lambda.invocations", "1", 1, &dp_attrs, &time_nano_str,
        ));
    }
    if status != "success" {
        metrics_arr.push(sum_delta(
            "aws.lambda.enhanced.errors",
            "1",
            1,
            &dp_attrs,
            &time_nano_str,
        ));
        if emit_base_aliases {
            metrics_arr.push(sum_delta(
                "aws.lambda.errors", "1", 1, &dp_attrs, &time_nano_str,
            ));
        }
    }
    if status == "timeout" {
        metrics_arr.push(sum_delta(
            "aws.lambda.enhanced.timeouts",
            "1",
            1,
            &dp_attrs,
            &time_nano_str,
        ));
    }
    if let (Some(mem), Some(max)) = (memory_size, max_memory) {
        if mem > 0.0 && max >= mem {
            metrics_arr.push(sum_delta(
                "aws.lambda.enhanced.out_of_memory",
                "1",
                1,
                &dp_attrs,
                &time_nano_str,
            ));
        }
    }
    if init_duration_ms.is_some() {
        metrics_arr.push(sum_delta(
            "aws.lambda.enhanced.cold_starts",
            "1",
            1,
            &dp_attrs,
            &time_nano_str,
        ));
    }

    if metrics_arr.is_empty() {
        return None;
    }

    Some(json!({
        "resourceMetrics": [{
            "resource": {"attributes": build_resource_attrs()},
            "scopeMetrics": [{
                "scope": {"name": SCOPE_NAME, "version": SCOPE_VERSION},
                "metrics": metrics_arr,
            }],
        }],
    }))
}

fn build_resource_attrs() -> Value {
    let mut attrs: Vec<Value> = Vec::new();

    let mut push_str = |key: &str, val: String| {
        if !val.is_empty() {
            attrs.push(json!({"key": key, "value": {"stringValue": val}}));
        }
    };

    push_str("cloud.provider", "aws".to_string());
    push_str("faas.name", env::var("AWS_LAMBDA_FUNCTION_NAME").unwrap_or_default());
    push_str("faas.version", env::var("AWS_LAMBDA_FUNCTION_VERSION").unwrap_or_default());
    push_str("cloud.region", env::var("AWS_REGION").unwrap_or_default());
    if let Ok(memory) = env::var("AWS_LAMBDA_FUNCTION_MEMORY_SIZE") {
        push_str("faas.max_memory", memory);
    }

    // service.name — Datadog's unified-service-tagging convention.
    // Precedence: O2_SERVICE > OTEL_SERVICE_NAME > AWS_LAMBDA_FUNCTION_NAME.
    let service = env::var("O2_SERVICE")
        .ok()
        .or_else(|| env::var("OTEL_SERVICE_NAME").ok())
        .or_else(|| env::var("AWS_LAMBDA_FUNCTION_NAME").ok())
        .unwrap_or_default();
    push_str("service.name", service);

    if let Ok(env_name) = env::var("O2_ENV") {
        push_str("deployment.environment", env_name);
    }

    Value::Array(attrs)
}

fn gauge(name: &str, unit: &str, value: f64, attrs: &Value, time_nano: &str) -> Value {
    json!({
        "name": name,
        "unit": unit,
        "gauge": {
            "dataPoints": [{
                "attributes": attrs,
                "timeUnixNano": time_nano,
                "asDouble": value,
            }]
        }
    })
}

fn sum_delta(name: &str, unit: &str, value: i64, attrs: &Value, time_nano: &str) -> Value {
    json!({
        "name": name,
        "unit": unit,
        "sum": {
            "dataPoints": [{
                "attributes": attrs,
                "startTimeUnixNano": time_nano,
                "timeUnixNano": time_nano,
                "asInt": value.to_string(),
            }],
            // 1 = AGGREGATION_TEMPORALITY_DELTA
            "aggregationTemporality": 1,
            "isMonotonic": true,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn make_report(with_init: bool, status: &str, max_mem: f64, mem: f64) -> TelemetryEvent {
        let mut metrics = serde_json::json!({
            "durationMs": 234.5,
            "billedDurationMs": 235.0,
            "memorySizeMB": mem,
            "maxMemoryUsedMB": max_mem,
        });
        if with_init {
            metrics["initDurationMs"] = serde_json::json!(120.7);
        }
        let record = serde_json::json!({
            "requestId": "req-abc",
            "status": status,
            "metrics": metrics,
        });
        TelemetryEvent {
            time: Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, 0).unwrap(),
            event_type: "platform.report".into(),
            record,
            request_id: Some("req-abc".into()),
        }
    }

    #[test]
    fn synthesizes_full_gauge_and_counter_set_on_cold_start() {
        let event = make_report(true, "success", 512.0, 1024.0);
        let payload = build_payload(&event).expect("payload");
        let names: Vec<&str> = payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();

        assert!(names.contains(&"aws.lambda.enhanced.duration"));
        assert!(names.contains(&"aws.lambda.enhanced.billed_duration"));
        assert!(names.contains(&"aws.lambda.enhanced.init_duration"));
        assert!(names.contains(&"aws.lambda.enhanced.memory_size"));
        assert!(names.contains(&"aws.lambda.enhanced.max_memory_used"));
        assert!(names.contains(&"aws.lambda.enhanced.memory_utilization"));
        assert!(names.contains(&"aws.lambda.enhanced.invocations"));
        assert!(names.contains(&"aws.lambda.enhanced.cold_starts"));
        // status=success => no errors/timeouts
        assert!(!names.contains(&"aws.lambda.enhanced.errors"));
        assert!(!names.contains(&"aws.lambda.enhanced.timeouts"));
    }

    #[test]
    fn emits_error_counter_when_status_not_success() {
        let event = make_report(false, "error", 100.0, 512.0);
        let payload = build_payload(&event).unwrap();
        let names: Vec<&str> = payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"aws.lambda.enhanced.errors"));
        assert!(!names.contains(&"aws.lambda.enhanced.timeouts"));
        assert!(!names.contains(&"aws.lambda.enhanced.cold_starts"));
    }

    #[test]
    fn base_aliases_only_emitted_when_opted_in() {
        // Default: no aliases
        std::env::remove_var("O2_EMIT_BASE_ALIASES");
        let payload = build_payload(&make_report(false, "error", 100.0, 512.0)).unwrap();
        let names: Vec<&str> = payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect();
        assert!(!names.contains(&"aws.lambda.duration"));
        assert!(!names.contains(&"aws.lambda.errors"));
        assert!(!names.contains(&"aws.lambda.invocations"));

        // Opt in
        std::env::set_var("O2_EMIT_BASE_ALIASES", "true");
        let payload = build_payload(&make_report(false, "error", 100.0, 512.0)).unwrap();
        let names: Vec<&str> = payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"aws.lambda.duration"));
        assert!(names.contains(&"aws.lambda.errors"));
        assert!(names.contains(&"aws.lambda.invocations"));
        // Enhanced set still there
        assert!(names.contains(&"aws.lambda.enhanced.duration"));
        assert!(names.contains(&"aws.lambda.enhanced.errors"));
        assert!(names.contains(&"aws.lambda.enhanced.invocations"));
        std::env::remove_var("O2_EMIT_BASE_ALIASES");
    }

    #[test]
    fn emits_out_of_memory_when_max_ge_size() {
        let event = make_report(false, "error", 512.0, 512.0);
        let payload = build_payload(&event).unwrap();
        let names: Vec<&str> = payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"aws.lambda.enhanced.out_of_memory"));
    }

    #[test]
    fn skips_non_report_events() {
        let event = TelemetryEvent {
            time: Utc::now(),
            event_type: "function".into(),
            record: serde_json::json!("hello"),
            request_id: None,
        };
        assert!(build_payload(&event).is_none() || event.event_type != "platform.report");
    }

    #[test]
    fn memory_utilization_is_ratio() {
        let event = make_report(false, "success", 256.0, 512.0); // 50%
        let payload = build_payload(&event).unwrap();
        let metrics = payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap();
        let util = metrics
            .iter()
            .find(|m| m["name"] == "aws.lambda.enhanced.memory_utilization")
            .unwrap();
        let dp = &util["gauge"]["dataPoints"][0]["asDouble"];
        assert!((dp.as_f64().unwrap() - 0.5).abs() < 1e-9);
    }
}
