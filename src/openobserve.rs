use anyhow::{anyhow, Result};
use chrono::Utc;
use reqwest::Client;
use std::cmp;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, warn};

use crate::config::Config;
use crate::telemetry::TelemetryEvent;

// Send JSON batch to OpenObserve with retry logic and exponential backoff
pub async fn send_batch_to_openobserve(
    client: &Client,
    config: &Config,
    json_batch: &[u8],
) -> Result<u64> {
    let url = config.openobserve_url();
    
    debug!("🌐 Making HTTP call to OpenObserve: {} bytes to {}", 
           json_batch.len(), url);
    
    // Parse the batch to count events for metrics
    let events_count = if let Ok(batch_str) = String::from_utf8(json_batch.to_vec()) {
        // Count events by counting commas + 1 (assuming valid JSON array)
        if batch_str.trim().starts_with('[') && batch_str.trim().ends_with(']') {
            batch_str.matches(',').count() as u64 + 1
        } else {
            1 // Single event
        }
    } else {
        1 // Default to 1 if we can't parse
    };
    
    let mut current_delay = config.initial_retry_delay_ms;
    let mut last_error = None;
    
    // Attempt initial request + retries
    for attempt in 0..=(config.max_retries) {
        let response_result = client
            .post(&url)
            .header("Authorization", &config.o2_authorization_header)
            .header("Content-Type", "application/json")
            .body(json_batch.to_vec())
            .send()
            .await;
        
        match response_result {
            Ok(response) => {
                let status = response.status();
                
                if status.is_success() {
                    // Consume response body for successful requests
                    let _response_text: String = (response.text().await).unwrap_or_default();
                    if attempt > 0 {
                        debug!("✅ Successfully sent batch of {} events to OpenObserve on retry attempt {} - Status: {}", 
                               events_count, attempt, status);
                    } else {
                        debug!("✅ Successfully sent batch of {} events to OpenObserve - Status: {}", 
                               events_count, status);
                    }
                    return Ok(events_count);
                } else {
                    // Server returned error status - safely consume response body
                    let error_text = match response.text().await {
                        Ok(text) => text,
                        Err(_) => format!("Status: {status} (response body unreadable)"),
                    };
                    let error_msg = format!("OpenObserve returned status {status}: {error_text}");
                    
                    // Check if this is a retryable error (5xx server errors are retryable, 4xx client errors are not)
                    let is_retryable = status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
                    
                    if !is_retryable || attempt >= config.max_retries {
                        error!("❌ FAILED to send batch to OpenObserve after {} attempts - Status: {}, Error: {}", 
                               attempt + 1, status, error_text);
                        return Err(anyhow!(error_msg));
                    }
                    
                    warn!("⚠️ Retry attempt {}/{} failed with retryable error - Status: {}, will retry in {}ms", 
                          attempt + 1, config.max_retries, status, current_delay);
                    last_error = Some(error_msg);
                }
            },
            Err(e) => {
                // Network/connection error
                let error_msg = format!("Request failed: {e}");
                
                if attempt >= config.max_retries {
                    error!("❌ FAILED to send batch to OpenObserve after {} attempts - Network error: {}", 
                           attempt + 1, e);
                    return Err(anyhow!(error_msg));
                }
                
                warn!("⚠️ Retry attempt {}/{} failed with network error - {}, will retry in {}ms", 
                      attempt + 1, config.max_retries, e, current_delay);
                last_error = Some(error_msg);
            }
        }
        
        // Wait before next retry (unless this was the last attempt)
        if attempt < config.max_retries {
            sleep(Duration::from_millis(current_delay)).await;
            
            // Exponential backoff: double the delay, capped at max_retry_delay_ms
            current_delay = cmp::min(current_delay * 2, config.max_retry_delay_ms);
        }
    }
    
    // This should never be reached, but just in case
    Err(anyhow!("All retry attempts exhausted: {}", 
                last_error.unwrap_or_else(|| "Unknown error".to_string())))
}

// Utility function to create a test event for health checks
pub fn create_test_event() -> TelemetryEvent {
    TelemetryEvent {
        time: Utc::now(),
        event_type: "extension".to_string(),
        record: serde_json::json!("OpenObserve Lambda Extension health check"),
        request_id: None,
    }
}