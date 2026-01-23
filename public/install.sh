#!/bin/bash
# public/install.sh

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

# Map to release filename format
case "$OS-$ARCH" in
  linux-x86_64)   FILE="vane-v*-linux-musl-x86_64.tar.gz" ;;
  linux-aarch64)  FILE="vane-v*-linux-musl-aarch64.tar.gz" ;;
  linux-armv7l)   FILE="vane-v*-linux-gnu-armv7.tar.gz" ;;
  darwin-arm64)   FILE="vane-v*-macos-aarch64.tar.gz" ;;
  *)              echo "Unsupported platform: $OS-$ARCH"; exit 1 ;;
esac

# Get latest version and download
VERSION=$(curl -s https://api.github.com/repos/canmi21/vane/releases/latest | grep -Po '"tag_name": "\K[^"]*')
FILENAME=$(echo "$FILE" | sed "s/\*/$VERSION/")

curl -L -o vane.tar.gz "https://github.com/canmi21/vane/releases/download/$VERSION/$FILENAME"
tar -xzf vane.tar.gz

# Install
if [ "$OS" = "darwin" ]; then
  sudo mv vane /usr/local/bin/
  sudo chmod +x /usr/local/bin/vane
  sudo xattr -d com.apple.quarantine /usr/local/bin/vane 2>/dev/null || true
else
  sudo mv vane /usr/local/bin/
  sudo chmod +x /usr/local/bin/vane
fi

rm vane.tar.gz

# Verify installation
vane --version