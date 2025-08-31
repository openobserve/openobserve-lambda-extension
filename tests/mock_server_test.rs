mod common;

use common::TestEnvironment;

#[tokio::test]
async fn test_mock_server_basic() {
    let mut test_env = TestEnvironment::new().await
        .expect("Failed to create test environment");
    
    let port = test_env.mock_server.port;
    println!("Mock server started on port: {port}");
    
    // Test basic connectivity
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/test");
    
    let response = client.post(&url)
        .header("Authorization", "Bearer test")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!([{"test": "data"}]))
        .send()
        .await;
    
    match response {
        Ok(resp) => {
            println!("Response status: {}", resp.status());
            assert!(resp.status().is_success());
        }
        Err(e) => {
            panic!("Failed to connect to mock server: {e}");
        }
    }
    
    test_env.shutdown().await;
}