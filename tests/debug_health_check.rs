mod common;

use common::TestEnvironment;
use std::time::Duration;

#[tokio::test] 
async fn debug_health_check() {
    let mut test_env = TestEnvironment::new().await
        .expect("Failed to create test environment");
    
    let mock_port = test_env.mock_server.port;
    println!("Mock server started on port: {mock_port}");
    
    // Give it more time to fully start
    tokio::time::sleep(Duration::from_millis(500)).await;
    
    // Test basic connectivity first
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    
    let test_url = format!("http://127.0.0.1:{mock_port}/api/test_org/default/_json");
    println!("Testing URL: {test_url}");
    
    let test_body = serde_json::json!([{
        "time": "2024-01-01T00:00:00Z",
        "type": "extension", 
        "record": "test"
    }]);
    
    let response = client
        .post(&test_url)
        .header("Authorization", "Basic dGVzdA==")
        .header("Content-Type", "application/json")
        .json(&test_body)
        .send()
        .await;
        
    match response {
        Ok(resp) => {
            let status = resp.status();
            println!("Response status: {status}");
            let body = resp.text().await.unwrap_or_default();
            println!("Response body: {body}");
            assert!(status.is_success());
        }
        Err(e) => {
            println!("Request failed: {e}");
            panic!("Failed to connect to mock server");
        }
    }
    
    // Now test the actual health check
    println!("Running actual health check");
    
    let output = std::process::Command::new("target/debug/o2-lambda-extension")
        .args(["--health-check"])
        .env("O2_ORGANIZATION_ID", "test_org")
        .env("O2_AUTHORIZATION_HEADER", "Basic dGVzdA==")
        .env("O2_ENDPOINT", format!("http://127.0.0.1:{mock_port}"))
        .output()
        .expect("Failed to run health check");
        
    println!("Health check exit code: {:?}", output.status.code());
    println!("Health check stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("Health check stderr: {}", String::from_utf8_lossy(&output.stderr));
    
    test_env.shutdown().await;
}