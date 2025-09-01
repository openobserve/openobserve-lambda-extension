#![allow(dead_code, unused_imports)]

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use serde_json::Value;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

// Re-export for easier access in tests
pub use test_utils::TestEnvironment;

/// Mock OpenObserve server for testing
pub struct MockOpenObserveServer {
    pub port: u16,
    pub requests_received: Arc<AtomicUsize>,
    pub last_request: Arc<RwLock<Option<MockRequest>>>,
    pub response_status: Arc<RwLock<StatusCode>>,
    pub server_handle: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Debug, Clone)]
pub struct MockRequest {
    pub method: String,
    pub uri: String,
    pub headers: std::collections::HashMap<String, String>,
    pub body: String,
}

impl MockOpenObserveServer {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            requests_received: Arc::new(AtomicUsize::new(0)),
            last_request: Arc::new(RwLock::new(None)),
            response_status: Arc::new(RwLock::new(StatusCode::OK)),
            server_handle: None,
        }
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = ([127, 0, 0, 1], self.port).into();
        
        let requests_received = Arc::clone(&self.requests_received);
        let last_request = Arc::clone(&self.last_request);
        let response_status = Arc::clone(&self.response_status);

        let make_svc = make_service_fn(move |_conn| {
            let requests_received = Arc::clone(&requests_received);
            let last_request = Arc::clone(&last_request);
            let response_status = Arc::clone(&response_status);

            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    let requests_received = Arc::clone(&requests_received);
                    let last_request = Arc::clone(&last_request);
                    let response_status = Arc::clone(&response_status);

                    async move {
                        handle_mock_request(req, requests_received, last_request, response_status).await
                    }
                }))
            }
        });

        let server = Server::bind(&addr).serve(make_svc);
        
        // Get the actual bound port
        if self.port == 0 {
            self.port = server.local_addr().port();
        }
        
        let server_handle = tokio::spawn(async move {
            if let Err(e) = server.await {
                eprintln!("Mock server error: {e}");
            }
        });

        self.server_handle = Some(server_handle);
        
        // Give server a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        Ok(())
    }

    pub async fn set_response_status(&self, status: StatusCode) {
        let mut response_status = self.response_status.write().await;
        *response_status = status;
    }

    pub fn get_request_count(&self) -> usize {
        self.requests_received.load(Ordering::Relaxed)
    }

    pub async fn get_last_request(&self) -> Option<MockRequest> {
        let last_request = self.last_request.read().await;
        last_request.clone()
    }

    pub async fn shutdown(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
    }

    pub async fn wait_for_requests(&self, expected_count: usize, timeout_secs: u64) -> bool {
        let start = tokio::time::Instant::now();
        let timeout = tokio::time::Duration::from_secs(timeout_secs);

        while start.elapsed() < timeout {
            if self.get_request_count() >= expected_count {
                return true;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
        false
    }
}

async fn handle_mock_request(
    req: Request<Body>,
    requests_received: Arc<AtomicUsize>,
    last_request: Arc<RwLock<Option<MockRequest>>>,
    response_status: Arc<RwLock<StatusCode>>,
) -> Result<Response<Body>, Infallible> {
    requests_received.fetch_add(1, Ordering::Relaxed);

    let method = req.method().to_string();
    let uri = req.uri().to_string();
    
    let mut headers = std::collections::HashMap::new();
    for (name, value) in req.headers().iter() {
        if let Ok(value_str) = value.to_str() {
            headers.insert(name.to_string(), value_str.to_string());
        }
    }

    let body_bytes = hyper::body::to_bytes(req.into_body()).await.unwrap_or_default();
    let body = String::from_utf8_lossy(&body_bytes).to_string();

    let mock_request = MockRequest {
        method,
        uri,
        headers,
        body,
    };

    {
        let mut last_request_guard = last_request.write().await;
        *last_request_guard = Some(mock_request);
    }

    let status = {
        let response_status_guard = response_status.read().await;
        *response_status_guard
    };

    let response_body = if status == StatusCode::OK {
        r#"{"status": "success", "message": "Logs received"}"#
    } else {
        r#"{"error": "Authentication failed"}"#
    };

    Ok(Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(response_body))
        .unwrap())
}

/// Test result indicating whether a test should expect success, failure, or timeout
#[derive(Debug, Clone, PartialEq)]
pub enum ExpectedResult {
    Success,
    Failure(String), // Expected error message
    NetworkTimeout,  // Explicit timeout expectation
}

/// Test utilities
pub mod test_utils {
    use super::*;
    use std::process::Command;
    use tempfile::NamedTempFile;

    pub struct TestEnvironment {
        pub mock_server: MockOpenObserveServer,
        pub temp_files: Vec<NamedTempFile>,
    }

    impl TestEnvironment {
        pub async fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
            let mut mock_server = MockOpenObserveServer::new(0); // Let OS choose port
            mock_server.start().await?;
            
            Ok(Self {
                mock_server,
                temp_files: Vec::new(),
            })
        }

        pub async fn shutdown(&mut self) {
            self.mock_server.shutdown().await;
        }
    }

    pub fn run_extension_command(args: &[&str]) -> Result<std::process::Output, std::io::Error> {
        let binary_path = std::env::current_dir()
            .unwrap()
            .join("target/debug/o2-lambda-extension");

        Command::new(binary_path)
            .args(args)
            .output()
    }

    pub fn run_extension_command_with_env(
        args: &[&str],
        env_vars: &[(&str, &str)],
    ) -> Result<std::process::Output, std::io::Error> {
        let binary_path = std::env::current_dir()
            .unwrap()
            .join("target/debug/o2-lambda-extension");

        let mut command = Command::new(binary_path);
        command.args(args);
        
        for (key, value) in env_vars {
            command.env(key, value);
        }
        
        command.output()
    }

    /// Run extension command with explicit expectations about the result
    pub fn run_extension_command_with_expectation(
        args: &[&str],
        env_vars: &[(&str, &str)],
        expected: ExpectedResult,
    ) -> Result<(), String> {
        let output = run_extension_command_with_env(args, env_vars)
            .map_err(|e| format!("Failed to run command: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined_output = format!("{stdout}{stderr}");

        match expected {
            ExpectedResult::Success => {
                if !output.status.success() {
                    return Err(format!(
                        "Expected success but command failed with exit code: {:?}\nOutput: {combined_output}",
                        output.status.code()
                    ));
                }
                Ok(())
            }
            ExpectedResult::Failure(expected_error) => {
                if output.status.success() {
                    return Err(format!(
                        "Expected failure with '{expected_error}' but command succeeded.\nOutput: {combined_output}"
                    ));
                }
                if !combined_output.contains(&expected_error) {
                    return Err(format!(
                        "Expected error message '{expected_error}' not found in output: {combined_output}"
                    ));
                }
                Ok(())
            }
            ExpectedResult::NetworkTimeout => {
                if output.status.success() {
                    return Err(format!(
                        "Expected network timeout but command succeeded.\nOutput: {combined_output}"
                    ));
                }
                let has_timeout = combined_output.contains("timed out") || 
                                combined_output.contains("timeout") ||
                                combined_output.contains("operation timed out") ||
                                combined_output.contains("deadline has elapsed") ||
                                combined_output.contains("Health check failed");
                if !has_timeout {
                    return Err(format!(
                        "Expected timeout indication but found: {combined_output}"
                    ));
                }
                Ok(())
            }
        }
    }

    /// Async version that can validate mock server interactions for successful cases
    pub async fn run_health_check_with_mock_server(
        test_env: &TestEnvironment,
        env_vars: &[(&str, &str)],
        expected: ExpectedResult,
    ) -> Result<(), String> {
        let output = run_extension_command_with_env(&["--health-check"], env_vars)
            .map_err(|e| format!("Failed to run command: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined_output = format!("{stdout}{stderr}");

        match expected {
            ExpectedResult::Success => {
                if !output.status.success() {
                    return Err(format!(
                        "Expected success but command failed with exit code: {:?}\nOutput: {combined_output}",
                        output.status.code()
                    ));
                }

                // Wait for and validate the request to mock server
                if !test_env.mock_server.wait_for_requests(1, 5).await {
                    return Err("No requests received by mock server within timeout".to_string());
                }

                let request = test_env.mock_server.get_last_request().await
                    .ok_or("No request found in mock server")?;
                
                if request.method != "POST" {
                    return Err(format!("Expected POST method, got: {}", request.method));
                }

                if !stderr.contains("Health check passed") {
                    return Err(format!("Expected 'Health check passed' in stderr, got: {stderr}"));
                }

                Ok(())
            }
            ExpectedResult::Failure(expected_error) => {
                if output.status.success() {
                    return Err(format!(
                        "Expected failure with '{expected_error}' but command succeeded.\nOutput: {combined_output}"
                    ));
                }
                if !combined_output.contains(&expected_error) {
                    return Err(format!(
                        "Expected error message '{expected_error}' not found in output: {combined_output}"
                    ));
                }
                Ok(())
            }
            ExpectedResult::NetworkTimeout => {
                // For timeout cases, we don't expect any requests to reach the mock server
                if output.status.success() {
                    return Err(format!(
                        "Expected network timeout but command succeeded.\nOutput: {combined_output}"
                    ));
                }
                let has_timeout = combined_output.contains("timed out") || 
                                combined_output.contains("timeout") ||
                                combined_output.contains("operation timed out") ||
                                combined_output.contains("deadline has elapsed") ||
                                combined_output.contains("Health check failed");
                if !has_timeout {
                    return Err(format!(
                        "Expected timeout indication but found: {combined_output}"
                    ));
                }
                Ok(())
            }
        }
    }

    pub async fn validate_log_request(
        mock_server: &MockOpenObserveServer,
        expected_org: &str,
        expected_stream: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let last_request = mock_server.get_last_request().await
            .ok_or("No request received")?;

        // Validate HTTP method
        assert_eq!(last_request.method, "POST");

        // Validate URL path
        let expected_path = format!("/api/{expected_org}/{expected_stream}//_json");
        assert!(last_request.uri.contains(&expected_path), 
               "Expected path '{}' not found in URI '{}'", expected_path, last_request.uri);

        // Validate headers
        assert!(last_request.headers.contains_key("authorization"), "Missing Authorization header");
        assert!(last_request.headers.contains_key("content-type"), "Missing Content-Type header");
        assert_eq!(last_request.headers.get("content-type").unwrap(), "application/json");

        // Validate body is valid JSON array
        let _: Value = serde_json::from_str(&last_request.body)
            .map_err(|e| format!("Invalid JSON body: {e}"))?;

        Ok(())
    }
}