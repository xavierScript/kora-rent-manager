# ==============================================================================
#  KORA RENT MANAGER PROJECT MAKEFILE
# ==============================================================================

# --------------------------
# 1. Global Variables
# --------------------------
RPC      = https://api.devnet.solana.com
CONFIG   = kora.toml
SIGNERS  = signers.toml
INTERVAL ?= 60s  # Default daemon interval (can be overridden: make run INTERVAL=5m)

# --------------------------
# 2. Standard Targets
# --------------------------
.PHONY: install
.PHONY: setup scan run stats reclaim force-reclaim

# Install the CLI tool locally (overwriting if it exists)
install:
	cargo install --path crates/cli --force

# --------------------------
# 3. Rent Manager Commands
# --------------------------

# Setup: Create a "Zombie" (empty) account to test the bot logic
setup:
	RPC_URL=$(RPC) cargo run --bin zombie_account_setup

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

