# OpenObserve Lambda Layer

A high-performance AWS Lambda Extension written in Rust that automatically captures and forwards Lambda function **logs and traces** to [OpenObserve](https://openobserve.ai) in real-time — with a single layer, no ADOT required.

## 🎯 Overview

This Lambda layer runs as a separate process alongside your Lambda function, capturing all logs via the Lambda Telemetry API and distributed traces via an embedded OTLP receiver. It uses AWS Lambda's Extensions API and forwards everything to OpenObserve.

### How It Works

```
┌──────────────────────────────────────────────────────────────────┐
│                        Lambda Function                           │
│                                                                  │
│  stdout/stderr ──▶ Telemetry API ──▶ ┌──────────────────────┐  │
│                                       │   O2 Extension        │  │
│  OTel SDK ──────▶ localhost:4318 ──▶  │   (This Layer)       │──▶ OpenObserve
│  (auto-instrumented)                  │   Rust sidecar        │  │
│                                       └──────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

- **Logs**: Captured via Lambda Telemetry API → forwarded to OpenObserve `/_json`
- **Traces**: OTel SDK auto-instruments your code → sends spans to the embedded OTLP receiver on `localhost:4318` → forwarded to OpenObserve `/v1/traces`

## 🚀 Quick Start

> **TL;DR**: Run `./build.sh && ./deploy.sh` to build and deploy both architectures!

### 1. Build the Layer

```bash
# Clone the repository
git clone <your-repo>
cd o2_lambda_layer

# Build and package the layer (builds BOTH architectures by default)
./build.sh

# This creates BOTH packages:
# - target/o2-lambda-extension-x86_64.zip (for Intel/AMD Lambda functions)
# - target/o2-lambda-extension-arm64.zip (for Graviton Lambda functions)

# Build for specific architecture only (optional):
BUILD_TARGETS=x86_64-unknown-linux-musl ./build.sh   # x86_64 only
BUILD_TARGETS=aarch64-unknown-linux-musl ./build.sh  # arm64 only

# Additional build commands:
./build.sh clean   # Clean build artifacts
./build.sh test    # Run tests
./build.sh check   # Run cargo check for targets
./build.sh help    # Show detailed help
```

### 2. Deploy as Lambda Layer

#### Option A: Using the Deploy Script (Recommended)

```bash
# Deploy both architectures to AWS Lambda
./deploy.sh

# Deploy specific architecture only
DEPLOY_ARCH=x86_64 ./deploy.sh  # Deploy x86_64 only
DEPLOY_ARCH=arm64 ./deploy.sh   # Deploy arm64 only

# Deploy to specific AWS region
AWS_REGION=eu-west-1 ./deploy.sh

# List existing layer versions
./deploy.sh list

# Delete all layer versions (with confirmation)
./deploy.sh delete

# Show deployment help
./deploy.sh help
```

#### Option B: Manual AWS CLI Deployment

```bash
# For x86_64 Lambda functions (Intel/AMD):
aws lambda publish-layer-version \
  --layer-name openobserve-extension-x86_64 \
  --zip-file fileb://target/o2-lambda-extension-x86_64.zip \
  --compatible-runtimes python3.9 python3.10 python3.11 python3.12 python3.13 nodejs18.x nodejs20.x nodejs22.x java11 java17 java21 dotnet8 ruby3.3 provided.al2 provided.al2023 \
  --compatible-architectures x86_64 \
  --description "OpenObserve lambda layer extension for forwarding logs (x86_64)"

# For arm64 Lambda functions (Graviton):
aws lambda publish-layer-version \
  --layer-name openobserve-extension-arm64 \
  --zip-file fileb://target/o2-lambda-extension-arm64.zip \
  --compatible-runtimes python3.9 python3.10 python3.11 python3.12 python3.13 nodejs18.x nodejs20.x nodejs22.x java11 java17 java21 dotnet8 ruby3.3 provided.al2 provided.al2023 \
  --compatible-architectures arm64 \
  --description "OpenObserve lambda layer extension for forwarding logs (arm64)"
```

### 3. Configure Your Lambda Function

Add the layer to your Lambda function and set these environment variables:

#### Logs only (all runtimes)

| Variable | Value |
|---|---|
| `O2_ORGANIZATION_ID` | Your OpenObserve org ID |
| `O2_AUTHORIZATION_HEADER` | `Basic <base64-credentials>` |
| `O2_ENDPOINT` | `https://api.openobserve.ai` |
| `O2_STREAM` | `lambda_logs` |

#### Logs + Traces (Node.js)

Add all the above **plus**:

| Variable | Value | Description |
|---|---|---|
| `AWS_LAMBDA_EXEC_WRAPPER` | `/opt/otel-instrument` | Enables OTel auto-instrumentation |
| `OTEL_SERVICE_NAME` | your function name | Identifies your service in traces |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `https://api.openobserve.ai/api/<org-id>` | Base OTLP endpoint |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | `https://api.openobserve.ai/api/<org-id>/v1/traces` | Traces endpoint |
| `OTEL_EXPORTER_OTLP_HEADERS` | `Authorization=Basic <base64-credentials>` | Auth header for OTLP export |
| `OTEL_TRACES_EXPORTER` | `otlp` | Use OTLP exporter |
| `OTEL_TRACES_SAMPLER` | `always_on` | Sample all traces |
| `OTEL_BSP_SCHEDULE_DELAY` | `1` | Flush spans after 1ms (critical for Lambda) |
| `OTEL_BSP_EXPORT_TIMEOUT` | `500` | Span export timeout in ms |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | `500` | OTLP request timeout in ms |

> **Note**: `OTEL_BSP_SCHEDULE_DELAY=1` is critical. Without it the default 5000ms batch delay causes spans to be lost when Lambda freezes the process after the handler returns.

### 4. Deploy Your Function

That's it! Your Lambda function will now automatically forward all logs and traces to OpenObserve.

## ⚙️ Configuration

### Environment Variables

#### Extension (Logs)

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `O2_ORGANIZATION_ID` | **Yes** | - | Your OpenObserve organization ID |
| `O2_AUTHORIZATION_HEADER` | **Yes** | - | Auth header value (e.g., `"Basic <base64>"`) |
| `O2_ENDPOINT` | No | `https://api.openobserve.ai` | OpenObserve API endpoint URL |
| `O2_STREAM` | No | `default` | Target log stream name |

#### OTel SDK (Traces — Node.js only)

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `AWS_LAMBDA_EXEC_WRAPPER` | **Yes** | - | Set to `/opt/otel-instrument` to enable auto-instrumentation |
| `OTEL_SERVICE_NAME` | **Yes** | - | Service name shown in traces |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | **Yes** | - | `https://api.openobserve.ai/api/<org-id>` |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | **Yes** | - | `https://api.openobserve.ai/api/<org-id>/v1/traces` |
| `OTEL_EXPORTER_OTLP_HEADERS` | **Yes** | - | `Authorization=Basic <base64-credentials>` |
| `OTEL_TRACES_EXPORTER` | No | `otlp` | Exporter type |
| `OTEL_TRACES_SAMPLER` | No | `always_on` | Sampling strategy |
| `OTEL_BSP_SCHEDULE_DELAY` | No | `1` | Batch processor flush delay (ms) — keep at `1` for Lambda |
| `OTEL_BSP_EXPORT_TIMEOUT` | No | `500` | Span export timeout (ms) |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | No | `500` | OTLP HTTP request timeout (ms) |

### Advanced Configuration (Optional)

For high-volume or specialized use cases:

| Variable | Default | Description |
|----------|---------|-------------|
| `O2_BATCH_SIZE` | 100 | Max logs per batch sent to OpenObserve |
| `O2_FLUSH_INTERVAL_MS` | 5000 | Flush interval for periodic flushing (ms) |
| `O2_MAX_BUFFER_SIZE_MB` | 10 | Max memory buffer size before dropping logs |
| `O2_REQUEST_TIMEOUT_MS` | 30000 | HTTP request timeout (ms) |
| `O2_MAX_RETRIES` | 3 | Max retry attempts for failed requests |
| `O2_INITIAL_RETRY_DELAY_MS` | 1000 | Initial retry delay (ms) |
| `O2_MAX_RETRY_DELAY_MS` | 30000 | Maximum retry delay (ms) |

## 🧠 Smart Flushing Strategies

The extension automatically chooses the optimal flushing strategy based on your function's invocation pattern:

### End-of-Invocation Flush
- **When**: Low-frequency functions (<10 invocations/minute)
- **Behavior**: Flushes all logs before calling `/next`
- **Benefit**: Ensures maximum data delivery for sporadic functions

### Continuous Flush  
- **When**: High-frequency functions (≥10 invocations/minute)
- **Behavior**: Async flushing during function execution
- **Benefit**: Optimal performance for busy functions

### Periodic Flush
- **When**: Long-running functions (>30s since last invocation)
- **Behavior**: Timer-based flushing at regular intervals
- **Benefit**: Handles long-duration functions efficiently

## 📊 What Gets Logged

The extension forwards **all** Lambda logs without parsing or modification:

- **Function logs**: Your application's stdout/stderr
- **Runtime logs**: Lambda runtime messages  
- **Extension logs**: Start/stop/error messages from this extension
- **Platform logs**: Lambda platform events and metrics

### Log Format

Logs are sent to OpenObserve as JSON arrays:

```json
[
  {
    "time": "2023-08-30T12:34:56.789Z",
    "type": "function", 
    "record": "2023-08-30T12:34:56.789Z\tINFO\tYour log message here",
    "requestId": "abc123-def456-ghi789"
  }
]
```

## ⚠️ Important Considerations

### Performance Impact
- **Cold Start**: Adds ~70ms to cold start time (vs 450ms+ for comparable solutions)
- **Runtime**: Zero impact on function execution (runs asynchronously)
- **Memory**: Uses ~10MB RAM (configurable buffer size)

### Cost Implications
- **Logging Volume**: High-volume functions will increase OpenObserve ingestion costs
- **Lambda Duration**: Extension flushing happens after response is sent (no duration impact)
- **Network**: Additional outbound HTTPS requests to OpenObserve

### Reliability
- **Network Failures**: Extension retries with exponential backoff
- **OpenObserve Outages**: Logs are buffered temporarily, oldest dropped if buffer fills
- **Extension Failures**: Lambda function continues to work normally
- **Data Loss**: Some logs may be lost if Lambda times out during flush

## 🔧 Monitoring and Troubleshooting

### CloudWatch Logs

The extension logs to CloudWatch with these key messages:

```
# Successful startup
INFO Extension registered successfully: my-function (ID: abc123)
INFO Successfully subscribed to Logs API on port 8080

# Flushing activity  
INFO Successfully sent 45 logs to OpenObserve
WARN Retry attempt 2/3 failed: Request timeout

# Shutdown
INFO Starting shutdown sequence with 2.5s deadline
INFO Final flush completed successfully
```

### Common Issues

**Extension Not Starting**
```bash
ERROR Configuration error: O2_ORGANIZATION_ID environment variable is required
```
→ Check required environment variables are set

**Network Connectivity**
```bash
ERROR OpenObserve request failed with status 401: Unauthorized
```
→ Verify `O2_AUTHORIZATION_HEADER` is correct

**High Memory Usage**
```bash
WARN Log buffer full, dropping oldest batch
```
→ Reduce `O2_MAX_BUFFER_SIZE_MB` or increase `O2_FLUSH_INTERVAL_MS`

### Health Check

Test the extension configuration and connectivity:

```bash
# Set required environment variables
export O2_ORGANIZATION_ID=your_org
export O2_AUTHORIZATION_HEADER="Basic your_token"

# Run health check
./target/debug/o2-lambda-extension --health-check

# Or use the release binary
./target/release/o2-lambda-extension --health-check
```

The health check will:
- ✅ Validate all configuration parameters
- ✅ Test connectivity to OpenObserve API
- ✅ Verify authentication credentials
- ✅ Send a test log entry

### CLI Usage

```bash
# Show help
./target/debug/o2-lambda-extension --help

# Show version  
./target/debug/o2-lambda-extension --version

# Run health check
./target/debug/o2-lambda-extension --health-check

# Alternative short form
./target/debug/o2-lambda-extension -h
```

## 🔒 Security

### Credentials
- **Environment Variables**: Store credentials securely in Lambda environment
- **AWS Secrets Manager**: Consider using AWS Secrets Manager for enhanced security
- **No Logging**: Extension never logs credential values

### Network Security  
- **HTTPS Only**: All communication uses TLS encryption
- **Outbound Only**: Extension only makes outbound connections
- **No Inbound Ports**: Does not open any listening ports (except internal log receiver)

### Data Privacy
- **No Parsing**: Logs are forwarded as-is without inspection
- **No Storage**: No local storage of logs or credentials
- **Minimal Metadata**: Only adds timestamps and request IDs

## 🛠️ Development

### Building from Source

```bash
# Prerequisites
rustup target add x86_64-unknown-linux-musl

# Build  
./build.sh

# Run tests
cargo test

# Check code quality
cargo clippy
```

### Project Structure

```
├── Cargo.toml              # Dependencies and build configuration
├── Cargo.lock              # Dependency lockfile
├── otel-instrument         # Bash bootstrap script (AWS_LAMBDA_EXEC_WRAPPER target)
├── src/
│   ├── main.rs            # Extension entry point and lifecycle
│   ├── config.rs          # Environment variable handling
│   ├── extension.rs       # Extensions API client + flushing strategies
│   ├── otlp_receiver.rs   # Embedded OTLP HTTP receiver on localhost:4318
│   ├── telemetry.rs       # Telemetry API subscriber for logs
│   └── openobserve.rs     # OpenObserve HTTP client
├── build.sh               # Cross-compilation + layer packaging script
├── deploy.sh              # AWS Lambda layer deployment script
└── .gitignore             # Git ignore rules
```

## 📈 Performance Benchmarks

| Metric | Value | Notes |
|--------|--------|--------|
| Cold Start Impact | ~70ms | One-time per container |
| Memory Usage | 5-15MB | Depends on buffer size |
| CPU Impact | <1% | During log processing |
| Network Overhead | ~1KB/log | Compressed JSON |

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Run tests: `cargo test`
5. Submit a pull request

## 📄 License

MIT License - See [Cargo.toml](Cargo.toml) for details

## 🆘 Support

- **Issues**: [GitHub Issues](openobserve-lambda-layer)
- **Documentation**: [OpenObserve Docs](https://openobserve.ai/docs)
- **Community**: [OpenObserve Slack](https://short.openobserve.ai/community)

---

**Built with ❤️ in Rust for maximum performance and reliability.**
