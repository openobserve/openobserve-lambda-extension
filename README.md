# OpenObserve Lambda Layer

A high-performance AWS Lambda Extension written in Rust that automatically captures and forwards Lambda function **logs, metrics, and traces** to [OpenObserve](https://openobserve.ai) in real-time — with a single layer, no ADOT required, no CloudWatch in the path.

## 🎯 Overview

The layer runs a small (~3.6 MB, static-linked Rust) sidecar process alongside your Lambda function. It:

- Subscribes to the **Lambda Telemetry API** for function stdout/stderr and platform events.
- Runs a local **OTLP HTTP receiver** on `localhost:4318` for spans and metrics emitted by your handler (via `@opentelemetry/*`, `opentelemetry-distro`, `opentelemetry-javaagent`, etc.).
- Synthesizes **`aws.lambda.enhanced.*` metrics** (duration, billed duration, max memory used, memory utilization, cold starts, timeouts, out-of-memory, invocations, errors) directly from `platform.report` telemetry — the same signals that make Datadog's Lambda dashboard tick.
- Forwards everything **gzip-compressed** over HTTPS directly to OpenObserve. Nothing goes through CloudWatch Metric Streams, CloudWatch Logs subscription filters, or Firehose.

### How it works

```
┌────────────────── Lambda execution environment ──────────────────┐
│                                                                  │
│  handler code                                                    │
│      │                                                           │
│      │  OTLP HTTP  (spans + custom metrics)                      │
│      ▼                                                           │
│  localhost:4318 ─────────────┐                                   │
│                              │                                   │
│  stdout/stderr               │                                   │
│      │                       │                                   │
│      │  Lambda Telemetry API │                                   │
│      ▼                       ▼                                   │
│  ┌─────────────────────────────────────────────────────┐         │
│  │   o2-lambda-extension (Rust sidecar)                │         │
│  │   • buffers logs, spans, metrics                    │         │
│  │   • synthesizes aws.lambda.enhanced.* from REPORT   │         │
│  │   • gzips payloads                                  │         │
│  │   • flushes on end-of-invocation + background timer │         │
│  └────────────┬────────────────────────────────────────┘         │
│               │                                                  │
└───────────────┼──────────────────────────────────────────────────┘
                │  HTTPS
                ▼
        OpenObserve
        /_json          (logs)
        /v1/metrics     (metrics — enhanced + application)
        /v1/traces      (traces)
```

### Signals collected

| Signal | Source | Destination |
|---|---|---|
| **Logs** — function stdout/stderr + platform events | Lambda Telemetry API subscription | `POST {O2_ENDPOINT}/api/{ORG}/{STREAM}/_json` |
| **Enhanced metrics** — `aws.lambda.enhanced.duration`, `.billed_duration`, `.init_duration`, `.restore_duration`, `.memory_size`, `.max_memory_used`, `.memory_utilization`, `.invocations`, `.errors`, `.timeouts`, `.out_of_memory`, `.cold_starts` | Synthesized inside the extension from `platform.report` — works on **all runtimes** without any code change | `POST {O2_ENDPOINT}/api/{ORG}/v1/metrics` |
| **Application metrics** — anything your handler emits via the OTel SDK (custom counters, histograms, gauges — including `tenant_id`-tagged metrics) | OTLP HTTP `POST /v1/metrics` on `localhost:4318` | forwarded to `/v1/metrics` |
| **Traces** — spans from your handler + auto-instrumented HTTP/DB/etc. calls | OTLP HTTP `POST /v1/traces` on `localhost:4318` | forwarded to `/v1/traces` |

## 📦 Layer variants

Each runtime bundles its own OpenTelemetry SDK. Pick the variant matching your Lambda runtime, or use `core` if you wire your own SDK.

| Variant | Bundles | Best for |
|---|---|---|
| `node`   | `/opt/nodejs/node_modules/@opentelemetry/auto-instrumentations-node` | Node.js runtimes |
| `python` | `/opt/python/*` (opentelemetry-distro + `sitecustomize.py`) | Python runtimes |
| `java`   | `/opt/java/opentelemetry-javaagent.jar` | Java runtimes |
| `core`   | extension binary only | Go, Ruby, .NET, or any runtime where you supply your own OTel SDK |

Each variant is produced per architecture: `x86_64` and `arm64`.

Build output naming: `target/o2-lambda-extension-{runtime}-{arch}.zip` plus a backward-compat alias `target/o2-lambda-extension-{arch}.zip` → node variant.

## 🚀 Quick start

### 1. Build

```bash
git clone <your-repo>
cd openobserve-lambda-extension

# All runtimes × all architectures (default)
./build.sh

# Restrict what gets built:
BUILD_RUNTIMES=node,core ./build.sh                          # only node + core
BUILD_TARGETS=aarch64-unknown-linux-musl ./build.sh          # arm64 only, all runtimes
BUILD_RUNTIMES=python BUILD_TARGETS=x86_64-unknown-linux-musl ./build.sh

# Just re-package (skip Rust compile; use when only the wrapper or SDK version changed):
./build.sh repackage
```

Requirements: `cargo`, `zip`, Docker (for cross-compile on macOS), and per-runtime tools where applicable (`npm`, `pip3`, `curl`). Missing per-runtime tools produce a warning and skip that variant only — the build doesn't fail overall.

### 2. Deploy

```bash
# Deploy every produced zip as its own layer under a shared prefix
./deploy.sh

# Common overrides
AWS_REGION=us-west-2 ./deploy.sh
DEPLOY_ARCH=arm64 ./deploy.sh
```

Or manually:

```bash
aws lambda publish-layer-version \
  --layer-name openobserve-extension-node-x86_64 \
  --zip-file fileb://target/o2-lambda-extension-node-x86_64.zip \
  --compatible-architectures x86_64 \
  --compatible-runtimes nodejs18.x nodejs20.x nodejs22.x \
  --description "OpenObserve Lambda extension (node, x86_64)"
```

### 3. Configure your Lambda function

Attach the layer, set `AWS_LAMBDA_EXEC_WRAPPER`, and provide OO credentials:

```bash
aws lambda update-function-configuration \
  --function-name my-fn \
  --layers arn:aws:lambda:us-east-1:123456789012:layer:openobserve-extension-node-x86_64:1 \
  --environment 'Variables={
    AWS_LAMBDA_EXEC_WRAPPER=/opt/otel-instrument,
    O2_ORGANIZATION_ID=<your-org>,
    O2_AUTHORIZATION_HEADER=Basic <base64(user:pass)>,
    O2_ENDPOINT=https://api.openobserve.ai,
    O2_STREAM=lambda_logs,
    O2_SERVICE=my-fn,
    O2_ENV=prod
  }'
```

That's it. The `otel-instrument` wrapper auto-detects the runtime (via `AWS_EXECUTION_ENV`), loads the bundled OTel SDK for it, and points the SDK's OTLP exporters at the extension's `localhost:4318` receiver. All defaults are sensible for Lambda.

## ⚙️ Configuration

### Extension environment variables

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `O2_ORGANIZATION_ID` | ✅ | – | OpenObserve org identifier |
| `O2_AUTHORIZATION_HEADER` | ✅ | – | Full `Authorization` header value, e.g. `Basic <base64(user:pass)>` |
| `O2_ENDPOINT` | – | `https://api.openobserve.ai` | OO API base URL |
| `O2_STREAM` | – | `default` | Log stream name (metrics + traces always go to `/v1/metrics` and `/v1/traces`) |
| `O2_SERVICE` | – | falls back to `OTEL_SERVICE_NAME` → `AWS_LAMBDA_FUNCTION_NAME` | Value used as `service.name` on synthesized `aws.lambda.enhanced.*` metrics |
| `O2_ENV` | – | – | Value used as `deployment.environment` on synthesized enhanced metrics |
| `O2_EMIT_BASE_ALIASES` | – | `false` | When `true`, also emit `aws.lambda.duration` / `.errors` / `.invocations` alongside the `enhanced.*` set. Enables Datadog-shaped dashboards without a separate CloudWatch Metric Streams ingest. **Do not enable** if you already ingest CloudWatch metrics into OO — you'll double count |
| `O2_MAX_BUFFER_SIZE_MB` | – | `10` | In-memory buffer cap for log batches before oldest is dropped |
| `O2_REQUEST_TIMEOUT_MS` | – | `30000` | Outbound HTTP client timeout (per attempt) |
| `O2_MAX_RETRIES` | – | `3` | Retry attempts on 5xx / 429 / network errors |
| `O2_INITIAL_RETRY_DELAY_MS` | – | `1000` | Exponential backoff start |
| `O2_MAX_RETRY_DELAY_MS` | – | `30000` | Exponential backoff cap |
| `RUST_LOG` / `LOG_LEVEL` | – | `INFO` | Extension log verbosity (`debug` prints per-flush details in CloudWatch) |

### OTel SDK — set by the wrapper (all runtimes)

You typically don't need to touch these — the `otel-instrument` wrapper sets sensible defaults. Override only if you have a specific reason.

| Variable | Default | Notes |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4318` | Points the SDK at the in-sandbox receiver |
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | `http://localhost:4318/v1/traces` | |
| `OTEL_EXPORTER_OTLP_METRICS_ENDPOINT` | `http://localhost:4318/v1/metrics` | |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `http/protobuf` | Extension preserves Content-Type end-to-end — protobuf is ~2× smaller than JSON |
| `OTEL_SERVICE_NAME` | `${AWS_LAMBDA_FUNCTION_NAME}` | |
| `OTEL_TRACES_EXPORTER` | `otlp` | |
| `OTEL_METRICS_EXPORTER` | `otlp` | |
| `OTEL_LOGS_EXPORTER` | `none` | Logs are captured server-side via the Telemetry API, no SDK export needed |
| `OTEL_METRIC_EXPORT_INTERVAL` | `1000` (ms) | Fast so short-lived invocations don't lose counters; SDK default 60000 is too slow for Lambda |
| `OTEL_METRIC_EXPORT_TIMEOUT` | `500` (ms) | |
| `OTEL_BSP_SCHEDULE_DELAY` | `500` (ms) | Wrapper lowers the SDK default (5000 ms) so spans on low-traffic Lambdas flush before the sandbox freezes |
| `OTEL_BSP_EXPORT_TIMEOUT` | `500` (ms) | Per-batch export timeout for the span processor |

### Runtime-specific wiring (also handled by the wrapper)

| Runtime | Trigger | Effect |
|---|---|---|
| Node.js | `AWS_EXECUTION_ENV` contains `nodejs` AND `/opt/nodejs/node_modules/@opentelemetry/auto-instrumentations-node/build/src/register.js` present | Sets `NODE_OPTIONS=--require .../register.js` |
| Python  | `AWS_EXECUTION_ENV` contains `python` AND `/opt/python/sitecustomize.py` present | Sets `PYTHONPATH=/opt/python:...`, `OTEL_PYTHON_DISTRO=opentelemetry_distro` |
| Java    | `AWS_EXECUTION_ENV` contains `java` AND `/opt/java/opentelemetry-javaagent.jar` present | Sets `JAVA_TOOL_OPTIONS=-javaagent:/opt/java/opentelemetry-javaagent.jar` |

The wrapper is idempotent and honors user-set env vars (defaults use `${VAR:-value}`), so anything you set on the function config takes precedence.

## 📊 `aws.lambda.enhanced.*` — the metrics you get for free

Emitted from every `platform.report` telemetry event, tagged with `aws_request_id` and `status`, carrying resource attributes `cloud.provider=aws`, `faas.name`, `faas.version`, `cloud.region`, `faas.max_memory`, `service.name`, and `deployment.environment` (when `O2_ENV` is set).

| Metric | Type | Unit | Emitted when |
|---|---|---|---|
| `aws.lambda.enhanced.duration` | gauge | `ms` | always |
| `aws.lambda.enhanced.billed_duration` | gauge | `ms` | always |
| `aws.lambda.enhanced.init_duration` | gauge | `ms` | cold start only |
| `aws.lambda.enhanced.restore_duration` | gauge | `ms` | SnapStart only |
| `aws.lambda.enhanced.memory_size` | gauge | `MBy` | always |
| `aws.lambda.enhanced.max_memory_used` | gauge | `MBy` | always |
| `aws.lambda.enhanced.memory_utilization` | gauge | `1` (ratio 0..1) | always |
| `aws.lambda.enhanced.invocations` | delta sum (monotonic) | `1` | always (+1 per report) |
| `aws.lambda.enhanced.errors` | delta sum | `1` | `status != success` |
| `aws.lambda.enhanced.timeouts` | delta sum | `1` | `status == timeout` |
| `aws.lambda.enhanced.out_of_memory` | delta sum | `1` | `max_memory_used >= memory_size` |
| `aws.lambda.enhanced.cold_starts` | delta sum | `1` | `initDurationMs` present in report |

Dot names become underscores in PromQL: `aws_lambda_enhanced_max_memory_used` etc.

## 🆚 Coverage vs Datadog's Lambda extension

| Signal | Datadog | This extension |
|---|---|---|
| **Logs** (stdout/stderr + platform events) | ✅ Telemetry API → intake | ✅ same |
| **Enhanced metrics** (`aws.lambda.enhanced.*`) | ✅ synthesized from `platform.report` | ✅ same |
| **Application custom metrics** (with request-scoped tags) | ✅ DogStatsD UDP 8125 | ✅ via OTLP HTTP `/v1/metrics` (all OTel SDKs) — DogStatsD wire not yet supported |
| **Traces** (spans) | ✅ trace-agent HTTP 8126 | ✅ via OTLP HTTP `/v1/traces` — see [ESM caveat](#nodejs-esm-caveat) |
| **Direct HTTPS to backend** (bypass CloudWatch) | ✅ | ✅ |
| **Auto trace↔log correlation stamping** | ✅ built in | ⚠️ user configures the OTel log-correlation processor |
| **X-Ray → span shim** | ✅ | ❌ |

**Base CloudWatch metrics** (`aws.lambda.duration`, `.errors`, `.invocations`, `.throttles`, `.concurrent_executions`) come from Datadog's **CloudWatch Metric Streams integration**, not from the extension. Same in OO: those live in your `cloudwatch_metrics` stream (from CW Metric Streams → Firehose → OO). The extension is orthogonal.

**Opt-in shortcut:** if you don't have CW Metric Streams wired up but still want dashboards written against the base names to work, set `O2_EMIT_BASE_ALIASES=true`. The extension will emit `aws.lambda.duration`, `.errors`, `.invocations` alongside the `enhanced.*` variants — derived from the same `platform.report`. Do NOT enable this if you also ingest CloudWatch — you'll double-count.

## 🧪 Testing

### Tier 1 — health check (30 s, no Lambda)

```bash
unzip -o target/o2-lambda-extension-core-x86_64.zip -d /tmp/o2ext

docker run --rm -i --platform linux/amd64 \
  -v /tmp/o2ext:/opt \
  -e O2_ORGANIZATION_ID=<org> \
  -e O2_AUTHORIZATION_HEADER="Basic $(echo -n 'you:secret' | base64)" \
  -e O2_ENDPOINT=https://api.openobserve.ai \
  -e O2_STREAM=lambda_test \
  amazonlinux:2023 /opt/extensions/o2-lambda-extension --health-check
```

Expect `Status: 200`. Then check the `lambda_test` stream in OO — one row `"OpenObserve Lambda Extension health check"`.

### Tier 2 — local Lambda with AWS RIE (5 min, no AWS deploy)

Exercises every code path: extension register, Telemetry subscription, OTLP receiver, enhanced-metrics synthesis, gzip forwarding.

```bash
mkdir -p /tmp/lambda-test && cd /tmp/lambda-test
cat > index.js <<'EOF'
const { metrics, trace } = require('@opentelemetry/api');
const meter = metrics.getMeter('demo');
const invocations = meter.createCounter('demo.invocations');
const tracer = trace.getTracer('demo');

exports.handler = async (event) => {
  invocations.add(1, { tenant_id: event.tenant_id ?? 'acme' });
  const span = tracer.startSpan('handler.work');
  try {
    console.log('handler invoked at', new Date().toISOString());
    await new Promise(r => setTimeout(r, 50));
  } finally {
    span.end();
  }
  return { statusCode: 200 };
};
EOF
cat > package.json <<'EOF'
{ "name":"demo", "version":"1.0.0", "dependencies":{ "@opentelemetry/api":"^1.9.0" } }
EOF
npm install --no-package-lock --omit=dev

unzip -o /path/to/target/o2-lambda-extension-node-x86_64.zip -d /tmp/o2layer

docker run --rm --platform linux/amd64 -p 9000:8080 \
  -v $PWD:/var/task \
  -v /tmp/o2layer:/opt \
  -e AWS_LAMBDA_EXEC_WRAPPER=/opt/otel-instrument \
  -e AWS_LAMBDA_FUNCTION_NAME=demo-fn \
  -e AWS_LAMBDA_FUNCTION_MEMORY_SIZE=256 \
  -e AWS_REGION=us-east-1 \
  -e O2_ORGANIZATION_ID=<org> \
  -e O2_AUTHORIZATION_HEADER="Basic $(echo -n 'you:secret' | base64)" \
  -e O2_ENDPOINT=https://api.openobserve.ai \
  -e O2_STREAM=lambda_logs \
  -e O2_SERVICE=demo-fn \
  -e O2_ENV=dev \
  -e RUST_LOG=info \
  public.ecr.aws/lambda/nodejs:20 index.handler

# In another terminal:
for i in {1..5}; do
  curl -s -X POST http://localhost:9000/2015-03-31/functions/function/invocations -d '{}'
  echo
done
```

Check OpenObserve after ~10 s:
- `lambda_logs` stream: rows with `type='function'` and `type='platform.report'`
- Metrics: `aws_lambda_enhanced_*` plus your `demo_invocations` counter with `tenant_id="acme"`
- Traces: spans named `handler.work`, service `demo-fn`

### Tier 3 — Python handler

Same pattern, unzip `o2-lambda-extension-python-x86_64.zip`, use `public.ecr.aws/lambda/python:3.12`, handler file `lambda_function.py`.

### Tier 4 — real AWS

Publish a private layer version, attach to a scratch Lambda in a dev account. See the pattern in the [Manual AWS CLI deployment](#option-b-manual-aws-cli-deployment) section.

## 🧠 Smart flushing strategies

The extension auto-selects a flushing strategy based on the invocation pattern:

- **EndOfInvocation** — the default for functions running <10 invocations/min. Flushes logs + spans + metrics at the end of each invocation.
- **Continuous** — activated automatically once the rolling rate crosses 10 inv/min. A background task drains all three signals every 5 s so end-of-invocation stays fast.
- **Periodic** — engages after 30 s of idleness inside a long-running invocation. Timer-based flush.

On `SHUTDOWN`, the extension does an emergency synchronous flush of everything.

**Failure isolation:** a failed log POST does NOT block span/metric flushing (older versions had this bug). All three signals attempt their POSTs independently.

## 📝 Log format

Logs land at `POST {O2_ENDPOINT}/api/{ORG}/{STREAM}/_json` as gzipped JSON arrays:

```json
[
  {
    "_timestamp": 1735659296000000,
    "type": "function",
    "record": "2026-01-01T12:34:56.789Z\tINFO\tYour log message",
    "requestId": "abc123-def456-ghi789"
  },
  {
    "_timestamp": 1735659297000000,
    "type": "platform.report",
    "record": {
      "requestId": "abc123-def456-ghi789",
      "status": "success",
      "metrics": {
        "durationMs": 234.5,
        "billedDurationMs": 235,
        "memorySizeMB": 256,
        "maxMemoryUsedMB": 120,
        "initDurationMs": 1213.7
      }
    }
  }
]
```

`platform.report` events are forwarded as logs AND their numeric fields are transformed into `aws.lambda.enhanced.*` metrics — see the table above.

## ⚠️ Runtime notes

### Node.js ESM caveat

`@opentelemetry/instrumentation-aws-lambda` hooks the handler via `require-in-the-middle`, which is CJS-only. **If your handler is `.mjs` (ESM), you won't get an automatic root span for each invocation** and any child spans emitted from your handler may be lost when Lambda freezes the sandbox before `BatchSpanProcessor` fires. Workarounds:

- Convert your handler to CJS (`index.js` with `exports.handler = ...`), OR
- Start manual spans in your handler and call `provider.forceFlush()` before returning.

The wrapper already lowers `OTEL_BSP_SCHEDULE_DELAY` from the SDK default of 5000 ms to 500 ms, which usually catches spans emitted directly by handler code even in ESM — but the aws-lambda root-span wrapping only works for CJS handlers.

Metrics from ESM handlers work fine either way — the `MeterProvider` doesn't need to wrap the handler.

### Python + Java

`opentelemetry-distro`'s `sitecustomize.py` (Python) and the `-javaagent` flag (Java) work at interpreter/JVM startup — no CJS/ESM distinction. Traces and metrics both work out of the box.

### `core` variant

If you use `core`, the extension still ships logs + enhanced metrics for you (those come from the Telemetry API, no SDK needed). You'll need to install and configure your own OTel SDK for traces and custom metrics, and point its OTLP exporter at `http://localhost:4318`.

## 🔒 Security

- All outbound traffic uses HTTPS/TLS.
- Credentials are read from environment variables. Consider AWS Secrets Manager for extra hardening (fetch at cold start, cache in-process).
- The OTLP receiver binds `127.0.0.1:4318` only — not reachable outside the Lambda sandbox.
- The extension never logs credential values.

## 🛠️ Development

### Build from source

```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
./build.sh
cargo test
```

### Project structure

```
├── Cargo.toml
├── otel-instrument             # Bash wrapper: runtime detection + OTel env defaults
├── build.sh                    # Cross-compile Rust + install per-runtime SDKs + zip
├── deploy.sh                   # aws lambda publish-layer-version per variant/arch
└── src/
    ├── main.rs                 # Extension entry point + lifecycle loop
    ├── config.rs               # O2_* env var parsing + URL builders
    ├── extension.rs            # Extensions API client + flushing strategies
    ├── telemetry.rs            # Telemetry API subscriber; feeds logs + platform events
    ├── enhanced_metrics.rs     # Synthesizes aws.lambda.enhanced.* from platform.report
    ├── otlp_receiver.rs        # HTTP receiver on :4318 for /v1/traces + /v1/metrics
    ├── openobserve.rs          # OO logs POST client (retry + gzip)
    └── compress.rs             # gzip helper shared by all forwarders
```

## 📈 Performance

- **Binary size**: ~3.6 MB (static-musl, stripped)
- **Cold start impact**: ~70 ms
- **Runtime CPU impact**: negligible (async flush, buffered writes)
- **Memory**: ~10 MB
- **Network**: gzip on the wire (~4–6× reduction on OTLP payloads, ~10× on logs)
- **Compression**: gzip via `flate2`; content-type preserved end-to-end so protobuf stays protobuf

## 🆘 Troubleshooting

Turn on `RUST_LOG=debug` briefly. You should see:

```
🔌 OTLP receiver listening on localhost:4318
📥 Buffered <N> bytes of metric data          # OTel SDK exported to us
📥 Buffered <N> bytes of trace data           # OTel SDK exported to us
📊 Synthesized enhanced metrics for request <id>
🔍 Forwarding <N> bytes of metrics (<N>→<M> bytes gzipped) to https://.../v1/metrics
✅ metrics forwarded successfully
✅ Successfully sent batch of <N> events - Status: 200 OK
```

Common failure modes:

| Symptom | Likely cause | Fix |
|---|---|---|
| `401 Unauthorized` from OO on log POST | `O2_AUTHORIZATION_HEADER` malformed | Value must include `Basic ` prefix followed by base64(user:pass) |
| `❌ Failed to forward metrics: operation timed out` | HTTP client timeout too short for cold TLS | Raise `O2_REQUEST_TIMEOUT_MS` (default 30000 is fine unless overridden) |
| `📥 Buffered X bytes of trace data` never appears | Handler is ESM AND no manual spans emitted | See [Node.js ESM caveat](#nodejs-esm-caveat) |
| `📥 Buffered X bytes of metric data` never appears | OTel SDK not loaded — wrapper didn't detect runtime | Check `AWS_LAMBDA_EXEC_WRAPPER=/opt/otel-instrument` is set and the layer variant matches your runtime |
| Metrics buffered but never forwarded | Log endpoint failing AND you're on an old version (v1) | Update to a newer layer — the log/span/metric flush paths are now independent |
| `Configuration error: O2_ORGANIZATION_ID environment variable is required` | Required env vars missing | Set `O2_ORGANIZATION_ID` and `O2_AUTHORIZATION_HEADER` |

## 📄 License

MIT License. See [LICENSE.txt](LICENSE.txt).

## 🆘 Support

- **Issues**: [GitHub Issues](https://github.com/openobserve/openobserve-lambda-extension/issues)
- **Docs**: [OpenObserve Docs](https://openobserve.ai/docs)
- **Community**: [OpenObserve Slack](https://short.openobserve.ai/community)

---

**Built with ❤️ in Rust for maximum performance and reliability.**
