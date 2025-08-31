use anyhow::{anyhow, Result};
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{EnvFilter, fmt::format::Writer, fmt::FormatEvent, fmt::FormatFields};

mod config;
mod extension;
mod telemetry;
mod openobserve;

use config::Config;
use extension::{ExtensionClient, NextEventResponse, FlushingStrategy};
use telemetry::{TelemetrySubscriber};

const EXTENSION_NAME: &str = "o2-lambda-extension";
const TELEMETRY_SUBSCRIBER_PORT: u16 = 8080;

struct ExtensionMetrics {
    start_time: Instant,
    invocations_processed: u64,
    logs_processed: u64,
}

impl ExtensionMetrics {
    fn new() -> Self {
        Self {
            start_time: Instant::now(),
            invocations_processed: 0,
            logs_processed: 0,
        }
    }

    fn log_stats(&self) {
        let uptime = self.start_time.elapsed();
        info!(
            "Extension stats: uptime={:.2}s, invocations={}, logs={}",
            uptime.as_secs_f64(),
            self.invocations_processed,
            self.logs_processed,
        );
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    
    // Handle CLI commands before initializing logging for cleaner output
    if args.len() > 1 {
        match args[1].as_str() {
            "--health-check" | "-h" => {
                init_logging();
                
                let config = Config::from_env().map_err(|e| {
                    error!("Configuration error: {}", e);
                    e
                })?;
                
                return health_check(&config).await;
            }
            "--version" | "-v" => {
                println!("{} v{}", EXTENSION_NAME, env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" => {
                print_help();
                return Ok(());
            }
            unknown => {
                eprintln!("Unknown command: {unknown}");
                print_help();
                return Err(anyhow!("Invalid command line argument"));
            }
        }
    }

    // Normal extension mode
    init_logging();

    // Load configuration
    let config = Arc::new(Config::from_env().map_err(|e| {
        error!("Configuration error: {}", e);
        e
    })?);

    
    // Log startup sequence

    // Initialize extension metrics
    let mut metrics = ExtensionMetrics::new();

    // Run the extension
    match run_extension(config, &mut metrics).await {
        Ok(_) => {
            metrics.log_stats();
            Ok(())
        }
        Err(e) => {
            error!("Extension failed: {}", e);
            metrics.log_stats();
            Err(e)
        }
    }
}

async fn run_extension(config: Arc<Config>, metrics: &mut ExtensionMetrics) -> Result<()> {
    // Create extension client
    let mut extension_client = ExtensionClient::new(EXTENSION_NAME.to_string());
    
    // We'll set telemetry components after creating them

    // Register extension
    let registration = extension_client.register().await?;

    let extension_id = registration.extension_id.clone();

    // Set up telemetry components
    
    // Create aggregator
    let aggregator = Arc::new(std::sync::Mutex::new(
        telemetry::TelemetryAggregator::new(
            config.max_buffer_size_bytes(),
            100, // max batch entries
        )
    ));

    // Set up telemetry subscriber
    let mut telemetry_subscriber = TelemetrySubscriber::new(TELEMETRY_SUBSCRIBER_PORT, Arc::clone(&aggregator));
    
    telemetry_subscriber.start().await?;
    
    telemetry_subscriber.subscribe_to_telemetry_api(&extension_id).await?;

    // Note: Using Telemetry API to capture logs, metrics, and traces
    // AWS Lambda allows only one subscription per extension
    
    // Note: No async OpenObserve client needed - using synchronous flush in extension.rs
    
    // Set telemetry components in extension client for SHUTDOWN handling
    extension_client.set_telemetry_components(
        Arc::clone(&aggregator),
        Arc::clone(&config),
    );

    // Main extension lifecycle loop - SHUTDOWN flush now happens in extension.rs
    let result = extension_lifecycle_loop(
        &mut extension_client,
        metrics,
    )
    .await;

    // Simplified shutdown - the flush already happened during SHUTDOWN event
    
    // Stop accepting new telemetry requests
    telemetry_subscriber.shutdown().await;
    
    // Give time for final processing
    tokio::time::sleep(Duration::from_millis(200)).await;

    result
}

async fn extension_lifecycle_loop(
    extension_client: &mut ExtensionClient,
    metrics: &mut ExtensionMetrics,
) -> Result<()> {

    loop {
        // Get the next event from Lambda
        let event = extension_client.next_event().await?;

        match event {
            NextEventResponse::Invoke { 
                request_id, 
                deadline_ms, 
                ..
            } => {
                metrics.invocations_processed += 1;
                

                // Handle the invoke event  
                handle_invoke_event(
                    extension_client,
                    metrics,
                    &request_id,
                    deadline_ms,
                ).await?;
            }
            NextEventResponse::Shutdown { 
                deadline_ms 
            } => {
                // Flush already happened in extension.rs during next_event()
                debug!("🔄 SHUTDOWN event processed by extension, breaking lifecycle loop");
                handle_shutdown_event(metrics, deadline_ms).await?;
                break;
            }
        }
    }

    Ok(())
}

async fn handle_invoke_event(
    extension_client: &mut ExtensionClient,
    _metrics: &mut ExtensionMetrics,
    request_id: &str,
    _deadline_ms: u64,
) -> Result<()> {
    let invoke_start = Instant::now();
    
    debug!("Processing INVOKE event for {}", request_id);
    
    // Just wait a bit to simulate function execution
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Apply smart flushing strategy
    let events_flushed = match extension_client.current_strategy {
        FlushingStrategy::EndOfInvocation => {
            // Low-frequency: flush at end of each invocation
            extension_client.flush_end_of_invocation().await.unwrap_or_else(|e| {
                warn!("⚠️ End-of-invocation flush failed: {}", e);
                0
            })
        },
        FlushingStrategy::Periodic => {
            // Long-running: periodic flush if interval elapsed
            extension_client.flush_periodic().await.unwrap_or_else(|e| {
                warn!("⚠️ Periodic flush failed: {}", e);
                0
            })
        },
        FlushingStrategy::Continuous => {
            // High-frequency: continuous flushing handled by background task
            0 // No action needed, background task handles flushing
        }
    };
    
    if events_flushed > 0 {
        debug!("📤 Flushed {} events using {:?} strategy", events_flushed, extension_client.current_strategy);
    }

    let invoke_duration = invoke_start.elapsed();
    debug!(
        "Completed INVOKE processing for {} in {:.2}ms",
        request_id,
        invoke_duration.as_millis()
    );

    Ok(())
}

async fn handle_shutdown_event(
    _metrics: &mut ExtensionMetrics,
    _deadline_ms: u64,
) -> Result<()> {
    let shutdown_start = Instant::now();
    
    // Flush already completed in extension.rs
    debug!("📊 Shutdown event handling complete");

    let _shutdown_duration = shutdown_start.elapsed();

    Ok(())
}

// Custom formatter that prefixes all log messages
struct OpenObserveFormatter;

impl<S, N> FormatEvent<S, N> for OpenObserveFormatter
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &tracing_subscriber::fmt::FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        // Write the prefix
        write!(writer, "OpenObserve extension - ")?;
        
        // Write the log level with color
        let level = *event.metadata().level();
        let level_color = match level {
            tracing::Level::ERROR => "\x1b[31m", // Red
            tracing::Level::WARN => "\x1b[33m",  // Yellow
            tracing::Level::INFO => "\x1b[32m",  // Green
            tracing::Level::DEBUG => "\x1b[34m", // Blue
            tracing::Level::TRACE => "\x1b[35m", // Magenta
        };
        write!(writer, "{level_color}{level}:\x1b[0m ")?;
        
        // Format and write the message
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

fn init_logging() {
    let log_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "INFO".to_string());
    
    // Create filter that suppresses debug messages from HTTP clients
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| {
            EnvFilter::try_new(format!(
                "{log_level},hyper=warn,reqwest=warn,h2=warn,rustls=warn"
            ))
        })
        .unwrap_or_else(|_| {
            EnvFilter::new("info")
                .add_directive("hyper=warn".parse().unwrap())
                .add_directive("reqwest=warn".parse().unwrap())
                .add_directive("h2=warn".parse().unwrap())
                .add_directive("rustls=warn".parse().unwrap())
        });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(false)
        .with_line_number(false)
        .without_time()
        .event_format(OpenObserveFormatter)
        .init();

}

fn print_help() {
    println!("{} v{}", EXTENSION_NAME, env!("CARGO_PKG_VERSION"));
    println!("AWS Lambda Extension for forwarding logs to OpenObserve");
    println!();
    println!("USAGE:");
    println!("    {EXTENSION_NAME} [COMMAND]");
    println!();
    println!("COMMANDS:");
    println!("    --health-check, -h    Run health check (test config and OpenObserve connectivity)");
    println!("    --version, -v         Show version information");
    println!("    --help               Show this help message");
    println!();
    println!("ENVIRONMENT VARIABLES (for health check and normal operation):");
    println!("    Required:");
    println!("        O2_ORGANIZATION_ID        OpenObserve organization ID");
    println!("        O2_AUTHORIZATION_HEADER   Authorization header (e.g., \"Basic <base64>\")");
    println!();
    println!("    Optional:");
    println!("        O2_ENDPOINT              OpenObserve API endpoint (default: https://api.openobserve.ai)");
    println!("        O2_STREAM               Log stream name (default: default)");
    println!("        LOG_LEVEL               Log level (default: INFO)");
    println!();
    println!("EXAMPLES:");
    println!("    # Run health check");
    println!("    export O2_ORGANIZATION_ID=my_org");
    println!("    export O2_AUTHORIZATION_HEADER=\"Basic $(echo -n 'user:pass' | base64)\"");
    println!("    {EXTENSION_NAME} --health-check");
    println!();
    println!("    # Show version");
    println!("    {EXTENSION_NAME} --version");
    println!();
    println!("For more information, visit: https://docs.openobserve.ai");
}

// Health check function for monitoring
pub async fn health_check(config: &Config) -> Result<()> {
    
    // Test configuration
    config.validate().map_err(|e| anyhow!("Config validation failed: {}", e))?;
    
    // Test OpenObserve connectivity
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(10000))
        .connect_timeout(Duration::from_millis(3000))
        .danger_accept_invalid_certs(true) // For testing with mock servers
        .local_address(None) // Let system choose
        .build()?;
    
    let test_event = openobserve::create_test_event();
    let url = config.openobserve_url();
    
    let response = client
        .post(&url)
        .header("Authorization", &config.o2_authorization_header)
        .header("Content-Type", "application/json")
        .json(&[test_event])
        .send()
        .await?;
    
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(anyhow!(
            "Health check failed - OpenObserve returned status: {}", 
            status
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_extension_metrics() {
        let mut metrics = ExtensionMetrics::new();
        
        assert_eq!(metrics.invocations_processed, 0);
        assert_eq!(metrics.logs_processed, 0);
        // No flush operations in simplified implementation
        
        metrics.invocations_processed += 1;
        assert_eq!(metrics.invocations_processed, 1);
    }
    
    #[tokio::test]
    async fn test_health_check_with_invalid_config() {
        // Test with invalid config
        let config = Config {
            o2_endpoint: "invalid-url".to_string(),
            o2_organization_id: "test".to_string(),
            o2_authorization_header: "test".to_string(),
            ..Default::default()
        };
        
        let result = health_check(&config).await;
        assert!(result.is_err());
    }
}