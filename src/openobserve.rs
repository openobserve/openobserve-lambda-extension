use anyhow::{anyhow, Result};
use chrono::Utc;
use reqwest::Client;
use tracing::{debug, error};

use crate::config::Config;
use crate::telemetry::TelemetryEvent;

// Send JSON batch to OpenObserve - batch is already a JSON array
pub async fn send_batch_to_openobserve(
    client: &Client,
    config: &Config,
    json_batch: &[u8],
) -> Result<u64> {
    let url = config.openobserve_url();
    
    debug!("🌐 Making HTTP call to OpenObserve: {} bytes to {}\nData: {}", 
           json_batch.len(), url, String::from_utf8_lossy(json_batch));
    
    
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
    
    let response = client
        .post(&url)
        .header("Authorization", &config.o2_authorization_header)
        .header("Content-Type", "application/json")
        .body(json_batch.to_vec())
        .send()
        .await
        .map_err(|e| anyhow!("Request failed: {}", e))?;
    
    let status = response.status();

    // debug!("📡 OpenObserve HTTP Response - Status: {}", status);
    
    if status.is_success() {
        let _response_text = response.text().await.unwrap_or_default();
        debug!("✅ Successfully sent batch of {} to OpenObserve - Status: {}", events_count, status);
        Ok(events_count)
    } else {
        let error_text = response.text().await.unwrap_or_default();
        error!("❌ FAILED to send batch to OpenObserve - Status: {}, Error: {}", status, error_text);
        Err(anyhow!(
            "OpenObserve request failed with status {}: {}", 
            status, 
            error_text
        ))
    }
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