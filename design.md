# OpenObserve Lambda Layer Design

## Overview

This project implements an AWS Lambda layer written in Rust that captures logs from Lambda functions and forwards them to OpenObserve. The layer uses the Lambda Extensions API to run as a separate process alongside the Lambda function, capturing logs and telemetry without impacting function performance.

## Architecture

### Components

1. **Extension Process**: Main binary that registers with Lambda Extensions API
2. **Telemetry Subscriber**: Captures logs and telemetry from Lambda runtime via Logs API  
3. **OpenObserve Client**: HTTP client to send logs as JSON arrays
4. **Configuration Manager**: Handles environment variables and validation
5. **Health Check System**: Validates configuration and OpenObserve connectivity

### Data Flow

```
Lambda Function → Lambda Runtime → Logs API → Extension → OpenObserve
```

1. Extension registers with Lambda Extensions API (`/2020-01-01/lambda/register`)
2. Extension subscribes to Logs API (`/2020-08-15/lambda/logs`) for function logs
3. Lambda Runtime sends log events to extension via HTTP endpoint
4. Extension buffers logs and sends batches to OpenObserve
5. Extension participates in Lambda lifecycle (INVOKE, SHUTDOWN events)

## Configuration

### Environment Variables

#### Required Variables
| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `O2_ORGANIZATION_ID` | **Yes** | - | OpenObserve organization ID |
| `O2_AUTHORIZATION_HEADER` | **Yes** | - | Authorization header value (e.g., "Basic <base64>") |

#### Optional Variables
| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `O2_ENDPOINT` | No | `https://api.openobserve.ai` | OpenObserve API endpoint |
| `O2_STREAM` | No | `default` | OpenObserve stream name |
| `O2_MAX_BUFFER_SIZE_MB` | No | `10` | Maximum memory buffer size in MB |
| `O2_REQUEST_TIMEOUT_MS` | No | `30000` | HTTP request timeout in milliseconds |
| `O2_MAX_RETRIES` | No | `3` | Maximum retry attempts for failed requests |
| `O2_INITIAL_RETRY_DELAY_MS` | No | `1000` | Initial retry delay in milliseconds |
| `O2_MAX_RETRY_DELAY_MS` | No | `30000` | Maximum retry delay in milliseconds |


### Configuration Validation

- Startup validation ensures mandatory environment variables are present
- Extension fails fast if required configuration is missing
- Invalid endpoint URLs are detected and reported

## OpenObserve Integration

### API Endpoint

- **URL**: `{O2_ENDPOINT}/api/{O2_ORGANIZATION_ID}/{O2_STREAM}/_json`
- **Method**: POST
- **Headers**: 
  - `Authorization: {O2_AUTHORIZATION_HEADER}`
  - `Content-Type: application/json`
- **Body**: JSON array of log entries

### Log Format

No log parsing is required. Logs are forwarded as-is in JSON array format as received from the Lambda Logs API.

Example payload:
```json
[
  {
    "_timestamp": 1693397696789000,
    "type": "function",
    "record": "2023-08-30T12:34:56.789Z\tINFO\tSample log message",
    "requestId": "abc123-def456-ghi789"
  }
]
```

**Note**: The extension converts timestamps from ISO 8601 format to microsecond epoch format (`_timestamp`) as required by OpenObserve.

## Buffering and Lifecycle Strategy

### Adaptive Flushing Strategy
The extension implements a 3-tier flushing strategy:

1. **End-of-Invocation Flush**: Blocking flush before calling `/next` (ensures data delivery)
2. **Continuous Flush**: Asynchronous periodic flushing during execution (performance optimized)  
3. **Periodic Flush**: Blocking flush at configurable intervals for long-running functions

### Lifecycle Management
- **Extension Registration**: Registers for `INVOKE` and `SHUTDOWN` events
- **Telemetry Subscription**: Subscribes to platform events for precise timing
- **Post-Response Flushing**: Leverages time after Lambda response is returned to client
- **Graceful Shutdown**: Waits for final telemetry events before termination

### Async Flush Handles
- Tracks pending flush operations using `FuturesOrdered`
- Awaits completion of all pending requests during shutdown
- Implements retry logic for failed requests

## Error Handling and Reliability

### Configuration Errors
- Missing mandatory environment variables cause immediate extension failure
- Invalid URLs or malformed configuration logged and cause startup failure

### Runtime Errors
- HTTP client implements exponential backoff for OpenObserve requests
- Network failures are retried with increasing delays (1s, 2s, 4s, 8s, max 30s)
- Async flush handles prevent blocking Lambda function execution
- Failed requests are queued for retry with exponential backoff

### Graceful Degradation
- If OpenObserve is unreachable, logs are buffered and retried
- Memory limits prevent unbounded log accumulation
- Extension continues operating even with temporary OpenObserve outages
- Final shutdown flush ensures maximum data delivery

## Implementation Details

### Project Structure

```
├── Cargo.toml              # Dependencies and build configuration
├── Cargo.lock              # Dependency lockfile
├── src/
│   ├── main.rs            # Extension entry point and lifecycle
│   ├── config.rs          # Environment variable handling
│   ├── extension.rs       # Extensions API client
│   ├── telemetry.rs       # Telemetry API subscriber (renamed from logs.rs)
│   └── openobserve.rs     # OpenObserve HTTP client
├── tests/                  # Test suite
├── build.sh               # Cross-compilation script
├── deploy.sh              # Deployment helper script
├── design.md              # This document
├── telemetry_api.md       # Telemetry API documentation
├── start_specs.md         # Startup specifications
└── .gitignore             # Git ignore rules
```

### Key Dependencies

- `tokio` - Async runtime with full features
- `reqwest` - HTTP client with rustls-tls for secure connections
- `serde` - JSON serialization/deserialization with derive features
- `tracing` - Structured logging framework
- `tracing-subscriber` - Log subscriber with env-filter support
- `anyhow` - Error handling and context
- `thiserror` - Error type definitions
- `hyper` - HTTP server for telemetry endpoint
- `chrono` - Date/time handling with serde support
- `uuid` - UUID generation for request tracking

### Build Process

1. Cross-compile for `x86_64-unknown-linux-musl` target (Lambda runtime)
2. Create directory structure: `/opt/extensions/{extension_name}`
3. Package binary in ZIP file for Lambda layer deployment

### Lambda Integration

- Extension binary must be executable and placed in `/opt/extensions/`
- Binary name determines extension identifier in Lambda
- Extension participates in Lambda lifecycle events
- Runs in parallel with Lambda function, minimal performance impact

## Security Considerations

- Authorization header is passed through environment variables
- No credential logging or exposure in application logs
- HTTPS-only communication with OpenObserve using rustls-tls
- Minimal network surface area (outbound HTTPS only)
- Extension validates credentials during health checks

## Performance Considerations

- Asynchronous log processing prevents blocking Lambda execution
- Configurable batch sizes for optimal throughput
- Memory-efficient log buffering with bounds
- Minimal CPU overhead for log forwarding

## Deployment

### Layer Creation
1. Build extension binary for Lambda target
2. Package in proper directory structure
3. Create Lambda layer with binary
4. Attach layer to target Lambda functions

### Function Configuration
Set required environment variables on Lambda functions using the layer:
```
O2_ORGANIZATION_ID=your_org_id
O2_AUTHORIZATION_HEADER="Basic your_base64_token"
```

### Monitoring and Health Checks
- Extension logs are available in CloudWatch using structured logging
- Failed OpenObserve requests are logged for troubleshooting
- Configuration errors are clearly reported at startup
- Health check functionality validates configuration and OpenObserve connectivity:
  ```bash
  # Test configuration and connectivity
  ./target/release/o2-lambda-extension --health-check
  ./target/release/o2-lambda-extension -h  # Short form
  ```