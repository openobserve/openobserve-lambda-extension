use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use http::{Request, Response, StatusCode};
use hyper::{body, Body, Server};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tracing::error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub time: DateTime<Utc>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub record: serde_json::Value,
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

// aggregator - exactly like their implementation
pub struct TelemetryAggregator {
    messages: VecDeque<String>,
    buffer: Vec<u8>,
    max_content_size_bytes: usize,
    max_batch_entries_size: usize,
}

impl TelemetryAggregator {
    pub fn new(max_content_size_bytes: usize, max_batch_entries_size: usize) -> Self {
        Self {
            messages: VecDeque::new(),
            buffer: Vec::with_capacity(max_content_size_bytes),
            max_content_size_bytes,
            max_batch_entries_size,
        }
    }

    // add a batch of events immediately
    pub fn add_batch(&mut self, events: Vec<TelemetryEvent>) {
        for event in events {
            // Convert to OpenObserve format: add _timestamp and remove time
            let mut event_json = serde_json::json!({
                "_timestamp": event.time.timestamp_micros(),
                "record": event.record,
                "type": event.event_type
            });
            
            // Add requestId if present
            if let Some(request_id) = event.request_id {
                event_json["requestId"] = serde_json::Value::String(request_id);
            }
            
            // Serialize to JSON string
            if let Ok(json_str) = serde_json::to_string(&event_json) {
                self.messages.push_back(json_str);
            }
        }
    }

    // returns JSON array bytes
    pub fn get_batch(&mut self) -> Vec<u8> {
        self.buffer.extend(b"[");

        // Fill the batch with events from the messages
        for _ in 0..self.max_batch_entries_size {
            if let Some(event_json) = self.messages.pop_front() {
                // Check if the buffer will be full after adding the event
                if self.buffer.len() + event_json.len() > self.max_content_size_bytes {
                    // Put the event back in the queue
                    self.messages.push_front(event_json);
                    break;
                }

                self.buffer.extend(event_json.as_bytes());
                self.buffer.extend(b",");
            } else {
                break;
            }
        }

        // Make sure we added at least one element
        if self.buffer.len() > 1 {
            // Remove the last comma and close bracket
            self.buffer.pop();
            self.buffer.extend(b"]");
        } else {
            // No elements, remove opening bracket
            self.buffer.pop();
        }

        std::mem::take(&mut self.buffer)
    }

}

// Note: TelemetryProcessor removed - events now added directly to aggregator
// Note: TelemetryFlusher removed - using synchronous flush in extension.rs

pub struct TelemetrySubscriber {
    port: u16,
    aggregator: Arc<Mutex<TelemetryAggregator>>,
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TelemetrySubscriber {
    pub fn new(port: u16, aggregator: Arc<Mutex<TelemetryAggregator>>) -> Self {
        Self {
            port,
            aggregator,
            server_handle: None,
        }
    }
    
    pub async fn start(&mut self) -> Result<()> {
        let addr = SocketAddr::from(([0, 0, 0, 0], self.port));
        let aggregator = Arc::clone(&self.aggregator);
        
        let make_svc = hyper::service::make_service_fn(move |_conn| {
            let aggregator = Arc::clone(&aggregator);
            async move {
                Ok::<_, Infallible>(hyper::service::service_fn(move |req| {
                    handle_telemetry_request(req, Arc::clone(&aggregator))
                }))
            }
        });
        
        let server = Server::bind(&addr).serve(make_svc);
        
        
        let server_handle = tokio::spawn(async move {
            if let Err(e) = server.await {
                error!("❌ Telemetry subscriber server error: {}", e);
            }
        });
        
        self.server_handle = Some(server_handle);
        
        Ok(())
    }
    
    pub async fn subscribe_to_telemetry_api(&self, extension_id: &str) -> Result<()> {
        let runtime_api_endpoint = std::env::var("AWS_LAMBDA_RUNTIME_API")
            .unwrap_or_else(|_| "localhost:9001".to_string());
        
        let url = format!("http://{runtime_api_endpoint}/2022-07-01/telemetry");
        
        let subscription = serde_json::json!({
            "schemaVersion": "2022-12-13",
            "destination": {
                "protocol": "HTTP",
                "URI": format!("http://sandbox.localdomain:{}", self.port)
            },
            "types": ["platform", "function", "extension"],
            "buffering": {
                "maxBytes": 262144, // maxBytes should be between 262144 and 10485760
                "maxItems": 1000, // maxItems should be between 1000 and 10000
                "timeoutMs": 25 // mimimum is 25ms
            }
        });
        
        let client = reqwest::Client::new();
        
        
        let response = client
            .put(&url)
            .header("Lambda-Extension-Identifier", extension_id)
            .json(&subscription)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to subscribe to Telemetry API: {}", e))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Telemetry API subscription failed with status {}: {}", 
                status, text
            ));
        }
        
        Ok(())
    }
    
    pub async fn shutdown(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
    }
}

async fn handle_telemetry_request(
    req: Request<Body>,
    aggregator: Arc<Mutex<TelemetryAggregator>>,
) -> Result<Response<Body>, Infallible> {
    // debug!("🔥 TELEMETRY REQUEST RECEIVED! Method: {}, URI: {}", req.method(), req.uri());
    
    match req.method() {
        &hyper::Method::POST => {
            match process_telemetry_batch(req, aggregator).await {
                Ok(_) => {
                    let response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("OK"))
                        .unwrap();
                    Ok(response)
                }
                Err(e) => {
                    error!("❌ Error processing telemetry batch: {}", e);
                    let response = Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from("Internal Server Error"))
                        .unwrap();
                    Ok(response)
                }
            }
        }
        _ => {
            let response = Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .body(Body::from("Method Not Allowed"))
                .unwrap();
            Ok(response)
        }
    }
}

async fn process_telemetry_batch(
    req: Request<Body>,
    aggregator: Arc<Mutex<TelemetryAggregator>>,
) -> Result<()> {
    let body_bytes = body::to_bytes(req.into_body())
        .await
        .map_err(|e| anyhow!("Failed to read request body: {}", e))?;
    
    let body_str = String::from_utf8(body_bytes.to_vec())
        .map_err(|e| anyhow!("Invalid UTF-8 in request body: {}", e))?;
    
    
    // Parse telemetry events
    let telemetry_events: Vec<TelemetryEvent> = serde_json::from_str(&body_str)
        .map_err(|e| {
            error!("Failed to parse telemetry events: {}", e);
            anyhow!("Failed to parse telemetry events: {}", e)
        })?;
    
    // Add events directly to aggregator
    {
        let mut aggregator_guard = aggregator.lock().expect("lock poisoned");
        aggregator_guard.add_batch(telemetry_events);
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_telemetry_aggregator() {
        let mut aggregator = TelemetryAggregator::new(1024, 10);
        
        let events = vec![
            TelemetryEvent {
                time: Utc::now(),
                event_type: "function".to_string(),
                record: serde_json::json!("test log"),
                request_id: None,
            }
        ];
        
        aggregator.add_batch(events);
        let batch = aggregator.get_batch();
        assert!(!batch.is_empty());
        
        // Should be JSON array
        let batch_str = String::from_utf8(batch).unwrap();
        assert!(batch_str.starts_with('['));
        assert!(batch_str.ends_with(']'));
    }
    
    #[test]
    fn test_telemetry_event_serialization() {
        let event = TelemetryEvent {
            time: Utc::now(),
            event_type: "function".to_string(),
            record: serde_json::json!("Test telemetry message"),
            request_id: Some("test-request-id".to_string()),
        };
        
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"function\""));
        assert!(json.contains("\"record\":\"Test telemetry message\""));
    }
}