# ==============================================================================
#  KORA PROJECT MAKEFILE
# ==============================================================================

# --------------------------
# 1. Modules & Includes
# --------------------------
include makefiles/UTILS.makefile
include makefiles/BUILD.makefile
include makefiles/RUST_TESTS.makefile
include makefiles/TYPESCRIPT_TESTS.makefile
include makefiles/CLIENT.makefile
include makefiles/DOCUMENTATION.makefile
include makefiles/COVERAGE.makefile
include makefiles/METRICS.makefile
include makefiles/RELEASE.makefile

# --------------------------
# 2. Global Variables
# --------------------------
RPC      = https://api.devnet.solana.com
CONFIG   = kora.toml
SIGNERS  = signers.toml
INTERVAL ?= 60s  # Default daemon interval (can be overridden: make run INTERVAL=5m)

# --------------------------
# 3. Standard Targets
# --------------------------
.PHONY: all check lint test build clean install
.PHONY: setup scan run stats reclaim force-reclaim
.PHONY: test-all test-ts test-integration coverage coverage-clean
.PHONY: build-bin build-lib build-cli release
.PHONY: generate-key setup-test-env run-presigned openapi gen-ts-client run-metrics build-transfer-hook

# Default target: Validates code, runs tests, and builds
all: check test build

# Install the CLI tool locally (overwriting if it exists)
install:
	cargo install --path crates/cli --force

# --------------------------
# 4. Rent Manager Commands
# --------------------------

# Setup: Create a "Zombie" (empty) account to test the bot logic
setup:
	RPC_URL=$(RPC) cargo run --bin setup_reclaim

# Scan: View accounts without taking action (Read-Only)
# Displays pending/funded accounts in TUI
scan:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager scan --all --signers-config $(SIGNERS)

# Run Daemon: Continuous monitoring background service
# Usage: "make run" or "make run INTERVAL=1h"
run:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager run --interval $(INTERVAL) --signers-config $(SIGNERS)

# Reclaim: Safely close eligible accounts (Executes transactions)
# Respects the 24h Grace Period and Whitelist
reclaim:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager reclaim --execute --signers-config $(SIGNERS)

# Force Reclaim: Close everything immediately (Ignores Grace Period)
# ⚠️ Use with caution!
force-reclaim:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager reclaim --execute --force-all --signers-config $(SIGNERS)

# Stats: Display quick text-based metrics (Non-TUI)
stats:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager stats --signers-config $(SIGNERS)

# --------------------------
# 5. Development & Testing
# --------------------------

# Run all test suites (Unit + TypeScript + Integration)
test-all: build test test-ts test-integration