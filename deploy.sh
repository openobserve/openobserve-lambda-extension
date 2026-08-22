#!/bin/bash

set -eo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Configuration
LAYER_NAME_PREFIX="openobserve-extension"
ARCHITECTURES=("x86_64" "arm64")
RUNTIMES=("node" "python" "java" "core")

# Runtime → compatible AWS Lambda runtime identifiers.
# Keep in sync with what each variant's bundled SDK actually supports.
compat_runtimes_for() {
    case "$1" in
        node)   echo "nodejs18.x nodejs20.x nodejs22.x" ;;
        python) echo "python3.9 python3.10 python3.11 python3.12 python3.13" ;;
        java)   echo "java11 java17 java21" ;;
        # core has no bundled SDK — declare compatibility with everything so
        # Go/Ruby/.NET/provided.al2* users can attach it.
        core)   echo "python3.9 python3.10 python3.11 python3.12 python3.13 nodejs18.x nodejs20.x nodejs22.x java11 java17 java21 dotnet8 ruby3.3 provided.al2 provided.al2023" ;;
        *)      echo "" ;;
    esac
}

# Defaults (env-overridable)
DEPLOY_ARCH="${DEPLOY_ARCH:-all}"
DEPLOY_RUNTIMES="${DEPLOY_RUNTIMES:-all}"
DEPLOY_LEGACY_ALIAS="${DEPLOY_LEGACY_ALIAS:-1}"   # also publish the old openobserve-extension-<arch> name
AWS_REGION="${AWS_REGION:-us-east-1}"

echo -e "${BLUE}🚀 OpenObserve Lambda Layer Deployment${NC}"
echo -e "${BLUE}Region: $AWS_REGION   Archs: $DEPLOY_ARCH   Runtimes: $DEPLOY_RUNTIMES${NC}"

# -----------------------------------------------------------------------------
# Helpers
# -----------------------------------------------------------------------------

selected_archs() {
    if [ "$DEPLOY_ARCH" = "all" ]; then
        printf '%s\n' "${ARCHITECTURES[@]}"
    else
        IFS=',' read -ra requested <<< "$DEPLOY_ARCH"
        for r in "${requested[@]}"; do
            r=$(echo "$r" | tr -d '[:space:]')
            for a in "${ARCHITECTURES[@]}"; do [ "$r" = "$a" ] && echo "$a"; done
        done
    fi
}

selected_runtimes() {
    if [ "$DEPLOY_RUNTIMES" = "all" ]; then
        printf '%s\n' "${RUNTIMES[@]}"
    else
        IFS=',' read -ra requested <<< "$DEPLOY_RUNTIMES"
        for r in "${requested[@]}"; do
            r=$(echo "$r" | tr -d '[:space:]')
            for v in "${RUNTIMES[@]}"; do [ "$r" = "$v" ] && echo "$r"; done
        done
    fi
}

check_aws_cli() {
    if ! command -v aws &> /dev/null; then
        echo -e "${RED}❌ AWS CLI is not installed${NC}" >&2
        exit 1
    fi
    if ! aws sts get-caller-identity &> /dev/null; then
        echo -e "${RED}❌ AWS credentials not configured${NC}" >&2
        exit 1
    fi
    ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
    echo -e "${GREEN}✅ AWS ready (account $ACCOUNT_ID, region $AWS_REGION)${NC}"
}

# -----------------------------------------------------------------------------
# Publish
# -----------------------------------------------------------------------------

publish_variant() {
    local runtime=$1
    local arch=$2
    local pkg="target/o2-lambda-extension-$runtime-$arch.zip"
    local layer_name="$LAYER_NAME_PREFIX-$runtime-$arch"
    local compat_runtimes
    compat_runtimes=$(compat_runtimes_for "$runtime")

    if [ ! -f "$pkg" ]; then
        echo -e "${YELLOW}  ⚠ $pkg missing — run ./build.sh first (or skip via DEPLOY_RUNTIMES=)${NC}"
        return 1
    fi
    local sz; sz=$(du -h "$pkg" | cut -f1)
    echo -e "${YELLOW}📤 publish $layer_name ($sz) → $AWS_REGION${NC}"

    # shellcheck disable=SC2086
    local result
    result=$(aws lambda publish-layer-version \
        --region "$AWS_REGION" \
        --layer-name "$layer_name" \
        --zip-file "fileb://$pkg" \
        --compatible-architectures "$arch" \
        --compatible-runtimes $compat_runtimes \
        --description "OpenObserve Lambda extension ($runtime, $arch)" \
        --query 'LayerVersionArn' \
        --output text 2>&1)
    local exit_code=$?

    if [ $exit_code -ne 0 ]; then
        echo -e "${RED}  ✗ publish failed: $result${NC}"
        return 1
    fi
    echo -e "${GREEN}  ✅ $result${NC}"
    printf '%s\n' "$result" >> "deployment-info-$runtime-$arch.txt"
}

publish_legacy_alias() {
    # Backward-compat: publish the pre-variant name (openobserve-extension-<arch>)
    # as an alias for the node variant. Existing customers referencing the old
    # ARN name don't have to change anything.
    local arch=$1
    local pkg="target/o2-lambda-extension-$arch.zip"
    if [ ! -f "$pkg" ]; then
        return 0  # no alias built by build.sh — skip silently
    fi
    local layer_name="$LAYER_NAME_PREFIX-$arch"
    local sz; sz=$(du -h "$pkg" | cut -f1)
    echo -e "${YELLOW}📤 publish legacy alias $layer_name ($sz) → $AWS_REGION${NC}"
    local result
    result=$(aws lambda publish-layer-version \
        --region "$AWS_REGION" \
        --layer-name "$layer_name" \
        --zip-file "fileb://$pkg" \
        --compatible-architectures "$arch" \
        --compatible-runtimes \
            python3.9 python3.10 python3.11 python3.12 python3.13 \
            nodejs18.x nodejs20.x nodejs22.x \
            java11 java17 java21 \
            dotnet8 ruby3.3 \
            provided.al2 provided.al2023 \
        --description "OpenObserve Lambda extension ($arch, node bundle) — legacy alias" \
        --query 'LayerVersionArn' \
        --output text 2>&1)
    if [ $? -eq 0 ]; then
        echo -e "${GREEN}  ✅ $result${NC}"
        printf '%s\n' "$result" >> "deployment-info-$arch.txt"
    else
        echo -e "${RED}  ✗ alias publish failed: $result${NC}"
    fi
}

deploy() {
    check_aws_cli

    local ok=0 total=0
    for arch in $(selected_archs); do
        for runtime in $(selected_runtimes); do
            total=$((total+1))
            publish_variant "$runtime" "$arch" && ok=$((ok+1)) || true
        done
        if [ "$DEPLOY_LEGACY_ALIAS" = "1" ]; then
            publish_legacy_alias "$arch"
        fi
    done

    echo ""
    echo -e "${BLUE}📊 Summary: $ok/$total variants published${NC}"

    cat <<'EOM'

📚 Attach the right variant on each Lambda function:
   Node.js runtimes            → openobserve-extension-node-<arch>
   Python runtimes             → openobserve-extension-python-<arch>
   Java runtimes               → openobserve-extension-java-<arch>
   Go / Ruby / .NET / custom   → openobserve-extension-core-<arch>

📌 Required function env vars:
   AWS_LAMBDA_EXEC_WRAPPER=/opt/otel-instrument
   O2_ORGANIZATION_ID=<your-org>
   O2_AUTHORIZATION_HEADER="Basic <base64(user:pass)>"

📌 Common optional:
   O2_ENDPOINT=https://api.openobserve.ai
   O2_STREAM=lambda_logs
   O2_SERVICE=<service.name>
   O2_ENV=<deployment.environment>
   O2_EMIT_BASE_ALIASES=true    # also emit aws.lambda.{duration,errors,invocations}
EOM

    if [ $ok -lt $total ]; then
        echo -e "${RED}⚠  Some publishes failed — see logs above${NC}"
        exit 1
    fi
}

# -----------------------------------------------------------------------------
# List
# -----------------------------------------------------------------------------

list_layers() {
    check_aws_cli
    echo -e "${YELLOW}📋 layers in $AWS_REGION starting with '$LAYER_NAME_PREFIX-'${NC}"
    for arch in $(selected_archs); do
        for runtime in $(selected_runtimes); do
            local ln="$LAYER_NAME_PREFIX-$runtime-$arch"
            local versions
            versions=$(aws lambda list-layer-versions --region "$AWS_REGION" \
                --layer-name "$ln" --query 'LayerVersions[].Version' --output text 2>/dev/null || echo "")
            if [ -z "$versions" ] || [ "$versions" = "None" ]; then
                echo -e "  · $ln  ${YELLOW}(none)${NC}"
            else
                echo -e "  · $ln  ${GREEN}versions: $versions${NC}"
            fi
        done
        # legacy alias
        local ln="$LAYER_NAME_PREFIX-$arch"
        local versions
        versions=$(aws lambda list-layer-versions --region "$AWS_REGION" \
            --layer-name "$ln" --query 'LayerVersions[].Version' --output text 2>/dev/null || echo "")
        if [ -n "$versions" ] && [ "$versions" != "None" ]; then
            echo -e "  · $ln (legacy alias)  ${GREEN}versions: $versions${NC}"
        fi
    done
}

# -----------------------------------------------------------------------------
# Delete
# -----------------------------------------------------------------------------

delete_layers() {
    check_aws_cli
    echo -e "${RED}⚠  This will delete ALL versions of the selected layers in $AWS_REGION${NC}"
    read -rp "Type 'DELETE' to confirm: " confirm
    if [ "$confirm" != "DELETE" ]; then
        echo -e "${YELLOW}cancelled${NC}"
        exit 0
    fi

    delete_one() {
        local ln=$1
        local versions
        versions=$(aws lambda list-layer-versions --region "$AWS_REGION" \
            --layer-name "$ln" --query 'LayerVersions[].Version' --output text 2>/dev/null || true)
        if [ -z "$versions" ] || [ "$versions" = "None" ]; then
            return 0
        fi
        for v in $versions; do
            echo -e "  ✗ $ln:$v"
            aws lambda delete-layer-version --region "$AWS_REGION" \
                --layer-name "$ln" --version-number "$v" >/dev/null
        done
    }

    for arch in $(selected_archs); do
        for runtime in $(selected_runtimes); do
            delete_one "$LAYER_NAME_PREFIX-$runtime-$arch"
        done
        delete_one "$LAYER_NAME_PREFIX-$arch"
    done
    echo -e "${GREEN}✅ done${NC}"
}

# -----------------------------------------------------------------------------
# Help
# -----------------------------------------------------------------------------

show_help() {
    cat <<EOF
${BLUE}OpenObserve Lambda Layer Deployment${NC}

Usage: $0 [command]

Commands:
  deploy   Publish new layer version(s) (default)
  list     List existing layer versions
  delete   Delete all layer versions (requires typing 'DELETE')
  help     Show this help

Environment variables:
  AWS_REGION           default: us-east-1
  DEPLOY_ARCH          'all' (default) or comma-separated: x86_64,arm64
  DEPLOY_RUNTIMES      'all' (default) or comma-separated: node,python,java,core
  DEPLOY_LEGACY_ALIAS  '1' (default) publishes openobserve-extension-<arch>
                       as an alias pointing at the node bundle for backward
                       compatibility with pre-per-runtime deployments.
                       Set to '0' to skip.

Layer name pattern:
  ${LAYER_NAME_PREFIX}-<runtime>-<arch>
    e.g. ${LAYER_NAME_PREFIX}-node-arm64, ${LAYER_NAME_PREFIX}-python-x86_64

  Legacy alias (when DEPLOY_LEGACY_ALIAS=1 and target/o2-lambda-extension-<arch>.zip exists):
  ${LAYER_NAME_PREFIX}-<arch>     (points to the node bundle)

Examples:
  $0                                            # publish everything present in target/
  DEPLOY_ARCH=arm64 DEPLOY_RUNTIMES=node $0     # only node-arm64
  DEPLOY_LEGACY_ALIAS=0 DEPLOY_RUNTIMES=node,python $0
  AWS_REGION=eu-west-1 $0 list
EOF
}

# -----------------------------------------------------------------------------
# CLI
# -----------------------------------------------------------------------------

case "${1:-deploy}" in
    deploy)         deploy ;;
    list)           list_layers ;;
    delete)         delete_layers ;;
    help|-h|--help) show_help ;;
    *)
        echo -e "${RED}Unknown command: $1${NC}"
        show_help
        exit 1
        ;;
esac
