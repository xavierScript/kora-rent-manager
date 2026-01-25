# Include all makefile modules
include makefiles/UTILS.makefile
include makefiles/BUILD.makefile
include makefiles/RUST_TESTS.makefile
include makefiles/TYPESCRIPT_TESTS.makefile
include makefiles/CLIENT.makefile
include makefiles/DOCUMENTATION.makefile
include makefiles/COVERAGE.makefile
include makefiles/METRICS.makefile
include makefiles/RELEASE.makefile

.PHONY: check lint test build run clean all install generate-key setup-test-env test-integration test-all test-ts coverage coverage-clean build-bin build-lib build-cli run-presigned openapi gen-ts-client run-metrics build-transfer-hook release

# Default target
all: check test build

# Run all tests (unit + TypeScript + integration)
test-all: build test test-ts test-integration


################################################## Rent Manager Makefile ##################################################

# Variables
RPC = https://api.devnet.solana.com
CONFIG = kora.toml
SIGNERS = signers.toml

# Default to 60s if the user doesn't specify one
INTERVAL ?= 60s

# Commands
.PHONY: scan run stats reclaim

# Short command: "make scan"
scan:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager scan --all --signers-config $(SIGNERS)

# Dynamic Run Command. Short command: "make run INTERVAL=5m", "make run INTERVAL=5h", etc
run:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager run --interval $(INTERVAL) --signers-config $(SIGNERS)

# Short command: "make stats"
stats:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager stats --signers-config $(SIGNERS)

#RECLAIM: Safely close eligible accounts (Executes transactions)
# Note: This respects your 24h Grace Period and Whitelist
# Short command: "make reclaim"
reclaim:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager reclaim --execute --signers-config $(SIGNERS)

#FORCE RECLAIM: Close everything immediately (Ignores Grace Period)
# Use with caution!
# Short command: "make force-reclaim"
force-reclaim:
	kora --rpc-url $(RPC) --config $(CONFIG) rent-manager reclaim --execute --force-all --signers-config $(SIGNERS)