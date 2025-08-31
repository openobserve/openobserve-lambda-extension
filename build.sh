#!/bin/bash

set -e

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

# Default to building all architectures, or use environment variable
BUILD_TARGETS="${BUILD_TARGETS:-all}"

echo -e "${BLUE}🚀 Building OpenObserve Lambda Extension${NC}"
if [ "$BUILD_TARGETS" = "all" ]; then
    echo -e "${BLUE}📦 Building for all architectures: x86_64 + arm64${NC}"
else
    # Find the architecture name for the specified target
    arch_name="unknown"
    for i in "${!TARGETS[@]}"; do
        if [ "${TARGETS[$i]}" = "$BUILD_TARGETS" ]; then
            arch_name="${ARCH_NAMES[$i]}"
            break
        fi
    done
    echo -e "${BLUE}📦 Building for architecture: $arch_name ($BUILD_TARGETS)${NC}"
fi

# Check if required tools are installed
check_requirements() {
    echo -e "${YELLOW}📋 Checking requirements...${NC}"
    
    if ! command -v cargo &> /dev/null; then
        echo -e "${RED}❌ Error: cargo is not installed${NC}"
        exit 1
    fi
    
    if ! command -v zip &> /dev/null; then
        echo -e "${RED}❌ Error: zip is not installed${NC}"
        exit 1
    fi
    
    echo -e "${GREEN}✅ All requirements met${NC}"
}

# Add targets if not already added
setup_targets() {
    echo -e "${YELLOW}🎯 Setting up build targets...${NC}"
    
    if [ "$BUILD_TARGETS" = "all" ]; then
        for target in "${TARGETS[@]}"; do
            echo -e "${BLUE}  Adding target: $target${NC}"
            rustup target add $target
        done
    else
        echo -e "${BLUE}  Adding target: $BUILD_TARGETS${NC}"
        rustup target add $BUILD_TARGETS
    fi
    
    echo -e "${GREEN}✅ Targets setup completed${NC}"
}

# Clean previous builds
clean_build() {
    echo -e "${YELLOW}🧹 Cleaning previous builds...${NC}"
    rm -rf $BUILD_DIR
    cargo clean
}

# Build the extension for a specific target
build_for_target() {
    local target=$1
    local arch_name=$2
    
    echo -e "${YELLOW}🔨 Building for $arch_name ($target)...${NC}"
    
    # Check if we're on macOS and need Docker
    if [[ "$OSTYPE" == "darwin"* ]]; then
        echo -e "${BLUE}🐳 Using Docker for cross-compilation on macOS...${NC}"
        
        if ! command -v docker &> /dev/null; then
            echo -e "${RED}❌ Error: Docker is required for cross-compilation on macOS${NC}"
            echo -e "${YELLOW}Please install Docker Desktop from https://www.docker.com/products/docker-desktop${NC}"
            exit 1
        fi
        
        # Use cross-compilation container for reliable builds
        docker run --rm \
            -v "$PWD":/workspace \
            -w /workspace \
            --platform linux/amd64 \
            rust:1.89 sh -c "
                # Install musl tools and cross-compilation support
                apt-get update &&
                apt-get install -y musl-tools musl-dev build-essential &&
                
                # Add the specific target
                rustup target add $target &&
                
                # Set up cross-compilation environment
                if [ '$target' = 'x86_64-unknown-linux-musl' ]; then
                    # Native compilation on x86_64 container
                    export CC=musl-gcc
                elif [ '$target' = 'aarch64-unknown-linux-musl' ]; then
                    # Install ARM64 cross-compiler
                    apt-get install -y gcc-aarch64-linux-gnu &&
                    export CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc &&
                    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc
                fi &&
                
                # Build the project
                cargo build --release --target $target
            "
    else
        # Set environment variables for cross-compilation on Linux
        if [ "$target" = "x86_64-unknown-linux-musl" ]; then
            export CC_x86_64_unknown_linux_musl=musl-gcc
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
        elif [ "$target" = "aarch64-unknown-linux-musl" ]; then
            export CC_aarch64_unknown_linux_musl=aarch64-linux-musl-gcc
            export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-musl-gcc
        fi
        
        # Build with musl target for Lambda
        cargo build --release --target $target
    fi
    
    if [ $? -ne 0 ]; then
        echo -e "${RED}❌ Build failed for $arch_name${NC}"
        return 1
    fi
    
    echo -e "${GREEN}✅ Build completed successfully for $arch_name${NC}"
}

# Build extensions for all specified targets
build_extensions() {
    echo -e "${YELLOW}🔨 Building extensions...${NC}"
    
    if [ "$BUILD_TARGETS" = "all" ]; then
        for i in "${!TARGETS[@]}"; do
            build_for_target "${TARGETS[$i]}" "${ARCH_NAMES[$i]}"
            if [ $? -ne 0 ]; then
                echo -e "${RED}❌ Build process failed${NC}"
                exit 1
            fi
        done
    else
        # Find the architecture name for the specified target
        arch_name="unknown"
        for i in "${!TARGETS[@]}"; do
            if [ "${TARGETS[$i]}" = "$BUILD_TARGETS" ]; then
                arch_name="${ARCH_NAMES[$i]}"
                break
            fi
        done
        build_for_target "$BUILD_TARGETS" "$arch_name"
        if [ $? -ne 0 ]; then
            echo -e "${RED}❌ Build process failed${NC}"
            exit 1
        fi
    fi
    
    echo -e "${GREEN}✅ All builds completed successfully${NC}"
}

# Create the Lambda layer structure for a specific target
create_layer_structure_for_target() {
    local target=$1
    local arch_name=$2
    local package_dir="$BUILD_DIR/$arch_name/extensions"
    
    echo -e "${YELLOW}📁 Creating Lambda layer structure for $arch_name...${NC}"
    
    # Create directories
    mkdir -p "$package_dir"
    
    # Copy the binary to the extensions directory
    cp "target/$target/release/$EXTENSION_NAME" "$package_dir/"
    
    # Make sure the binary is executable
    chmod +x "$package_dir/$EXTENSION_NAME"
    
    echo -e "${GREEN}✅ Layer structure created for $arch_name${NC}"
}

# Create layer structures for all built targets
create_layer_structures() {
    echo -e "${YELLOW}📁 Creating Lambda layer structures...${NC}"
    
    if [ "$BUILD_TARGETS" = "all" ]; then
        for i in "${!TARGETS[@]}"; do
            create_layer_structure_for_target "${TARGETS[$i]}" "${ARCH_NAMES[$i]}"
        done
    else
        # Find the architecture name for the specified target
        arch_name="unknown"
        for i in "${!TARGETS[@]}"; do
            if [ "${TARGETS[$i]}" = "$BUILD_TARGETS" ]; then
                arch_name="${ARCH_NAMES[$i]}"
                break
            fi
        done
        create_layer_structure_for_target "$BUILD_TARGETS" "$arch_name"
    fi
    
    echo -e "${GREEN}✅ All layer structures created${NC}"
}

# Create deployment package for a specific architecture
create_package_for_target() {
    local arch_name=$1
    local package_name="o2-lambda-extension-$arch_name.zip"
    
    echo -e "${YELLOW}📦 Creating deployment package for $arch_name...${NC}"
    
    cd "$BUILD_DIR/$arch_name"
    zip -r "../../$package_name" extensions/
    cd - > /dev/null
    
    PACKAGE_SIZE=$(du -h "target/$package_name" | cut -f1)
    echo -e "${GREEN}✅ Package created: target/$package_name ($PACKAGE_SIZE)${NC}"
}

# Create deployment packages for all built targets
create_packages() {
    echo -e "${YELLOW}📦 Creating deployment packages...${NC}"
    
    if [ "$BUILD_TARGETS" = "all" ]; then
        for i in "${!ARCH_NAMES[@]}"; do
            create_package_for_target "${ARCH_NAMES[$i]}"
        done
        
        echo -e "${BLUE}📋 Available packages:${NC}"
        for arch in "${ARCH_NAMES[@]}"; do
            if [ -f "target/o2-lambda-extension-$arch.zip" ]; then
                PACKAGE_SIZE=$(du -h "target/o2-lambda-extension-$arch.zip" | cut -f1)
                echo -e "  - target/o2-lambda-extension-$arch.zip ($PACKAGE_SIZE)"
            fi
        done
    else
        # Find the architecture name for the specified target
        arch_name="unknown"
        for i in "${!TARGETS[@]}"; do
            if [ "${TARGETS[$i]}" = "$BUILD_TARGETS" ]; then
                arch_name="${ARCH_NAMES[$i]}"
                break
            fi
        done
        create_package_for_target "$arch_name"
    fi
    
    echo -e "${GREEN}✅ All packages created${NC}"
}

# Validate package for a specific architecture
validate_package_for_target() {
    local arch_name=$1
    local package_dir="$BUILD_DIR/$arch_name/extensions"
    local package_name="o2-lambda-extension-$arch_name.zip"
    
    echo -e "${YELLOW}🔍 Validating package for $arch_name...${NC}"
    
    # Check if the binary exists and is executable
    if [ ! -x "$package_dir/$EXTENSION_NAME" ]; then
        echo -e "${RED}❌ Error: Extension binary for $arch_name is not executable${NC}"
        return 1
    fi
    
    # Check binary size (should be reasonably small)
    BINARY_SIZE=$(du -h "$package_dir/$EXTENSION_NAME" | cut -f1)
    echo -e "${BLUE}📊 Binary size ($arch_name): $BINARY_SIZE${NC}"
    
    # List package contents
    echo -e "${BLUE}📋 Package contents ($arch_name):${NC}"
    if [ -f "target/$package_name" ]; then
        unzip -l "target/$package_name"
    else
        echo -e "${RED}❌ Package file not found: target/$package_name${NC}"
        return 1
    fi
    
    echo -e "${GREEN}✅ Package validation completed for $arch_name${NC}"
}

# Validate all packages
validate_packages() {
    echo -e "${YELLOW}🔍 Validating packages...${NC}"
    
    if [ "$BUILD_TARGETS" = "all" ]; then
        for arch in "${ARCH_NAMES[@]}"; do
            validate_package_for_target "$arch"
            if [ $? -ne 0 ]; then
                echo -e "${RED}❌ Validation failed for $arch${NC}"
                exit 1
            fi
        done
    else
        # Find the architecture name for the specified target
        arch_name="unknown"
        for i in "${!TARGETS[@]}"; do
            if [ "${TARGETS[$i]}" = "$BUILD_TARGETS" ]; then
                arch_name="${ARCH_NAMES[$i]}"
                break
            fi
        done
        validate_package_for_target "$arch_name"
        if [ $? -ne 0 ]; then
            echo -e "${RED}❌ Validation failed${NC}"
            exit 1
        fi
    fi
    
    echo -e "${GREEN}✅ All package validations completed${NC}"
}

# Main execution flow
main() {
    echo -e "${BLUE}Starting build process...${NC}\n"
    
    check_requirements
    setup_targets
    clean_build
    build_extensions
    create_layer_structures
    create_packages
    validate_packages
    
    echo -e "\n${GREEN}🎉 Build completed successfully!${NC}"
    
    if [ "$BUILD_TARGETS" = "all" ]; then
        echo -e "${BLUE}📦 Packages created for all architectures:${NC}"
        for arch in "${ARCH_NAMES[@]}"; do
            if [ -f "target/o2-lambda-extension-$arch.zip" ]; then
                echo -e "  - target/o2-lambda-extension-$arch.zip (for AWS Lambda $arch)"
            fi
        done
    else
        arch_name="unknown"
        for i in "${!TARGETS[@]}"; do
            if [ "${TARGETS[$i]}" = "$BUILD_TARGETS" ]; then
                arch_name="${ARCH_NAMES[$i]}"
                break
            fi
        done
        echo -e "${BLUE}📦 Package: target/o2-lambda-extension-$arch_name.zip${NC}"
    fi
    
    echo -e "${BLUE}🚀 Ready to deploy as Lambda layers${NC}"
    
    # Show deployment instructions
    echo -e "\n${YELLOW}📚 Deployment Instructions:${NC}"
    if [ "$BUILD_TARGETS" = "all" ]; then
        echo -e "1. Choose the appropriate package for your Lambda architecture:"
        echo -e "   - target/o2-lambda-extension-x86_64.zip for x86_64 Lambda functions"
        echo -e "   - target/o2-lambda-extension-arm64.zip for arm64 Lambda functions"
        echo -e "2. Upload the chosen package to AWS Lambda as a new layer"
    else
        arch_name="unknown"
        for i in "${!TARGETS[@]}"; do
            if [ "${TARGETS[$i]}" = "$BUILD_TARGETS" ]; then
                arch_name="${ARCH_NAMES[$i]}"
                break
            fi
        done
        echo -e "1. Upload target/o2-lambda-extension-$arch_name.zip to AWS Lambda as a new layer"
        echo -e "2. Ensure your Lambda function architecture matches: $arch_name"
    fi
    echo -e "3. Set the following environment variables on your Lambda function:"
    echo -e "   - O2_ORGANIZATION_ID=your_organization_id"
    echo -e "   - O2_AUTHORIZATION_HEADER=\"Basic your_base64_encoded_credentials\""
    echo -e "   - O2_ENDPOINT=https://api.openobserve.ai (optional)"
    echo -e "   - O2_STREAM=default (optional)"
    echo -e "4. Add the layer to your Lambda function"
    echo -e "5. The extension will automatically start capturing and forwarding logs"
    echo -e "\n${BLUE}💡 Architecture Notes:${NC}"
    echo -e "• x86_64: Traditional Intel/AMD 64-bit architecture (most common)"
    echo -e "• arm64: AWS Graviton2/3 processors (better price-performance for many workloads)"
    echo -e "• Layer architecture must match your Lambda function architecture"
}

# Handle script arguments
case "${1:-build}" in
    "build")
        main
        ;;
    "clean")
        echo -e "${YELLOW}🧹 Cleaning build artifacts...${NC}"
        rm -rf $BUILD_DIR
        cargo clean
        echo -e "${GREEN}✅ Clean completed${NC}"
        ;;
    "test")
        echo -e "${YELLOW}🧪 Running tests...${NC}"
        cargo test
        ;;
    "check")
        echo -e "${YELLOW}🔍 Running cargo check...${NC}"
        if [ "$BUILD_TARGETS" = "all" ]; then
            for target in "${TARGETS[@]}"; do
                echo -e "${BLUE}Checking target: $target${NC}"
                cargo check --target $target
            done
        else
            cargo check --target $BUILD_TARGETS
        fi
        ;;
    "help"|"-h"|"--help")
        echo -e "${BLUE}OpenObserve Lambda Extension Build Script${NC}"
        echo ""
        echo "Usage: $0 [command]"
        echo ""
        echo "Commands:"
        echo "  build    Build and package the extension for all architectures (default)"
        echo "  clean    Clean build artifacts"
        echo "  test     Run tests"
        echo "  check    Run cargo check for target(s)"
        echo "  help     Show this help message"
        echo ""
        echo "Default Behavior:"
        echo "  • Builds for BOTH x86_64 AND arm64 architectures by default"
        echo "  • Creates separate packages for each architecture:"
        echo "    - target/o2-lambda-extension-x86_64.zip"
        echo "    - target/o2-lambda-extension-arm64.zip"
        echo ""
        echo "Environment Variables:"
        echo "  BUILD_TARGETS   Override default multi-architecture build:"
        echo "                  'all' - Build for all architectures (default)"
        echo "                  'x86_64-unknown-linux-musl' - Build for x86_64 only"
        echo "                  'aarch64-unknown-linux-musl' - Build for arm64 only"
        echo ""
        echo "Examples:"
        echo "  $0                                           # Build both x86_64 + arm64 (default)"
        echo "  $0 build                                     # Same as above"
        echo "  BUILD_TARGETS=x86_64-unknown-linux-musl $0  # Build x86_64 only"
        echo "  BUILD_TARGETS=aarch64-unknown-linux-musl $0 # Build arm64 only"
        echo ""
        echo "Architecture Guide:"
        echo "  • x86_64: Traditional Intel/AMD processors (most common)"
        echo "  • arm64:  AWS Graviton2/3 processors (better price-performance)"
        ;;
    *)
        echo -e "${RED}❌ Unknown command: $1${NC}"
        echo "Use '$0 help' for usage information"
        exit 1
        ;;
esac