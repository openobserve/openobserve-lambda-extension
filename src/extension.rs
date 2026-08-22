use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use tokio::time::timeout;

use crate::telemetry::TelemetryAggregator;
use crate::config::Config;
use crate::otlp_receiver::{SpanBuffer, MetricBuffer, flush_spans, flush_metrics};

const LAMBDA_EXTENSION_IDENTIFIER_HEADER: &str = "Lambda-Extension-Identifier";
const LAMBDA_EXTENSION_NAME_HEADER: &str = "Lambda-Extension-Name";
const LAMBDA_EXTENSION_ACCEPT_FEATURE_HEADER: &str = "Lambda-Extension-Accept-Feature";
const LAMBDA_EXTENSION_FEATURES: &str = "accountId";

// Flushing strategy thresholds (as described in README)
const HIGH_FREQUENCY_THRESHOLD: f64 = 10.0; // ≥10 invocations/minute
const LONG_RUNNING_THRESHOLD_SECS: u64 = 30; // >30s since last invocation
const PERIODIC_FLUSH_INTERVAL_SECS: u64 = 5; // Periodic flush every 5 seconds

#[derive(Debug, Clone, PartialEq)]
pub enum FlushingStrategy {
    EndOfInvocation,  // Low-frequency: <10 invocations/minute
    Continuous,       // High-frequency: ≥10 invocations/minute  
    Periodic,         // Long-running: >30s since last invocation
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterResponse {
    #[serde(skip)]
    pub extension_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "eventType")]
pub enum NextEventResponse {
    #[serde(rename = "INVOKE")]
    Invoke {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "deadlineMs")]
        deadline_ms: u64,
    },
    #[serde(rename = "SHUTDOWN")]
    Shutdown {
        #[serde(rename = "deadlineMs")]
        deadline_ms: u64,
    },
}



pub struct ExtensionClient {
    client: Client,
    extension_name: String,
    runtime_api_endpoint: String,
    extension_id: Option<String>,
    invocation_count: u64,
    last_invocation_time: Instant,
    recent_invocations: VecDeque<Instant>,
    aggregator: Option<Arc<Mutex<TelemetryAggregator>>>,
    config: Option<Arc<Config>>,
    span_buffer: Option<SpanBuffer>,
    metric_buffer: Option<MetricBuffer>,
    pub current_strategy: FlushingStrategy,
    last_periodic_flush: Instant,
    continuous_flush_task: Option<tokio::task::JoinHandle<()>>,
}

impl ExtensionClient {
    pub fn new(extension_name: String) -> Self {
        let runtime_api_endpoint = std::env::var("AWS_LAMBDA_RUNTIME_API")
            .unwrap_or_else(|_| "localhost:9001".to_string());
        
        let now = Instant::now();
        Self {
            client: Client::new(),
            extension_name,
            runtime_api_endpoint,
            extension_id: None,
            invocation_count: 0,
            last_invocation_time: now,
            recent_invocations: VecDeque::new(),
            aggregator: None,
            config: None,
            span_buffer: None,
            metric_buffer: None,
            current_strategy: FlushingStrategy::EndOfInvocation, // Start with safe default
            last_periodic_flush: now,
            continuous_flush_task: None,
        }
    }
    
    pub fn set_telemetry_components(
        &mut self,
        aggregator: Arc<Mutex<TelemetryAggregator>>,
        config: Arc<Config>,
        span_buffer: SpanBuffer,
        metric_buffer: MetricBuffer,
    ) {
        self.aggregator = Some(aggregator);
        self.config = Some(config);
        self.span_buffer = Some(span_buffer);
        self.metric_buffer = Some(metric_buffer);
    }

    /// Determine the appropriate flushing strategy based on invocation patterns
    fn determine_flushing_strategy(&self) -> FlushingStrategy {
        let now = Instant::now();
        
        // Check for long-running (>30s since last invocation)
        if now.duration_since(self.last_invocation_time).as_secs() > LONG_RUNNING_THRESHOLD_SECS {
            return FlushingStrategy::Periodic;
        }
        
        // Calculate invocations per minute in the last 5 minutes
        let recent_invocations_count = self.recent_invocations.len() as f64;
        let invocations_per_minute = if recent_invocations_count > 0.0 {
            // Calculate actual timespan covered by recent invocations
            if let (Some(&oldest), Some(&newest)) = (self.recent_invocations.front(), self.recent_invocations.back()) {
                let timespan_minutes = newest.duration_since(oldest).as_secs_f64() / 60.0;
                if timespan_minutes > 0.0 {
                    recent_invocations_count / timespan_minutes
                } else {
                    recent_invocations_count // If all invocations are in same second, assume high frequency
                }
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Decide strategy based on frequency
        if invocations_per_minute >= HIGH_FREQUENCY_THRESHOLD {
            FlushingStrategy::Continuous
        } else {
            FlushingStrategy::EndOfInvocation
        }
    }

    /// Update the flushing strategy and handle transitions
    async fn update_flushing_strategy(&mut self) -> Result<()> {
        let new_strategy = self.determine_flushing_strategy();
        
        if new_strategy != self.current_strategy {
            info!("🔄 Flushing strategy changed: {:?} → {:?}", self.current_strategy, new_strategy);
            
            // Handle strategy transitions
            match (&self.current_strategy, &new_strategy) {
                (FlushingStrategy::Continuous, _) => {
                    // Stop continuous flushing task
                    if let Some(task) = self.continuous_flush_task.take() {
                        task.abort();
                        debug!("🛑 Stopped continuous flush task");
                    }
                },
                (_, FlushingStrategy::Continuous) => {
                    // Start continuous flushing task
                    self.start_continuous_flush_task().await?;
                },
                _ => {}
            }
            
            self.current_strategy = new_strategy;
        }
        
        Ok(())
    }

    /// Start continuous flushing task for high-frequency functions
    async fn start_continuous_flush_task(&mut self) -> Result<()> {
        if let (Some(aggregator), Some(config)) = (self.aggregator.clone(), self.config.clone()) {
            let aggregator_clone = Arc::clone(&aggregator);
            let config_clone = Arc::clone(&config);
            let span_buffer_clone = self.span_buffer.clone();
            let metric_buffer_clone = self.metric_buffer.clone();

            let task = tokio::spawn(async move {
                debug!("🚀 Started continuous flush task");
                let mut interval = tokio::time::interval(Duration::from_secs(PERIODIC_FLUSH_INTERVAL_SECS));

                loop {
                    interval.tick().await;

                    // Wrap logs + spans + metrics flushes together. On a cold TLS
                    // handshake each POST can take ~200–400 ms; sequential across
                    // three signals easily exceeds 500 ms. 3 s gives headroom
                    // without letting a hung endpoint stall the flush task.
                    let flush_result = timeout(
                        Duration::from_millis(3000),
                        Self::flush_telemetry_async(
                            &aggregator_clone,
                            &config_clone,
                            span_buffer_clone.as_ref(),
                            metric_buffer_clone.as_ref(),
                        )
                    ).await;

                    match flush_result {
                        Ok(Ok(events_sent)) if events_sent > 0 => {
                            debug!("📤 Continuous flush: {} events sent", events_sent);
                        },
                        Ok(Err(e)) => {
                            warn!("⚠️ Continuous flush failed: {}", e);
                        },
                        Err(_) => {
                            warn!("⚠️ Continuous flush timed out");
                        },
                        _ => {} // No events to send, normal case
                    }
                }
            });

            self.continuous_flush_task = Some(task);
            info!("✅ Continuous flush task started");
        }

        Ok(())
    }

    /// Perform end-of-invocation flush for low-frequency functions
    pub async fn flush_end_of_invocation(&self) -> Result<u64> {
        if let (Some(aggregator), Some(config)) = (&self.aggregator, &self.config) {
            debug!("📤 End-of-invocation flush");
            self.flush_telemetry_synchronously(aggregator, config).await
        } else {
            Ok(0)
        }
    }

    /// Perform periodic flush for long-running functions  
    pub async fn flush_periodic(&mut self) -> Result<u64> {
        let now = Instant::now();
        if now.duration_since(self.last_periodic_flush).as_secs() >= PERIODIC_FLUSH_INTERVAL_SECS {
            self.last_periodic_flush = now;
            
            if let (Some(aggregator), Some(config)) = (&self.aggregator, &self.config) {
                debug!("📤 Periodic flush");
                self.flush_telemetry_synchronously(aggregator, config).await
            } else {
                Ok(0)
            }
        } else {
            Ok(0)
        }
    }

    /// Async flush method for continuous flushing (non-blocking)
    async fn flush_telemetry_async(
        aggregator: &Arc<Mutex<TelemetryAggregator>>,
        config: &Arc<Config>,
        span_buffer: Option<&SpanBuffer>,
        metric_buffer: Option<&MetricBuffer>,
    ) -> Result<u64> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.request_timeout_ms))
            .build()
            .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

        let mut total: u64 = 0;

        let batch = {
            let mut guard = aggregator.lock().await;
            guard.get_batch()
        };
        // A log failure MUST NOT block span/metric flushing.
        let mut log_err: Option<anyhow::Error> = None;
        if !batch.is_empty() {
            match crate::openobserve::send_batch_to_openobserve(&client, config, &batch).await {
                Ok(n) => { total += n; debug!("✅ Async log flush: {} events", n); }
                Err(e) => { warn!("❌ Async log flush failed: {}", e); log_err = Some(e); }
            }
        }
        // Silence unused warning if neither span nor metric buffer present
        let _ = &log_err;

        if let Some(span_buf) = span_buffer {
            match flush_spans(&client, config, span_buf).await {
                Ok(n) => { total += n; if n > 0 { debug!("🔍 Async span flush: {} batches", n); } }
                Err(e) => warn!("⚠️ Async span flush failed: {}", e),
            }
        }

        if let Some(metric_buf) = metric_buffer {
            match flush_metrics(&client, config, metric_buf).await {
                Ok(n) => { total += n; if n > 0 { debug!("📊 Async metric flush: {} batches", n); } }
                Err(e) => warn!("⚠️ Async metric flush failed: {}", e),
            }
        }

        if let Some(e) = log_err { return Err(e); }
        Ok(total)
    }
    
    pub async fn register(&mut self) -> Result<RegisterResponse> {
        let url = format!("http://{}/2020-01-01/extension/register", self.runtime_api_endpoint);
        
        let register_request = RegisterRequest {
            events: vec!["INVOKE".to_string(), "SHUTDOWN".to_string()],
        };
        
        
        let response = self
            .client
            .post(&url)
            .header(LAMBDA_EXTENSION_NAME_HEADER, &self.extension_name)
            .header(LAMBDA_EXTENSION_ACCEPT_FEATURE_HEADER, LAMBDA_EXTENSION_FEATURES)
            .json(&register_request)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to register extension: {}", e))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Extension registration failed with status {}: {}", 
                status, text
            ));
        }
        
        // Extract extension ID from headers
        let extension_id = response
            .headers()
            .get(LAMBDA_EXTENSION_IDENTIFIER_HEADER)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| anyhow!("Extension ID not found in response headers"))?
            .to_string();
        
        let mut register_response: RegisterResponse = response
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse registration response: {}", e))?;
        
        register_response.extension_id = extension_id.clone();
        self.extension_id = Some(extension_id);
        
        
        Ok(register_response)
    }
    
    pub async fn next_event(&mut self) -> Result<NextEventResponse> {
        let extension_id = self.extension_id.as_ref()
            .ok_or_else(|| anyhow!("Extension not registered"))?;
        
        let url = format!(
            "http://{}/2020-01-01/extension/event/next", 
            self.runtime_api_endpoint
        );
        
        
        let response = self
            .client
            .get(&url)
            .header(LAMBDA_EXTENSION_IDENTIFIER_HEADER, extension_id)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to get next event: {}", e))?;
        
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Next event request failed with status {}: {}", 
                status, text
            ));
        }
        
        let event: NextEventResponse = response
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse next event response: {}", e))?;
        
        match &event {
            NextEventResponse::Invoke { request_id: _, deadline_ms: _ } => {
                let now = std::time::Instant::now();
                self.invocation_count += 1;
                self.last_invocation_time = now;
                
                // Track recent invocations for frequency calculation
                self.recent_invocations.push_back(now);
                
                // Keep only invocations from the last 5 minutes for frequency calculation
                let five_minutes_ago = now - std::time::Duration::from_secs(300);
                while let Some(&front_time) = self.recent_invocations.front() {
                    if front_time < five_minutes_ago {
                        self.recent_invocations.pop_front();
                    } else {
                        break;
                    }
                }
                
                // Update flushing strategy based on current patterns
                if let Err(e) = self.update_flushing_strategy().await {
                    warn!("⚠️ Failed to update flushing strategy: {}", e);
                }
                
            },
            NextEventResponse::Shutdown { deadline_ms: _ } => {
                debug!("🔄 SHUTDOWN event received - triggering immediate synchronous flush");
                
                if let (Some(aggregator), Some(config)) = (&self.aggregator, &self.config) {
                    match self.flush_telemetry_synchronously(aggregator, config).await {
                        Ok(events_sent) => debug!("✅ Emergency flush completed: {} events sent", events_sent),
                        Err(e) => debug!("❌ Emergency flush failed: {}", e),
                    }
                } else {
                    debug!("⚠️ SHUTDOWN received but telemetry components not set");
                }
            },
        }
        
        Ok(event)
    }
    
    async fn flush_telemetry_synchronously(
        &self,
        aggregator: &Arc<Mutex<TelemetryAggregator>>,
        config: &Arc<Config>,
    ) -> Result<u64> {
        let mut total_events = 0;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(config.request_timeout_ms))
            .build()
            .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

        // Flush logs — a log-send failure MUST NOT abort span/metric flushing below.
        // Break out on the first error so we don't retry the same failing batch in
        // a hot loop, then continue to spans/metrics.
        let mut log_err: Option<anyhow::Error> = None;
        loop {
            let batch = {
                let mut guard = aggregator.lock().await;
                guard.get_batch()
            };
            if batch.is_empty() { break; }
            match crate::openobserve::send_batch_to_openobserve(&client, config, &batch).await {
                Ok(n) => total_events += n,
                Err(e) => { debug!("❌ Log batch failed: {}", e); log_err = Some(e); break; }
            }
        }

        // Flush spans (if OTLP receiver is enabled)
        if let Some(span_buf) = &self.span_buffer {
            match flush_spans(&client, config, span_buf).await {
                Ok(n) => { total_events += n; debug!("🔍 Flushed {} span batches", n); }
                Err(e) => debug!("⚠️ Span flush failed: {}", e),
            }
        }

        // Flush metrics (if OTLP receiver is enabled)
        if let Some(metric_buf) = &self.metric_buffer {
            match flush_metrics(&client, config, metric_buf).await {
                Ok(n) => { total_events += n; debug!("📊 Flushed {} metric batches", n); }
                Err(e) => debug!("⚠️ Metric flush failed: {}", e),
            }
        }

        debug!("🎉 Flush completed: {} total events sent", total_events);
        // If logs failed but spans/metrics succeeded, surface the log error at the
        // top for retry-strategy logic in the caller; total_events still reflects
        // what did make it out.
        if let Some(e) = log_err { return Err(e); }
        Ok(total_events)
    }
    
    
}



#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_extension_client_creation() {
        let client = ExtensionClient::new("test-extension".to_string());
        assert_eq!(client.extension_name, "test-extension");
        assert_eq!(client.invocation_count, 0);
    }
}