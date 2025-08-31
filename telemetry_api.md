# AWS Lambda Telemetry API Implementation Status & OTLP Enhancement Plan

## Overview

The OpenObserve Lambda Extension **currently implements** the AWS Lambda Telemetry API to capture comprehensive telemetry data. This document outlines the current implementation status and Phase 2 plans for enhanced OTLP HTTP integration with separate endpoints for logs, metrics, and traces.

## Current Implementation Status ✅ **IMPLEMENTED**

**Current Implementation (Telemetry API):**
- ✅ **Already using Telemetry API**: `TelemetrySubscriber` subscribes to `/2022-07-01/telemetry`
- ✅ **Comprehensive data capture**: Captures platform telemetry, function logs, and extension logs
- ✅ **Complete telemetry events**: `TelemetryEvent` with full AWS telemetry schema support
- ✅ **HTTP server**: Running on port 8080 receiving all telemetry data types
- ✅ **Batch processing**: Processes all telemetry events into batches for OpenObserve
- ✅ **Timestamp conversion**: Converts ISO 8601 timestamps to OpenObserve `_timestamp` format
- ✅ **Current configuration**: Uses `O2_ENDPOINT`, `O2_ORGANIZATION_ID`, `O2_AUTHORIZATION_HEADER`

**What We Currently Send to OpenObserve:**
- All platform events (lifecycle, metrics, traces) → sent as "logs" to OpenObserve
- Function logs → sent as "logs" to OpenObserve  
- Extension logs → sent as "logs" to OpenObserve
- Everything goes to a single OpenObserve logs endpoint: `{O2_ENDPOINT}/api/{O2_ORGANIZATION_ID}/{O2_STREAM}/_json`

## Phase 2 Enhancement Plan: OTLP HTTP Integration 🚀

### Goal: Transform Single-Endpoint to Multi-Endpoint OTLP

**Current Challenge:**
- Everything sent as "logs" to one OpenObserve endpoint
- Platform metrics and traces mixed with logs, losing semantic meaning
- No separation between logs, metrics, and traces for proper observability

**Phase 2 Objective:**
- Separate telemetry data by type and send to appropriate OTLP endpoints
- Use proper OTLP HTTP protocol for enhanced observability
- Maintain existing comprehensive telemetry capture

### Phase 2.1: Data Classification & Routing ✅ **Already Implemented Foundation**

**Current State:**  
- ✅ `TelemetryEvent` structure supports all AWS telemetry types
- ✅ Event parsing handles `platform.*`, `function`, `extension` events  
- ✅ Telemetry aggregation and buffering system in place

**Enhancement:**
- Add data type classification logic to route events appropriately
- Platform events → **Metrics & Traces**
- Function/Extension logs → **Logs**

### Phase 2.2: OTLP HTTP Endpoint Integration

**New Multi-Endpoint Architecture:**
```
Current: All Telemetry → Single OpenObserve Logs Endpoint
Phase 2: Classified Telemetry → Multiple OTLP Endpoints

┌─────────────────────┐    ┌─────────────────────┐    ┌─────────────────────┐
│   AWS Telemetry     │    │   Extension         │    │   OpenObserve       │
│   API Stream        │───▶│   Classification    │───▶│   OTLP Endpoints    │
│                     │    │   & Routing         │    │                     │
│ • platform.report   │    │                     │    │ • /v1/logs          │
│ • platform.start    │    │ ┌─────────────────┐ │    │ • /v1/metrics       │
│ • function logs     │    │ │ OTLP Converters │ │    │ • /v1/traces        │
│ • extension logs    │    │ └─────────────────┘ │    │                     │
└─────────────────────┘    └─────────────────────┘    └─────────────────────┘
```

### Phase 2.3: OTLP HTTP Endpoint Configuration
OpenObserve supports OTLP HTTP protocol for all telemetry data types using standard OTLP endpoints:
- **Base Endpoint**: `{OTEL_EXPORTER_OTLP_ENDPOINT}` (e.g., `https://api.openobserve.ai/api/my_org`)
- **Logs**: `{OTEL_EXPORTER_OTLP_ENDPOINT}/v1/logs`
- **Metrics**: `{OTEL_EXPORTER_OTLP_ENDPOINT}/v1/metrics`  
- **Traces**: `{OTEL_EXPORTER_OTLP_ENDPOINT}/v1/traces`

### Phase 2.4: Data Transformation to OTLP Format

**Classification Logic:**
- **Logs**: `function` and `extension` events → OTLP LogRecord format → `/v1/logs`
- **Metrics**: `platform.report`, `platform.initReport` events → OTLP Metric format → `/v1/metrics`  
- **Traces**: `platform.start`, `platform.runtimeDone` lifecycle events → OpenTelemetry Spans → `/v1/traces`

**OTLP Conversion Details:**

### Phase 2.4.1: OpenTelemetry Span Creation (Following AWS Guidance)
Convert AWS Lambda Telemetry events to OTel spans using recommended approaches:

#### Span Events Mapping Strategy (Recommended)
Convert three related Lambda lifecycle events into a single OpenTelemetry Span:

**Event Mapping:**
- `platform.start` → Span start time and initial attributes
- `platform.runtimeDone` → Span events and runtime attributes  
- `platform.report` → Span end time, final status, and metrics

**Implementation Steps:**
1. **Trace Context Extraction:**
   ```
   traceId = event.tracing.value.Root (remove "1-" prefix)
   spanId = event.tracing.value.Parent  
   parentId = extracted from Parent field
   sampled = event.tracing.value.Sampled
   ```

2. **Span Creation:**
   - Set Span Kind = `Server` (Lambda function is server)
   - Set Span Name = `{function_name}` or event type
   - Set Start Time = `platform.start` timestamp
   - Set End Time = `platform.report` timestamp

3. **Span Status:**
   - `Error` if any event status ≠ `success`  
   - `Ok` for successful completion
   - `Unset` as default

4. **Span Attributes:**
   - `aws.lambda.function_name`
   - `aws.lambda.function_version` 
   - `aws.lambda.request_id`
   - `aws.lambda.invocation.duration_ms`
   - Custom attributes from event properties

#### Child Spans Mapping Strategy (Alternative)
Create nested child spans for different Lambda phases:

**Span Hierarchy:**
```
Lambda Invocation (Parent Span)
├── Initialization Phase (Child Span) 
├── Runtime Phase (Child Span)
└── Report Phase (Child Span)
```

**Benefits:**
- More granular tracing of Lambda phases
- Better visualization of timing relationships
- Detailed performance analysis per phase

### Phase 2.5: Configuration Updates for OTLP HTTP

**New Environment Variables (Standard OTLP Variables):**
- **Required**: `OTEL_EXPORTER_OTLP_ENDPOINT` - Base OTLP endpoint including org (e.g., `https://api.openobserve.ai/api/my_org`)
- **Required**: `OTEL_EXPORTER_OTLP_HEADERS` - Authorization and other headers (e.g., `authorization=Basic xyz123`)
- **Optional**: `O2_TELEMETRY_TYPES` (default: "platform,function,extension")
- **Optional**: `O2_SPAN_MAPPING_STRATEGY` (default: "span_events", alternative: "child_spans")
- **Optional**: `OTEL_EXPORTER_OTLP_TIMEOUT` - Request timeout in seconds (default: 10)

**Migration Strategy:**
- Phase 2.0: Support **both** old and new configuration variables
- Phase 2.1: Migrate users to new OTLP standard variables
- Phase 2.2: Deprecate old variables (`O2_ENDPOINT`, `O2_ORGANIZATION_ID`, `O2_AUTHORIZATION_HEADER`)

**Endpoint Resolution:**
- Logs: `${OTEL_EXPORTER_OTLP_ENDPOINT}/v1/logs`
- Metrics: `${OTEL_EXPORTER_OTLP_ENDPOINT}/v1/metrics`
- Traces: `${OTEL_EXPORTER_OTLP_ENDPOINT}/v1/traces`

**Configuration Example:**
```bash
# Required
export OTEL_EXPORTER_OTLP_ENDPOINT="https://api.openobserve.ai/api/my_organization_123"
export OTEL_EXPORTER_OTLP_HEADERS="authorization=Basic cHJhYmhhdEBvcGVub2JzZXJ2ZS5haTp***"

# Optional
export O2_TELEMETRY_TYPES="platform,function,extension"
export O2_SPAN_MAPPING_STRATEGY="span_events"
export OTEL_EXPORTER_OTLP_TIMEOUT="10"
```

### Phase 2.5.1: Buffering Configuration
- Maintain existing batch size controls
- Add telemetry-specific buffering options
- Support different flush intervals per data type

### Phase 2.6: Implementation Steps

**Core Changes Needed:**
1. ✅ **Already Done**: `telemetry.rs` implemented with `TelemetrySubscriber`
2. ✅ **Already Done**: Telemetry event parsing and processing in place
3. 🆕 **New**: Implement OTLP data format converters:
   - `OtlpLogsConverter` - Convert function/extension logs to OTLP LogRecord format
   - `OtlpMetricsConverter` - Extract and convert metrics from `platform.report` events
   - `OtlpSpansConverter` - Convert Lambda lifecycle events to OpenTelemetry spans
4. 🆕 **New**: Add data classification and routing logic in `telemetry.rs`
5. 🆕 **New**: Enhance `OpenObserveClient` with OTLP HTTP support for multiple endpoints
6. 🆕 **New**: Update configuration management to support OTLP variables

**Enhanced Data Flow (Phase 2):**
```
                           ✅ CURRENT                    🆕 PHASE 2 ENHANCEMENT
Lambda Runtime → Telemetry API → Extension HTTP Server → Event Classification & Routing
                                          │
                    ┌─────────────────────┼─────────────────────┐
                    │                     │                     │
                    ▼                     ▼                     ▼
         Function/Extension Logs    Platform Reports    Lifecycle Events
                    │                     │                     │
                    ▼                     ▼                     ▼
           OtlpLogsConverter    OtlpMetricsConverter    OtlpSpansConverter
                    │                     │                     │
                    ▼                     ▼                     ▼
              /v1/logs               /v1/metrics            /v1/traces
```

**Current vs Phase 2 Comparison:**
- **Current**: All telemetry → Single logs endpoint (everything as "logs")
- **Phase 2**: Classified telemetry → Appropriate OTLP endpoints (proper observability)

### Phase 2.6.1: OTLP Implementation Details
- Use OpenTelemetry Rust SDK for OTLP format generation
- Implement proper resource attribution (service.name, service.version, etc.)
- Handle trace context propagation between spans
- Support both Span Events and Child Spans mapping strategies
- Maintain proper timing relationships between Lambda lifecycle events

### Phase 2.7: Testing & Validation

**Phase 2.7.1: Functional Testing**
- Verify all telemetry types are captured
- Test OpenObserve data ingestion for logs/metrics/traces  
- Validate trace correlation and metrics accuracy
- Performance testing with high-volume telemetry

**Phase 2.7.2: Integration Testing**
- Test with various Lambda runtime types
- Verify X-Ray tracing integration
- Test with different OpenObserve configurations

## AWS Lambda Telemetry API Details

### Telemetry Data Types
The Telemetry API provides access to three types of telemetry streams:

1. **Platform Telemetry**: Logs, metrics, and traces describing events and errors related to:
   - Execution environment runtime lifecycle
   - Extension lifecycle  
   - Function invocations

2. **Function Logs**: Custom logs that the Lambda function code generates

3. **Extension Logs**: Custom logs that the Lambda extension code generates

### Key Event Types
1. **Platform Events**:
   - `platform.initStart`: Function initialization start
   - `platform.initRuntimeDone`: Function initialization completion
   - `platform.initReport`: Initialization phase report
   - `platform.start`: Function invocation start
   - `platform.runtimeDone`: Function invocation completion
   - `platform.report`: Invocation phase report
   - `platform.restoreStart`: Environment restoration start
   - `platform.restoreRuntimeDone`: Environment restoration completion
   - `platform.restoreReport`: Restoration phase report
   - `platform.telemetrySubscription`: Extension subscription details
   - `platform.logsDropped`: Dropped log events

2. **Log Events**:
   - `function`: Logs from function code
   - `extension`: Logs from extension code

### Common Event Structure
```json
{
  "time": "ISO 8601 Timestamp",
  "type": "Event Type", 
  "record": { "Event-specific details" }
}
```

### Subscription Configuration
- **Protocols**: HTTP (recommended) or TCP
- **Buffering parameters**:
  - `maxBytes`: 262,144 to 1,048,576 bytes
  - `maxItems`: 1,000 to 10,000 events
  - `timeoutMs`: 25 to 30,000 milliseconds

## Key Benefits

### Current Implementation Benefits ✅
1. **Complete Telemetry Capture**: Already capturing logs, platform metrics, and traces via Telemetry API
2. **Enhanced Data**: Platform events provide runtime insights beyond basic function logs
3. **Future-Proof Architecture**: Using AWS's recommended Telemetry API approach
4. **Comprehensive Coverage**: All Lambda lifecycle, performance, and logging data captured

### Phase 2 Enhancement Benefits 🚀
1. **Proper Observability Semantics**: Logs, metrics, and traces sent to appropriate endpoints
2. **Industry Standard OTLP**: Native OTLP HTTP protocol for better tooling compatibility
3. **Enhanced Visualization**: Separate data streams enable proper dashboards and alerting in OpenObserve
4. **OpenTelemetry Ecosystem**: Standard OTLP format works with OTel collectors and tools
5. **Correlation & Context**: Proper trace correlation between logs, metrics, and spans
6. **Performance Insights**: Lambda metrics as proper time-series data rather than log entries

## Migration from Current Configuration

**Environment Variable Changes:**
- `O2_ENDPOINT` + `O2_ORGANIZATION_ID` → `OTEL_EXPORTER_OTLP_ENDPOINT` (includes org in URL path)
- `O2_AUTHORIZATION_HEADER` → `OTEL_EXPORTER_OTLP_HEADERS` (standard OTLP variable)

**URL Construction Change:**
- **Before**: `{O2_ENDPOINT}/api/{O2_ORGANIZATION_ID}/default/_json` 
- **After**: `{OTEL_EXPORTER_OTLP_ENDPOINT}/v1/logs` (where endpoint already includes `/api/org_id`)

**Benefits of Standard OTLP Variables:**
- Compatibility with OpenTelemetry ecosystem
- Consistent with OTLP exporter libraries  
- Simplified configuration management
- Industry standard environment variable names

## Risk Mitigation

1. **Gradual Migration**: Keep existing log processing logic as fallback
2. **Feature Flags**: Environment variables to control telemetry types
3. **Configuration Migration**: Support both old and new environment variables during transition
4. **Comprehensive Testing**: Validate all telemetry scenarios

## Required Dependencies

Add to `Cargo.toml`:
```toml
[dependencies]
# Existing dependencies...
opentelemetry = "0.28"
opentelemetry-otlp = { version = "0.30", features = ["http-proto", "reqwest-blocking-client"] }
opentelemetry-semantic-conventions = "0.28" 
opentelemetry_sdk = { version = "0.28", features = ["rt-tokio"] }
tonic = "0.12"  # For OTLP gRPC support (if needed)
prost = "0.13"  # Protocol buffer support
```

**Version Notes:**
- **Updated to latest stable versions** (as of 2024/2025)
- **MSRV**: Minimum Supported Rust Version is **1.75.0**
- **Breaking changes**: Versions 0.28+ include breaking changes from earlier versions
- **Default features**: `opentelemetry-otlp` now defaults to `http-proto` and `reqwest-blocking-client`
- **Unified versioning**: All OpenTelemetry crates now follow the same version scheme


## References

- [AWS Lambda Telemetry API Documentation](https://docs.aws.amazon.com/lambda/latest/dg/telemetry-api.html)
- [Telemetry API Schema Reference](https://docs.aws.amazon.com/lambda/latest/dg/telemetry-schema-reference.html)
- [OpenTelemetry Spans Documentation](https://docs.aws.amazon.com/lambda/latest/dg/telemetry-otel-spans.html)
- [AWS Lambda Telemetry API Blog Post](https://aws.amazon.com/blogs/compute/introducing-the-aws-lambda-telemetry-api/)