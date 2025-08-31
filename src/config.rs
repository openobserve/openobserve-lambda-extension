use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::env;
use url::Url;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub o2_endpoint: String,
    pub o2_organization_id: String,
    pub o2_stream: String,
    pub o2_authorization_header: String,
    
    // Performance tuning
    pub max_buffer_size_mb: usize,
    pub request_timeout_ms: u64,
    
    // Retry configuration
    pub max_retries: u32,
    pub initial_retry_delay_ms: u64,
    pub max_retry_delay_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            o2_endpoint: "https://api.openobserve.ai".to_string(),
            o2_organization_id: String::new(),
            o2_stream: "default".to_string(),
            o2_authorization_header: String::new(),
            max_buffer_size_mb: 10,
            request_timeout_ms: 30000,
            max_retries: 3,
            initial_retry_delay_ms: 1000,
            max_retry_delay_ms: 30000,
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // Required environment variables
        let o2_organization_id = env::var("O2_ORGANIZATION_ID")
            .map_err(|_| anyhow!("O2_ORGANIZATION_ID environment variable is required"))?;
        
        let o2_authorization_header = env::var("O2_AUTHORIZATION_HEADER")
            .map_err(|_| anyhow!("O2_AUTHORIZATION_HEADER environment variable is required"))?;
        
        let mut config = Config {
            o2_organization_id,
            o2_authorization_header,
            ..Default::default()
        };
        
        // Optional environment variables with defaults
        if let Ok(endpoint) = env::var("O2_ENDPOINT") {
            config.o2_endpoint = endpoint;
        }
        
        if let Ok(stream) = env::var("O2_STREAM") {
            config.o2_stream = stream;
        }
        
        // Performance tuning variables
        if let Ok(max_buffer_size) = env::var("O2_MAX_BUFFER_SIZE_MB") {
            config.max_buffer_size_mb = max_buffer_size.parse()
                .map_err(|_| anyhow!("Invalid O2_MAX_BUFFER_SIZE_MB: must be a positive integer"))?;
        }
        
        if let Ok(request_timeout) = env::var("O2_REQUEST_TIMEOUT_MS") {
            config.request_timeout_ms = request_timeout.parse()
                .map_err(|_| anyhow!("Invalid O2_REQUEST_TIMEOUT_MS: must be a positive integer"))?;
        }
        
        // Retry configuration
        if let Ok(max_retries) = env::var("O2_MAX_RETRIES") {
            config.max_retries = max_retries.parse()
                .map_err(|_| anyhow!("Invalid O2_MAX_RETRIES: must be a positive integer"))?;
        }
        
        if let Ok(initial_delay) = env::var("O2_INITIAL_RETRY_DELAY_MS") {
            config.initial_retry_delay_ms = initial_delay.parse()
                .map_err(|_| anyhow!("Invalid O2_INITIAL_RETRY_DELAY_MS: must be a positive integer"))?;
        }
        
        if let Ok(max_delay) = env::var("O2_MAX_RETRY_DELAY_MS") {
            config.max_retry_delay_ms = max_delay.parse()
                .map_err(|_| anyhow!("Invalid O2_MAX_RETRY_DELAY_MS: must be a positive integer"))?;
        }
        
        // Validate configuration
        config.validate()?;
        
        Ok(config)
    }
    
    pub fn validate(&self) -> Result<()> {
        // Validate endpoint URL
        Url::parse(&self.o2_endpoint)
            .map_err(|e| anyhow!("Invalid O2_ENDPOINT URL: {}", e))?;
        
        // Validate organization ID is not empty
        if self.o2_organization_id.trim().is_empty() {
            return Err(anyhow!("O2_ORGANIZATION_ID cannot be empty"));
        }
        
        // Validate stream name is not empty
        if self.o2_stream.trim().is_empty() {
            return Err(anyhow!("O2_STREAM cannot be empty"));
        }
        
        // Validate authorization header is not empty
        if self.o2_authorization_header.trim().is_empty() {
            return Err(anyhow!("O2_AUTHORIZATION_HEADER cannot be empty"));
        }
        
        // Validate numeric constraints
        
        if self.max_buffer_size_mb == 0 {
            return Err(anyhow!("O2_MAX_BUFFER_SIZE_MB must be greater than 0"));
        }
        
        if self.request_timeout_ms == 0 {
            return Err(anyhow!("O2_REQUEST_TIMEOUT_MS must be greater than 0"));
        }
        
        if self.initial_retry_delay_ms > self.max_retry_delay_ms {
            return Err(anyhow!("O2_INITIAL_RETRY_DELAY_MS cannot be greater than O2_MAX_RETRY_DELAY_MS"));
        }
        
        Ok(())
    }
    
    pub fn openobserve_url(&self) -> String {
        format!("{}/api/{}/{}/_json", 
            self.o2_endpoint, 
            self.o2_organization_id, 
            self.o2_stream
        )
    }
    
    pub fn max_buffer_size_bytes(&self) -> usize {
        self.max_buffer_size_mb * 1024 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    
    #[test]
    fn test_config_validation() {
        // Set required environment variables
        env::set_var("O2_ORGANIZATION_ID", "test_org");
        env::set_var("O2_AUTHORIZATION_HEADER", "Basic dGVzdDp0ZXN0");
        
        let config = Config::from_env().expect("Config should be valid");
        
        assert_eq!(config.o2_organization_id, "test_org");
        assert_eq!(config.o2_authorization_header, "Basic dGVzdDp0ZXN0");
        assert_eq!(config.o2_endpoint, "https://api.openobserve.ai");
        assert_eq!(config.o2_stream, "default");
        
        // Clean up
        env::remove_var("O2_ORGANIZATION_ID");
        env::remove_var("O2_AUTHORIZATION_HEADER");
    }
    
    #[test]
    fn test_openobserve_url() {
        let config = Config {
            o2_endpoint: "https://api.openobserve.ai".to_string(),
            o2_organization_id: "my_org".to_string(),
            o2_stream: "my_stream".to_string(),
            ..Default::default()
        };
        
        assert_eq!(
            config.openobserve_url(),
            "https://api.openobserve.ai/api/my_org/my_stream/_json"
        );
    }
}