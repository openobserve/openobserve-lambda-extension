#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
LAYER_NAME_PREFIX="openobserve-extension"
ARCHITECTURES=("x86_64" "arm64")

# Default deployment settings
DEPLOY_ARCH="${DEPLOY_ARCH:-all}"
AWS_REGION="${AWS_REGION:-us-east-1}"
LAYER_DESCRIPTION="OpenObserve lambda layer extension for forwarding logs"

echo -e "${BLUE}🚀 OpenObserve Lambda Layer Deployment Script${NC}"
echo -e "${BLUE}=============================================${NC}"

# Check if AWS CLI is installed and configured
check_aws_cli() {
    echo -e "${YELLOW}🔍 Checking AWS CLI...${NC}"
    
    if ! command -v aws &> /dev/null; then
        echo -e "${RED}❌ Error: AWS CLI is not installed${NC}"
        echo -e "${YELLOW}Please install AWS CLI: https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html${NC}"
        exit 1
    fi
    
    # Check if AWS credentials are configured
    if ! aws sts get-caller-identity &> /dev/null; then
        echo -e "${RED}❌ Error: AWS credentials not configured${NC}"
        echo -e "${YELLOW}Please configure AWS CLI: aws configure${NC}"
        exit 1
    fi
    
    ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
    echo -e "${GREEN}✅ AWS CLI configured (Account: $ACCOUNT_ID, Region: $AWS_REGION)${NC}"
}

# Check if deployment packages exist
check_packages() {
    echo -e "${YELLOW}📦 Checking deployment packages...${NC}"
    
    if [ "$DEPLOY_ARCH" = "all" ]; then
        for arch in "${ARCHITECTURES[@]}"; do
            package_file="target/o2-lambda-extension-$arch.zip"
            if [ ! -f "$package_file" ]; then
                echo -e "${RED}❌ Error: Package not found: $package_file${NC}"
                echo -e "${YELLOW}Run './build.sh' to build the packages first${NC}"
                exit 1
            fi
            
            package_size=$(du -h "$package_file" | cut -f1)
            echo -e "${GREEN}✅ Found: $package_file ($package_size)${NC}"
        done
    else
        package_file="target/o2-lambda-extension-$DEPLOY_ARCH.zip"
        if [ ! -f "$package_file" ]; then
            echo -e "${RED}❌ Error: Package not found: $package_file${NC}"
            echo -e "${YELLOW}Run 'BUILD_TARGETS=<target> ./build.sh' to build the package first${NC}"
            exit 1
        fi
        
        package_size=$(du -h "$package_file" | cut -f1)
        echo -e "${GREEN}✅ Found: $package_file ($package_size)${NC}"
    fi
}

# Deploy layer for a specific architecture
deploy_layer_for_arch() {
    local arch=$1
    local layer_name="$LAYER_NAME_PREFIX-$arch"
    local package_file="target/o2-lambda-extension-$arch.zip"
    local description="$LAYER_DESCRIPTION ($arch)"
    
    echo -e "${YELLOW}🚀 Deploying layer for $arch architecture...${NC}"
    
    # Compatible runtimes for Lambda
    local compatible_runtimes=(
        "python3.9" "python3.10" "python3.11" "python3.12" "python3.13"
        "nodejs18.x" "nodejs20.x" "nodejs22.x"
        "java11" "java17" "java21"
        "dotnet8" "dotnet6"
        "go1.x"
        "ruby3.2" "ruby3.3"
        "provided.al2" "provided.al2023"
    )
    
    # Build the AWS CLI command
    local aws_command="aws lambda publish-layer-version"
    aws_command="$aws_command --layer-name $layer_name"
    aws_command="$aws_command --zip-file fileb://$package_file"
    aws_command="$aws_command --compatible-architectures $arch"
    aws_command="$aws_command --description \"$description\""
    aws_command="$aws_command --region $AWS_REGION"
    
    # Add compatible runtimes
    for runtime in "${compatible_runtimes[@]}"; do
        aws_command="$aws_command --compatible-runtimes $runtime"
    done
    
    echo -e "${BLUE}📡 Executing: $aws_command${NC}"
    
    # Execute the deployment
    local result
    result=$(eval "$aws_command" 2>&1)
    local exit_code=$?
    
    if [ $exit_code -eq 0 ]; then
        # Parse the result to get layer ARN and version
        local layer_arn=$(echo "$result" | grep -o '"LayerArn": "[^"]*"' | cut -d'"' -f4)
        local version=$(echo "$result" | grep -o '"Version": [0-9]*' | cut -d' ' -f2)
        
        echo -e "${GREEN}✅ Layer deployed successfully!${NC}"
        echo -e "${BLUE}   Layer Name: $layer_name${NC}"
        echo -e "${BLUE}   Version: $version${NC}"
        echo -e "${BLUE}   ARN: $layer_arn${NC}"
        echo -e "${BLUE}   Region: $AWS_REGION${NC}"
        echo -e "${BLUE}   Architecture: $arch${NC}"
        echo ""
        
        # Save deployment info
        echo "$layer_arn:$version" >> "deployment-info-$arch.txt"
        
        return 0
    else
        echo -e "${RED}❌ Deployment failed for $arch:${NC}"
        echo -e "${RED}$result${NC}"
        return 1
    fi
}

# Deploy layers for all specified architectures
deploy_layers() {
    echo -e "${YELLOW}🚀 Starting layer deployment...${NC}"
    
    local success_count=0
    local total_count=0
    local deployed_layers=()
    
    if [ "$DEPLOY_ARCH" = "all" ]; then
        total_count=${#ARCHITECTURES[@]}
        for arch in "${ARCHITECTURES[@]}"; do
            if deploy_layer_for_arch "$arch"; then
                success_count=$((success_count + 1))
                deployed_layers+=("$arch")
            fi
        done
    else
        total_count=1
        if deploy_layer_for_arch "$DEPLOY_ARCH"; then
            success_count=1
            deployed_layers+=("$DEPLOY_ARCH")
        fi
    fi
    
    # Summary
    echo -e "${BLUE}📊 Deployment Summary:${NC}"
    echo -e "${GREEN}✅ Successfully deployed: $success_count/$total_count layers${NC}"
    
    if [ ${#deployed_layers[@]} -gt 0 ]; then
        echo -e "${BLUE}🎯 Deployed layers:${NC}"
        for arch in "${deployed_layers[@]}"; do
            echo -e "   • $LAYER_NAME_PREFIX-$arch (architecture: $arch)"
        done
        
        echo -e "\n${YELLOW}📚 Usage Instructions:${NC}"
        echo -e "1. In your Lambda function configuration:"
        echo -e "   • Go to 'Layers' section"
        echo -e "   • Click 'Add a layer'"
        echo -e "   • Select 'Custom layers'"
        echo -e "   • Choose the layer matching your function's architecture"
        echo -e ""
        echo -e "2. Set environment variables in your Lambda function:"
        echo -e "   • O2_ORGANIZATION_ID=your_organization_id"
        echo -e "   • O2_AUTHORIZATION_HEADER=\"Basic your_base64_encoded_credentials\""
        echo -e "   • O2_ENDPOINT=https://api.openobserve.ai (optional)"
        echo -e "   • O2_STREAM=default (optional)"
        echo -e ""
        echo -e "3. The extension will automatically start capturing and forwarding logs"
    fi
    
    if [ $success_count -lt $total_count ]; then
        echo -e "${RED}⚠️  Some deployments failed. Check the error messages above.${NC}"
        exit 1
    fi
}

# List existing layers
list_layers() {
    echo -e "${YELLOW}📋 Listing existing OpenObserve layers...${NC}"
    
    for arch in "${ARCHITECTURES[@]}"; do
        local layer_name="$LAYER_NAME_PREFIX-$arch"
        echo -e "${BLUE}Checking layer: $layer_name${NC}"
        
        local result
        result=$(aws lambda list-layer-versions --layer-name "$layer_name" --region "$AWS_REGION" 2>/dev/null || echo "No layers found")
        
        if [[ "$result" == "No layers found" ]] || [[ "$result" == *"ResourceNotFoundException"* ]]; then
            echo -e "${YELLOW}   No versions found for $layer_name${NC}"
        else
            local versions=$(echo "$result" | grep -o '"Version": [0-9]*' | cut -d' ' -f2 | head -5)
            echo -e "${GREEN}   Latest versions: $(echo $versions | tr '\n' ' ')${NC}"
        fi
        echo ""
    done
}

# Delete layers
delete_layers() {
    echo -e "${YELLOW}🗑️  Deleting OpenObserve layers...${NC}"
    echo -e "${RED}⚠️  This will delete ALL versions of the layers!${NC}"
    read -p "Are you sure? (y/N): " -n 1 -r
    echo
    
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${YELLOW}Deletion cancelled.${NC}"
        exit 0
    fi
    
    for arch in "${ARCHITECTURES[@]}"; do
        local layer_name="$LAYER_NAME_PREFIX-$arch"
        echo -e "${YELLOW}Deleting layer: $layer_name${NC}"
        
        # Get all versions
        local versions
        versions=$(aws lambda list-layer-versions --layer-name "$layer_name" --region "$AWS_REGION" 2>/dev/null | grep -o '"Version": [0-9]*' | cut -d' ' -f2 || true)
        
        if [ -z "$versions" ]; then
            echo -e "${YELLOW}   No versions found for $layer_name${NC}"
            continue
        fi
        
        # Delete each version
        for version in $versions; do
            echo -e "${YELLOW}   Deleting version $version...${NC}"
            aws lambda delete-layer-version --layer-name "$layer_name" --version-number "$version" --region "$AWS_REGION" > /dev/null
            echo -e "${GREEN}   ✅ Deleted version $version${NC}"
        done
    done
    
    echo -e "${GREEN}✅ All layers deleted successfully${NC}"
}

# Show help
show_help() {
    echo -e "${BLUE}OpenObserve Lambda Layer Deployment Script${NC}"
    echo ""
    echo "Usage: $0 [command] [options]"
    echo ""
    echo "Commands:"
    echo "  deploy     Deploy layer(s) to AWS Lambda (default)"
    echo "  list       List existing layer versions"
    echo "  delete     Delete all layer versions"
    echo "  help       Show this help message"
    echo ""
    echo "Environment Variables:"
    echo "  DEPLOY_ARCH      Architecture to deploy:"
    echo "                   'all' - Deploy both architectures (default)"
    echo "                   'x86_64' - Deploy x86_64 only"
    echo "                   'arm64' - Deploy arm64 only"
    echo ""
    echo "  AWS_REGION       AWS region (default: us-east-1)"
    echo ""
    echo "Examples:"
    echo "  $0                                    # Deploy both architectures"
    echo "  DEPLOY_ARCH=x86_64 $0                # Deploy x86_64 only"
    echo "  DEPLOY_ARCH=arm64 AWS_REGION=eu-west-1 $0  # Deploy arm64 to EU"
    echo "  $0 list                               # List existing layers"
    echo "  $0 delete                             # Delete all layers"
}

# Main execution flow
main() {
    check_aws_cli
    check_packages
    deploy_layers
    
    echo -e "\n${GREEN}🎉 Deployment completed successfully!${NC}"
    echo -e "${BLUE}Region: $AWS_REGION${NC}"
    echo -e "${BLUE}Account: $ACCOUNT_ID${NC}"
}

# Handle script arguments
case "${1:-deploy}" in
    "deploy")
        main
        ;;
    "list")
        check_aws_cli
        list_layers
        ;;
    "delete")
        check_aws_cli
        delete_layers
        ;;
    "help"|"-h"|"--help")
        show_help
        ;;
    *)
        echo -e "${RED}❌ Unknown command: $1${NC}"
        echo "Use '$0 help' for usage information"
        exit 1
        ;;
esac