#!/bin/bash

set -eo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
EXTENSION_NAME="o2-lambda-extension"
BUILD_DIR="target/lambda"

# Architecture targets for Lambda
TARGETS=("x86_64-unknown-linux-musl" "aarch64-unknown-linux-musl")
ARCH_NAMES=("x86_64" "arm64")

# Runtime variants — each produces a separately-packaged layer that bundles
# the extension binary + the OpenTelemetry SDK for one Lambda runtime.
#   node   – Node.js auto-instrumentation via /opt/nodejs/node_modules
#   python – Python opentelemetry-distro under /opt/python
#   java   – opentelemetry-javaagent.jar under /opt/java
#   core   – extension binary only, for Go / Ruby / .NET / bring-your-own-SDK
RUNTIMES=("node" "python" "java" "core")

# Package versions (pinned so builds are reproducible)
NODE_OTEL_AUTO_VERSION="0.57.0"
# NOTE: opentelemetry-distro uses 0.5xbY pre-release versioning; the exporter
# package uses 1.x.y. Leave the exporter unpinned so pip resolves it against
# whatever distro version we install.
PYTHON_OTEL_DISTRO_VERSION="0.53b0"
JAVA_OTEL_AGENT_VERSION="2.11.0"               # opentelemetry-javaagent

# Defaults (can be overridden via env)
BUILD_TARGETS="${BUILD_TARGETS:-all}"
BUILD_RUNTIMES="${BUILD_RUNTIMES:-all}"

echo -e "${BLUE}🚀 Building OpenObserve Lambda Extension${NC}"
echo -e "${BLUE}📦 Architectures: ${BUILD_TARGETS}   Runtimes: ${BUILD_RUNTIMES}${NC}"

# -----------------------------------------------------------------------------
# Helpers
# -----------------------------------------------------------------------------

# Return the set of architectures to build, honoring BUILD_TARGETS.
selected_archs() {
    if [ "$BUILD_TARGETS" = "all" ]; then
        for i in "${!TARGETS[@]}"; do echo "${TARGETS[$i]}:${ARCH_NAMES[$i]}"; done
    else
        for i in "${!TARGETS[@]}"; do
            if [ "${TARGETS[$i]}" = "$BUILD_TARGETS" ]; then
                echo "${TARGETS[$i]}:${ARCH_NAMES[$i]}"
            fi
        done
    fi
}

# Return the set of runtimes to package, honoring BUILD_RUNTIMES.
selected_runtimes() {
    if [ "$BUILD_RUNTIMES" = "all" ]; then
        for r in "${RUNTIMES[@]}"; do echo "$r"; done
    else
        # Comma-separated allowed: BUILD_RUNTIMES=node,python
        IFS=',' read -ra requested <<< "$BUILD_RUNTIMES"
        for r in "${requested[@]}"; do
            r=$(echo "$r" | tr -d '[:space:]')
            for v in "${RUNTIMES[@]}"; do
                if [ "$r" = "$v" ]; then echo "$r"; fi
            done
        done
    fi
}

check_requirements() {
    echo -e "${YELLOW}📋 Checking requirements...${NC}"

    for tool in cargo zip; do
        if ! command -v "$tool" &> /dev/null; then
            echo -e "${RED}❌ Error: $tool is not installed${NC}"
            exit 1
        fi
    done

    # Optional per-runtime tools; warn only.
    for r in $(selected_runtimes); do
        case "$r" in
            node)   command -v npm  >/dev/null || echo -e "${YELLOW}⚠️  npm not found — node variant will be skipped per-arch${NC}" ;;
            python) command -v pip3 >/dev/null || echo -e "${YELLOW}⚠️  pip3 not found — python variant will be skipped per-arch${NC}" ;;
            java)   command -v curl >/dev/null || echo -e "${YELLOW}⚠️  curl not found — java variant will be skipped per-arch${NC}" ;;
        esac
    done

    echo -e "${GREEN}✅ Requirements ok${NC}"
}

setup_targets() {
    echo -e "${YELLOW}🎯 Setting up build targets...${NC}"
    for pair in $(selected_archs); do
        target="${pair%%:*}"
        echo -e "${BLUE}  rustup target add $target${NC}"
        rustup target add "$target" >/dev/null
    done
    echo -e "${GREEN}✅ Targets ready${NC}"
}

clean_build() {
    echo -e "${YELLOW}🧹 Cleaning previous builds...${NC}"
    rm -rf "$BUILD_DIR"
    rm -f target/o2-lambda-extension-*.zip
    cargo clean
}

# -----------------------------------------------------------------------------
# Compilation (Rust extension binary)
# -----------------------------------------------------------------------------

build_for_target() {
    local target=$1
    local arch_name=$2

    echo -e "${YELLOW}🔨 Building extension binary for $arch_name ($target)...${NC}"

    if [[ "$OSTYPE" == "darwin"* ]]; then
        if ! command -v docker &> /dev/null; then
            echo -e "${RED}❌ Error: Docker is required for cross-compilation on macOS${NC}"
            echo -e "${YELLOW}Install Docker Desktop: https://www.docker.com/products/docker-desktop${NC}"
            exit 1
        fi

        docker run --rm \
            -v "$PWD":/workspace \
            -w /workspace \
            --platform linux/amd64 \
            rust:1.89 sh -c "
                apt-get update &&
                apt-get install -y musl-tools musl-dev build-essential &&
                rustup target add $target &&
                if [ '$target' = 'x86_64-unknown-linux-musl' ]; then
                    export CC=musl-gcc
                elif [ '$target' = 'aarch64-unknown-linux-musl' ]; then
                    apt-get install -y gcc-aarch64-linux-gnu &&
                    export CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc &&
                    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc
                fi &&
                cargo build --release --target $target
            "
    else
        if [ "$target" = "x86_64-unknown-linux-musl" ]; then
            export CC_x86_64_unknown_linux_musl=musl-gcc
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
        elif [ "$target" = "aarch64-unknown-linux-musl" ]; then
            export CC_aarch64_unknown_linux_musl=aarch64-linux-musl-gcc
            export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-musl-gcc
        fi
        cargo build --release --target "$target"
    fi

    echo -e "${GREEN}✅ Built $arch_name${NC}"
}

build_extensions() {
    echo -e "${YELLOW}🔨 Building extension binaries for all selected architectures...${NC}"
    for pair in $(selected_archs); do
        target="${pair%%:*}"
        arch="${pair##*:}"
        build_for_target "$target" "$arch"
    done
}

# -----------------------------------------------------------------------------
# Per-runtime SDK bundles
# -----------------------------------------------------------------------------

install_node_sdk() {
    local dir=$1
    if ! command -v npm >/dev/null; then
        echo -e "${YELLOW}  ⚠ npm missing, skipping node bundle${NC}"
        return 1
    fi

    mkdir -p "$dir/nodejs"
    cat > "$dir/nodejs/package.json" <<EOF
{
  "name": "o2-otel-layer-node",
  "version": "1.0.0",
  "dependencies": {
    "@opentelemetry/auto-instrumentations-node": "^${NODE_OTEL_AUTO_VERSION}",
    "@opentelemetry/api": "^1.9.0"
  }
}
EOF
    (cd "$dir/nodejs" && npm install --omit=dev --no-package-lock 2>&1 | tail -3)
}

install_python_sdk() {
    local dir=$1
    if ! command -v pip3 >/dev/null; then
        echo -e "${YELLOW}  ⚠ pip3 missing, skipping python bundle${NC}"
        return 1
    fi

    mkdir -p "$dir/python"
    # opentelemetry-distro pulls in the SDK, bootstrap machinery, and
    # sitecustomize.py. Exporter uses a different (1.x.y) version stream and
    # is resolved by pip against the distro version.
    if ! pip3 install \
        --target="$dir/python" \
        --no-compile \
        --upgrade \
        "opentelemetry-distro==${PYTHON_OTEL_DISTRO_VERSION}" \
        "opentelemetry-exporter-otlp-proto-http" \
        > "$dir/python-install.log" 2>&1
    then
        echo -e "${RED}  ✗ pip install failed (see $dir/python-install.log)${NC}"
        tail -5 "$dir/python-install.log"
        return 1
    fi

    # Copy sitecustomize.py to /opt/python top level so Python auto-imports it
    # at interpreter startup (Lambda adds /opt/python to sys.path).
    local site_src="$dir/python/opentelemetry/instrumentation/auto_instrumentation/sitecustomize.py"
    if [ -f "$site_src" ]; then
        cp "$site_src" "$dir/python/sitecustomize.py"
    else
        echo -e "${YELLOW}  ⚠ sitecustomize.py not found at expected path — auto-inst may not activate${NC}"
    fi
}

install_java_agent() {
    local dir=$1
    if ! command -v curl >/dev/null; then
        echo -e "${YELLOW}  ⚠ curl missing, skipping java bundle${NC}"
        return 1
    fi

    mkdir -p "$dir/java"
    local url="https://github.com/open-telemetry/opentelemetry-java-instrumentation/releases/download/v${JAVA_OTEL_AGENT_VERSION}/opentelemetry-javaagent.jar"
    curl -fsSL -o "$dir/java/opentelemetry-javaagent.jar" "$url"
    du -h "$dir/java/opentelemetry-javaagent.jar" | awk '{print "    → downloaded", $1}'
}

# -----------------------------------------------------------------------------
# Layer packaging (per (arch, runtime))
# -----------------------------------------------------------------------------

create_variant() {
    local target=$1
    local arch=$2
    local runtime=$3

    local variant_dir="$BUILD_DIR/$arch/$runtime"
    local package_name="o2-lambda-extension-$runtime-$arch.zip"

    echo -e "${YELLOW}📁 Packaging $runtime layer for $arch...${NC}"

    mkdir -p "$variant_dir/extensions"
    cp "target/$target/release/$EXTENSION_NAME" "$variant_dir/extensions/"
    chmod +x "$variant_dir/extensions/$EXTENSION_NAME"

    cp otel-instrument "$variant_dir/otel-instrument"
    chmod +x "$variant_dir/otel-instrument"

    local sdk_bundled=1
    case "$runtime" in
        node)   install_node_sdk   "$variant_dir" || sdk_bundled=0 ;;
        python) install_python_sdk "$variant_dir" || sdk_bundled=0 ;;
        java)   install_java_agent "$variant_dir" || sdk_bundled=0 ;;
        core)   sdk_bundled=1 ;;  # no bundle by design
    esac

    if [ "$sdk_bundled" = 0 ] && [ "$runtime" != "core" ]; then
        echo -e "${YELLOW}  ⚠ Skipping $runtime-$arch package (missing SDK tool)${NC}"
        return 0
    fi

    # Contents to include in the zip depend on runtime
    local extras=()
    case "$runtime" in
        node)   extras=("nodejs") ;;
        python) extras=("python") ;;
        java)   extras=("java") ;;
    esac

    (cd "$variant_dir" && zip -qr "../../../$package_name" extensions/ otel-instrument "${extras[@]}")
    local size
    size=$(du -h "target/$package_name" | cut -f1)
    echo -e "${GREEN}✅ target/$package_name ($size)${NC}"
}

create_layer_variants() {
    echo -e "${YELLOW}📁 Creating layer variants...${NC}"
    for pair in $(selected_archs); do
        target="${pair%%:*}"
        arch="${pair##*:}"
        for runtime in $(selected_runtimes); do
            create_variant "$target" "$arch" "$runtime"
        done

        # Backward-compat alias: existing published layer name maps to the node variant.
        if [ -f "target/o2-lambda-extension-node-$arch.zip" ]; then
            cp "target/o2-lambda-extension-node-$arch.zip" "target/o2-lambda-extension-$arch.zip"
            echo -e "${BLUE}  🔗 alias: o2-lambda-extension-$arch.zip → node variant${NC}"
        fi
    done
}

# -----------------------------------------------------------------------------
# Validation
# -----------------------------------------------------------------------------

validate_packages() {
    echo -e "${YELLOW}🔍 Validating packages...${NC}"
    local any_missing=0
    for pair in $(selected_archs); do
        arch="${pair##*:}"
        for runtime in $(selected_runtimes); do
            local pkg="target/o2-lambda-extension-$runtime-$arch.zip"
            if [ ! -f "$pkg" ]; then
                echo -e "${YELLOW}  ⚠ $pkg not produced (tool missing?)${NC}"
                any_missing=1
                continue
            fi
            local sz
            sz=$(du -h "$pkg" | cut -f1)
            echo -e "  ✅ $pkg ($sz)"
        done
    done
    if [ "$any_missing" = 0 ]; then
        echo -e "${GREEN}✅ All packages present${NC}"
    fi
}

# -----------------------------------------------------------------------------
# Main
# -----------------------------------------------------------------------------

main() {
    echo -e "${BLUE}Starting build process...${NC}\n"
    check_requirements
    setup_targets
    # NOTE: no clean_build here — cargo is incremental, and a stray shell/wrapper
    # change shouldn't force a Rust recompile. Run `./build.sh clean` explicitly
    # if you need to wipe artifacts.
    build_extensions
    create_layer_variants
    validate_packages

    echo -e "\n${GREEN}🎉 Build complete.${NC}"
    echo -e "${BLUE}📦 Artifacts under target/${NC}"
    ls -1 target/o2-lambda-extension-*.zip 2>/dev/null | sed 's/^/  - /'

    cat <<'EOM'

📚 Which layer to attach:
  • Node.js runtimes            → o2-lambda-extension-node-<arch>.zip
  • Python runtimes             → o2-lambda-extension-python-<arch>.zip
  • Java runtimes               → o2-lambda-extension-java-<arch>.zip
  • Go / Ruby / .NET / custom   → o2-lambda-extension-core-<arch>.zip
                                  (extension only, wire your own OTel SDK)

📌 Required env vars on the function:
  O2_ORGANIZATION_ID=<your org>
  O2_AUTHORIZATION_HEADER="Basic <base64(user:pass)>"

📌 Optional:
  O2_ENDPOINT=https://api.openobserve.ai    (default)
  O2_STREAM=default                          (default log stream)
  O2_SERVICE=<service.name>                  (added to enhanced metrics)
  O2_ENV=<deployment.environment>            (added to enhanced metrics)

📌 Handler wrapper (set on the function):
  AWS_LAMBDA_EXEC_WRAPPER=/opt/otel-instrument

EOM
}

# -----------------------------------------------------------------------------
# CLI
# -----------------------------------------------------------------------------

case "${1:-build}" in
    "build") main ;;
    "repackage")
        # Re-run only the packaging step against existing extension binaries.
        # Useful when you've only changed the wrapper or SDK bundle versions.
        echo -e "${BLUE}Repackaging (skip Rust build)...${NC}\n"
        check_requirements
        create_layer_variants
        validate_packages
        ;;
    "clean")
        echo -e "${YELLOW}🧹 Cleaning...${NC}"
        rm -rf "$BUILD_DIR"
        rm -f target/o2-lambda-extension-*.zip
        cargo clean
        echo -e "${GREEN}✅ Clean done${NC}"
        ;;
    "test") cargo test ;;
    "check")
        for pair in $(selected_archs); do
            target="${pair%%:*}"
            echo -e "${BLUE}cargo check --target $target${NC}"
            cargo check --target "$target"
        done
        ;;
    "help"|"-h"|"--help")
        cat <<EOF
${BLUE}OpenObserve Lambda Extension Build Script${NC}

Usage: $0 [command]

Commands:
  build    Build extension + package layer variants (default)
  clean    Remove build artifacts
  test     cargo test
  check    cargo check per architecture
  help     This message

Environment variables:
  BUILD_TARGETS   'all' (default) or a specific rust target triple
                  ('x86_64-unknown-linux-musl' | 'aarch64-unknown-linux-musl')
  BUILD_RUNTIMES  'all' (default) or comma-separated subset:
                  node,python,java,core

Examples:
  $0                                             # all archs × all runtimes
  BUILD_RUNTIMES=node,core $0                    # only node + core, both archs
  BUILD_TARGETS=aarch64-unknown-linux-musl $0    # arm64 only, all runtimes
  BUILD_RUNTIMES=node BUILD_TARGETS=x86_64-unknown-linux-musl $0

Output naming:
  target/o2-lambda-extension-<runtime>-<arch>.zip
  target/o2-lambda-extension-<arch>.zip   (backward-compat alias → node variant)
EOF
        ;;
    *)
        echo -e "${RED}❌ Unknown command: $1${NC}"
        echo "Use '$0 help' for usage information"
        exit 1
        ;;
esac
