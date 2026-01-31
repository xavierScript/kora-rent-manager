# ==============================================================================
#  KORA RENT MANAGER PROJECT MAKEFILE
# ==============================================================================

# --------------------------
# 1. Global Variables
# --------------------------
RPC      = https://api.devnet.solana.com
CONFIG   = kora.toml
SIGNERS  = signers.toml
INTERVAL ?= 60s  # Default daemon interval (override: make run INTERVAL=5m)

# --------------------------
# 2. Standard Targets
# --------------------------
.PHONY: default install setup scan run stats reclaim force-reclaim welcome help

# Default target: Shows the welcome menu
default: welcome

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
# Note: This runs in Safe Mode (Alerts only, no execution)
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
# 4. Welcome & Help
# --------------------------

# Short alias: "make help" runs "make welcome"
help: welcome

welcome:
	@echo ""
	@echo "  🛡️   \033[1;36mKORA RENT MANAGER\033[0m - Automated Treasury Recovery"
	@echo "  =================================================================="
	@echo "  Recover rent-locked SOL from inactive sponsored accounts."
	@echo ""
	@echo "  \033[1;33mUsage:\033[0m make [command]"
	@echo ""
	@echo "  \033[1;32mCore Commands:\033[0m"
	@echo "    \033[1mscan\033[0m          🔍  Audit accounts (Read-Only). View 'Pending' vs 'Funded'."
	@echo "    \033[1mreclaim\033[0m       💰  Execute cleanup. Closes accounts older than 24h (or test duration)."
	@echo "    \033[1mrun\033[0m           🤖  Start Daemon. Continuous monitoring & Telegram alerts."
	@echo "    \033[1mstats\033[0m         📊  Show quick text-based metrics (Non-TUI)."
	@echo ""
	@echo "  \033[1;32mTesting & Setup:\033[0m"
	@echo "    \033[1msetup\033[0m         🧟  Create a 'Zombie' empty account on Devnet to test the bot."
	@echo "    \033[1minstall\033[0m       🚀  Compile & install the 'kora' binary globally."
	@echo ""
	@echo "  \033[1;32mConfiguration:\033[0m"
	@echo "    Edit \033[1mkora.toml\033[0m to whitelist tokens."
	@echo "    Set \033[1mKORA_TG_TOKEN\033[0m in .env for mobile alerts."
	@echo ""
	@echo "  =================================================================="
	@echo ""