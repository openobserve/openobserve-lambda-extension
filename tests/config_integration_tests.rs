mod common;

use common::test_utils::*;

#[tokio::test]
async fn test_config_validation_missing_org_id() {
    let output = run_extension_command_with_env(
        &["--health-check"],
        &[("O2_AUTHORIZATION_HEADER", "Basic dGVzdA==")],
    ).expect("Failed to run command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("O2_ORGANIZATION_ID environment variable is required"));
}

#[tokio::test]
async fn test_config_validation_missing_auth_header() {
    let output = run_extension_command_with_env(
        &["--health-check"],
        &[("O2_ORGANIZATION_ID", "test_org")],
    ).expect("Failed to run command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("O2_AUTHORIZATION_HEADER environment variable is required"));
}

#[tokio::test]
async fn test_config_validation_invalid_url() {
    let output = run_extension_command_with_env(
        &["--health-check"],
        &[
            ("O2_ORGANIZATION_ID", "test_org"),
            ("O2_AUTHORIZATION_HEADER", "Basic dGVzdA=="),
            ("O2_ENDPOINT", "invalid-url"),
        ],
    ).expect("Failed to run command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Invalid O2_ENDPOINT URL"));
}

#[tokio::test]
async fn test_config_with_defaults() {
    // This test will fail with connection error, but should pass config validation
    let output = run_extension_command_with_env(
        &["--health-check"],
        &[
            ("O2_ORGANIZATION_ID", "test_org"),
            ("O2_AUTHORIZATION_HEADER", "Basic dGVzdA=="),
        ],
    ).expect("Failed to run command");

    // Should fail on connectivity, not config validation
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should not contain config validation errors
    assert!(!stderr.contains("environment variable is required"));
    assert!(!stderr.contains("Invalid O2_ENDPOINT URL"));
}

#[tokio::test]
async fn test_config_with_custom_values() {
    let output = run_extension_command_with_env(
        &["--health-check"],
        &[
            ("O2_ORGANIZATION_ID", "custom_org"),
            ("O2_AUTHORIZATION_HEADER", "Bearer custom_token"),
            ("O2_ENDPOINT", "https://custom.openobserve.ai"),
            ("O2_STREAM", "custom_stream"),
            ("O2_BATCH_SIZE", "50"),
            ("O2_FLUSH_INTERVAL_MS", "10000"),
        ],
    ).expect("Failed to run command");

    // Should fail on connectivity to custom endpoint, but config should be valid
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should not contain config validation errors
    assert!(!stderr.contains("environment variable is required"));
    assert!(!stderr.contains("Invalid O2_ENDPOINT URL"));
    assert!(!stderr.contains("must be greater than 0"));
}

#[tokio::test]
async fn test_config_validation_invalid_numeric_values() {
    let test_cases = vec![
        ("O2_MAX_BUFFER_SIZE_MB", "0", "must be greater than 0"),
        ("O2_REQUEST_TIMEOUT_MS", "0", "must be greater than 0"),
        ("O2_MAX_RETRIES", "abc", "Invalid O2_MAX_RETRIES"),
    ];

    for (env_var, invalid_value, expected_error) in test_cases {
        let mut env_vars = vec![
            ("O2_ORGANIZATION_ID", "test_org"),
            ("O2_AUTHORIZATION_HEADER", "Basic dGVzdA=="),
        ];
        env_vars.push((env_var, invalid_value));

        let output = run_extension_command_with_env(&["--health-check"], &env_vars)
            .expect("Failed to run command");

        assert!(!output.status.success(), "Expected failure for {env_var}={invalid_value}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected_error),
            "Expected error '{expected_error}' not found for {env_var}={invalid_value}. Stderr: {stderr}"
        );
    }
}

#[tokio::test]
async fn test_config_validation_retry_delay_conflict() {
    let output = run_extension_command_with_env(
        &["--health-check"],
        &[
            ("O2_ORGANIZATION_ID", "test_org"),
            ("O2_AUTHORIZATION_HEADER", "Basic dGVzdA=="),
            ("O2_INITIAL_RETRY_DELAY_MS", "5000"),
            ("O2_MAX_RETRY_DELAY_MS", "1000"), // Less than initial
        ],
    ).expect("Failed to run command");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be greater than"));
}

#[cfg(test)]
mod url_construction_tests {
    // Note: These would need to be unit tests in the actual config module
    // since we can't easily test URL construction in integration tests
    // without actually making HTTP requests
    
    #[test]
    fn test_url_construction_documentation() {
        // This test documents the expected URL format
        // The actual URL construction is tested in unit tests in config.rs
        
        // Expected format: {endpoint}/api/{org}/{stream}/_json
        // Example: https://api.openobserve.ai/api/my_org/default/_json
        
        println!("URL format: {{endpoint}}/api/{{org}}/{{stream}}/_json");
        println!("Example: https://api.openobserve.ai/api/my_org/default/_json");
    }
}