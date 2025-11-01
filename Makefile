.PHONY: build release test clean install run help build-rpi deploy-rpi install-rpi

# Configuration
RPI_HOST ?= pi@snow
RPI_TARGET = armv7-unknown-linux-gnueabihf

# Default target
all: build

# Build debug version
build:
	cargo build

# Build optimized release version
release:
	cargo build --release

# Run tests
test:
	cargo test

# Run clippy linter
lint:
	cargo clippy -- -D warnings

# Format code
fmt:
	cargo fmt

# Clean build artifacts
clean:
	cargo clean

# Install to /usr/local/bin (requires sudo)
install: release
	install -m 755 target/release/snowgauge /usr/local/bin/

# Run simulator mode
run:
	cargo run -- --simulator

# Run with debug logging
run-debug:
	cargo run -- --simulator --debug --log

# Build for Raspberry Pi (ARMv7) - requires rustup target and linker
build-rpi:
	@echo "Building for Raspberry Pi (armv7-unknown-linux-gnueabihf)..."
	@echo "Note: First time setup requires:"
	@echo "  rustup target add armv7-unknown-linux-gnueabihf"
	@echo "  brew tap messense/macos-cross-toolchains"
	@echo "  brew install armv7-unknown-linux-gnueabihf"
	cargo build --release --target $(RPI_TARGET)
	@echo "Binary location: target/$(RPI_TARGET)/release/snowgauge"
	@file target/$(RPI_TARGET)/release/snowgauge

# Build on Raspberry Pi directly (no cross-compilation needed)
build-rpi-remote:
	@echo "Building on Raspberry Pi..."
	@echo "Copying source to $(RPI_HOST):/tmp/snowgauge-build/..."
	ssh $(RPI_HOST) 'mkdir -p /tmp/snowgauge-build'
	rsync -av --exclude target --exclude .git . $(RPI_HOST):/tmp/snowgauge-build/
	@echo "Building on Pi (this may take 5-15 minutes)..."
	ssh $(RPI_HOST) 'cd /tmp/snowgauge-build && cargo build --release'
	@echo "Copying binary back..."
	scp $(RPI_HOST):/tmp/snowgauge-build/target/release/snowgauge target/snowgauge-armv7
	@echo "Binary location: target/snowgauge-armv7"
	@file target/snowgauge-armv7

# Deploy to Raspberry Pi (using locally cross-compiled binary)
deploy-rpi: build-rpi
	@echo "Copying binary to $(RPI_HOST):/home/pi/..."
	scp target/$(RPI_TARGET)/release/snowgauge $(RPI_HOST):/home/pi/
	ssh $(RPI_HOST) 'chmod +x /home/pi/snowgauge'
	@echo "Deployed! Test with: ssh $(RPI_HOST) './snowgauge --help'"

# Deploy to Raspberry Pi (using remote-built binary)
deploy-rpi-remote: build-rpi-remote
	@echo "Copying binary to $(RPI_HOST):/home/pi/..."
	scp target/snowgauge-armv7 $(RPI_HOST):/home/pi/snowgauge
	ssh $(RPI_HOST) 'chmod +x /home/pi/snowgauge'
	@echo "Deployed! Test with: ssh $(RPI_HOST) './snowgauge --help'"

# Install on Raspberry Pi system-wide (from cross-compiled build)
install-rpi: deploy-rpi
	@echo "Installing to $(RPI_HOST):/usr/local/bin/..."
	ssh $(RPI_HOST) 'sudo mv /home/pi/snowgauge /usr/local/bin/ && sudo chmod +x /usr/local/bin/snowgauge'
	@echo "Installed! Test with: ssh $(RPI_HOST) 'snowgauge --help'"

# Install on Raspberry Pi system-wide (from remote build)
install-rpi-remote: deploy-rpi-remote
	@echo "Installing to $(RPI_HOST):/usr/local/bin/..."
	ssh $(RPI_HOST) 'sudo mv /home/pi/snowgauge /usr/local/bin/ && sudo chmod +x /usr/local/bin/snowgauge'
	@echo "Installed! Test with: ssh $(RPI_HOST) 'snowgauge --help'"

# Test on Raspberry Pi
test-rpi: deploy-rpi-remote
	@echo "Testing on Raspberry Pi..."
	ssh $(RPI_HOST) '/home/pi/snowgauge --help'

# Show help
help:
	@echo "Available targets:"
	@echo ""
	@echo "Local builds:"
	@echo "  make build       - Build debug version"
	@echo "  make release     - Build optimized release version"
	@echo "  make test        - Run tests"
	@echo "  make lint        - Run clippy linter"
	@echo "  make fmt         - Format code"
	@echo "  make clean       - Remove build artifacts"
	@echo "  make install     - Install binary to /usr/local/bin"
	@echo "  make run         - Run simulator mode"
	@echo "  make run-debug   - Run with debug logging"
	@echo ""
	@echo "Raspberry Pi builds:"
	@echo "  make build-rpi-remote   - Build on Raspberry Pi (recommended for macOS)"
	@echo "  make deploy-rpi-remote  - Build on Pi and copy binary"
	@echo "  make install-rpi-remote - Build on Pi and install system-wide"
	@echo "  make test-rpi           - Deploy and test on Raspberry Pi"
	@echo ""
	@echo "  make build-rpi          - Cross-compile locally (requires 'cross' tool)"
	@echo "  make deploy-rpi         - Deploy cross-compiled binary"
	@echo "  make install-rpi        - Install cross-compiled binary"
	@echo ""
	@echo "Configuration:"
	@echo "  RPI_HOST=$(RPI_HOST)  - Override with 'make deploy-rpi RPI_HOST=user@hostname'"
