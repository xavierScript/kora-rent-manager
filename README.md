# 🛡️ Kora Rent Manager

[![Rust](https://img.shields.io/badge/Built_with-Rust-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Solana](https://img.shields.io/badge/Solana-Devnet-green?style=flat-square&logo=solana)](https://solana.com/)
[![License](https://img.shields.io/badge/License-MIT-blue?style=flat-square)](LICENSE)

> **Automated Treasury Recovery for Kora Node Operators.**
> _Monitor, detect, and reclaim idle rent-locked SOL with safety and clarity._

---

## 📺 Demo & Deep Dive

**▶️ WATCH THE WALKTHROUGH VIDEO HERE**
_https://www.loom.com/share/021e74ce74b14293808946eb0a58e326_

---

## 🖼️ Screenshots

Quick visual tour — images are available in the `screenshots/` folder.

<p align="center">
  <img src="screenshots/tui-dashboard.png" alt="TUI Dashboard" />
</p>

<p align="center"><em>TUI dashboard showing cycle efficiency and live logs.</em></p>

<p align="center">
  <img src="screenshots/scan.png" alt="Scan Results" />
</p>

<p align="center"><em>Scan results with `Pending` and `Reclaimable` statuses.</em></p>

<p align="center">
  <img src="screenshots/stats.png" alt="Stats & Reclaims" />
</p>

<p align="center"><em>Cycle stats and reclaimed SOL summary.</em></p>

<p align="center">
  <img src="screenshots/telegram-alerts.png" alt="Telegram Alerts" />
</p>

<p align="center"><em>Heartbeat and notification examples (Telegram).</em></p>

<p align="center">
  <img src="screenshots/setup.png" alt="Demo Setup" />
</p>

<p align="center"><em>Test/devnet setup for creating zombie accounts.</em></p>

<p align="center">
  <img src="screenshots/run.png" alt="Daemon Running" />
</p>

<p align="center"><em>Background daemon running in the terminal.</em></p>

<p align="center">
  <img src="screenshots/audit-logs.png" alt="Audit Log" />
</p>

<p align="center"><em>CSV audit log view of reclaimed accounts.</em></p>

---

---

## 🗂️ Project File Structure

Below is a high-level overview of the main file and directory structure for this repository:

```
audit_log.csv
grace_period.json
LICENSE.md
Makefile
README.md

audits/
crates/
  cli/
    Cargo.toml
    src/
      args.rs
      main.rs
      rent_manager.rs
      bin/
  lib/
      ...
examples/
makefiles/
sdks/
target/
tests/
```

---

## 🚨 The Problem: Silent Capital Loss

Kora makes onboarding users to Solana seamless by sponsoring account creation fees. However, this convenience creates an operational gap: **Rent-Locked SOL**.

When a Kora node creates a Token Account for a user, it deposits ~0.002 SOL (Rent Exempt Minimum).

- If 1,000 users churn or empty their wallets, **2 SOL** remains locked on-chain.
- If 100,000 users churn, **200 SOL** is lost to "zombie" accounts.

Operators rarely have the time to manually audit thousands of accounts, check balances, and sign close transactions.

---

## ⚙️ How Kora Works & The "Rent Trap"

To understand why this tool is necessary, it helps to understand the architecture of Kora and the Solana Storage Model.

### 1. Kora: The Fee Abstraction Layer

Kora acts as a Paymaster and Relayer. It sits between your application and the Solana network to provide "Gasless" transactions.

- The User signs a transaction to move tokens (USDC, BONK, etc.).
- The Kora Node validates the transaction and acts as the Fee Payer, covering the SOL network costs.
- The Result: Users interact with Solana without ever holding SOL.

### 2. The Solana Rent Model

On Solana, everything is an account, and every account takes up space on the validator's disk. To prevent spam, Solana charges Rent (approx. `0.002039 SOL` per Token Account).

- This SOL is deposited when an account is created.
- It is locked inside the account as long as the account exists.
- It is fully refundable if the account is closed.

### 3. Where the Lock Happens (The Leak)

In high-throughput Kora deployments—especially those involving custodial wallets, intermediate buffering, or rapid user onboarding—the Kora Operator often acts as the owner of the Token Accounts to facilitate transfers.

1. **Creation:** Kora creates a Token Account to receive or buffer user funds. The Operator pays the `~0.002 SOL` rent deposit.
2. **Usage:** The user interacts with the app, eventually withdrawing or spending their tokens.
3. **Abandonment:** The Token Account balance hits `0`. However, the account remains open on-chain.
4. **The Lock:** The `0.002 SOL` rent deposit remains locked in this empty "Zombie Account."

While 0.002 SOL seems trivial, a Kora node servicing 100,000 operations can easily end up with 200+ SOL ($30,000+) locked in inactive accounts. **Kora Rent Manager** automates the recovery of this dormant capital.

---

## 🛠️ The Solution: Kora Rent Manager

This tool is a **set-and-forget** CLI utility and background service designed to close this operational gap. It provides a visual dashboard to monitor rent status and an automated daemon to reclaim funds safely.

### Key Features

- **📊 TUI Dashboard:** Real-time visualization of cycle efficiency, reclaimed funds, and active tasks using `Ratatui`.
- **🛡️ Safety Grace Period:** Built-in tracking ensures newly detected empty accounts are **never** closed immediately. They must remain empty for **24 hours** before being flagged as reclaimable.
- **📲 Telegram Alerts:** Passive monitoring. Get notified on your phone if idle rent exceeds a threshold (e.g., 5 SOL) or if a reclaim cycle succeeds.
- **💓 Heartbeat Reporting:** The daemon sends periodic "System Alive" snapshots to Telegram, ensuring operators know the bot is active without checking the terminal.
- **📜 Audit Trail:** Every action is logged to `audit_log.csv` for financial reconciliation.
- **⚙️ Configurable:** Customize scan intervals, thresholds, and whitelists via environment variables and CLI args.

---

## 🧠 Technical Context: How It Works

### Solana Rent Mechanics

On Solana, every account must hold a minimum amount of SOL (approx. 0.002039 SOL) to remain "rent-exempt." If an account has 0 tokens but still holds this SOL, it is essentially wasting space and money.

### The Reclaim Logic

The bot performs the following cycle:

1. **Scan:** It queries the RPC for all Token Accounts owned by the configured Signer.
2. **Filter:** It identifies accounts with `amount: 0` (Empty).
3. **Safety Check (The Tracker):**
   - _Is this account whitelisted?_ (Skip)
   - _Is this the first time we've seen it empty?_ (Mark as "Pending", start 24h timer).
   - _Has it been empty for >24 hours?_ (Mark as "Reclaimable").
4. **Execution:** If enabled, it constructs a `closeAccount` instruction, signs it with the operator's keypair, and sends it to the network.
5. **Alerting:** If the total rent reclaimed > 0 or total locked rent > Threshold, it fires a notification.

---

## 🚀 Getting Started

### System Requirements

#### For Kora Rent Manager (Server)

- **Rust:** Version 1.86 or higher
- **Make:** Command-line build utility
- **Kora CLI:** For operating a Kora node

#### For TypeScript SDK (Client Applications)

- **Node.js:** Version LTS or higher
- **TypeScript:** Latest version

#### Optional Dependencies

- **Solana CLI:** Helpful for key generation and testing
- **Docker:** For containerized deployments

---

### Prerequisites Installation

#### 1. Install Rust

If you don't have Rust installed, install it using `rustup`:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env
```

Verify installation:

```bash
rustc --version  # Should show 1.86 or higher
cargo --version
```

#### 2. Install Make

**macOS:**

```bash
xcode-select --install
```

**Linux (Ubuntu/Debian):**

```bash
sudo apt-get update
sudo apt-get install build-essential
```

**Linux (Fedora/RHEL):**

```bash
sudo dnf install make
```

**Windows:**

- Install via [Chocolatey](https://chocolatey.org/): `choco install make`
- Or use [WSL](https://docs.microsoft.com/en-us/windows/wsl/install) for a Linux environment

Verify installation:

```bash
make --version
```

#### 3. Install Kora CLI

The Kora CLI is required for operating a Kora node. Install directly from crates.io:

```bash
cargo install kora-cli
```

Verify installation:

```bash
kora --version
```

#### 4. Install Solana CLI (Optional but Recommended)

The Solana CLI is useful for key generation and testing:

```bash
sh -c "$(curl -sSfL https://release.solana.com/stable/install)"
```

Add Solana to your PATH (add to `~/.bashrc` or `~/.zshrc`):

```bash
export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"
```

Verify installation:

```bash
solana --version
```

---

### Installation and Setup

#### Clone the Repository

```bash
git clone https://github.com/xavierScript/kora-rent-manager.git
cd kora-rent-manager
```

#### Build the Project

```bash
make install
```

The compiled binary will be available at `./target/release/kora-rent-manager`.

#### 🧟‍♂️ Set Up Zombie Accounts (Devnet Testing)

```bash
make setup
```

This command creates a **test (zombie) account on Solana devnet** whose rent is funded by your wallet.  
It is intended strictly for development and testing purposes.

##### Requirements

- Your wallet must have sufficient **devnet SOL**
- Obtain devnet SOL from the Solana faucet before running the command

---

### Configuration

#### 1. Create Environment File

Create a `.env` file in the project root:

```bash
touch .env
```

#### 2. Set Environment Variables

Add the following to your `.env` file:

```dotenv
# ========================================
# KORA NODE OPERATOR CONFIGURATION
# ========================================

# Private Key Signer (Your Kora Operator Keypair)
KORA_PRIVATE_KEY=your_base58_private_key_here

# RPC Endpoint (Optional - defaults to devnet)
# SOLANA_RPC_URL=https://api.devnet.solana.com

# ========================================
# TELEGRAM ALERTS (OPTIONAL)
# ========================================

# Get your bot token from @BotFather on Telegram
KORA_TG_TOKEN=123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11

# Your Telegram Chat ID (get from @userinfobot)
KORA_TG_CHAT_ID=987654321
```

#### 3. Secure Your Keys

**Important Security Notes:**

- Never commit `.env` or keypair files to version control
- Add `.env` and `*.json` to your `.gitignore`

---

### Verification

Before running the bot, verify your setup:

```bash
# Check if all dependencies are installed
rustc --version
cargo --version
make --version
kora --version
```

---

## 🎮 Usage Guide

We provide a Makefile for easy operation.

### 1. 🔍 Scan (Read-Only)

View the state of your accounts without sending transactions. This populates the Dashboard with "Pending", "Reclaimable" or "Funded" statuses.

```bash
make scan
```

This command is **safe** and does not modify any accounts. Use it to:

- Audit your current rent allocation
- Identify accounts ready for reclamation
- Test your configuration before enabling reclamation

### 2. ⚡ Reclaim (Action)

Execute the cleanup. This will only close accounts that have passed the 24h Grace Period.

```bash
make reclaim
```

**⚠️ Warning:** This command will send transactions to the Solana network and close accounts. Ensure you've reviewed the scan results first.

### 3. 🤖 Run Daemon (Background Service)

Run the bot continuously. It will sleep for the specified interval (default 10s) and wake up to process accounts.

```bash
# Run with default 10s interval
make run

# Run with custom interval (e.g., 1 hour)
make run INTERVAL=1h

# Run with 5-minute interval
make run INTERVAL=5m
```

### 4. 📜 Stats (Action)

Check for Stats relating to the Node Operator (Your wallet address, in this case)

```bash
make stats
```

**Recommended for Production:**

- Start with a longer interval (e.g., `1h`) for safety
- Monitor the first few cycles manually
- Gradually decrease interval as you gain confidence

### 4. 📋 View Logs

Check the audit log for all reclamation activity:

```bash
cat audit_log.csv
```

Or use tools like `csvkit` for better formatting:

```bash
csvlook audit_log.csv
```

---

## 📊 Dashboard & Monitoring

### The TUI (Terminal User Interface)

When running, the bot displays a rich terminal interface:

**Performance Metrics:** Real-time counter of SOL reclaimed.

**Cycle Efficiency:** A gauge showing how "optimized" your treasury is.

**Live Logs:** Detailed color-coded logs of every account checked.

- **YELLOW:** Account is empty but inside Grace Period.
- **GREEN:** Account successfully closed & rent recovered.
- **GREY:** Account is funded (Skipped).
- **RED:** High Rent Alert or Error.

### The Audit Log

Check `audit_log.csv` for a permanent record:

```csv
timestamp,date_utc,account,mint,action,reason,rent_reclaimed_sol,signature
1706131200,2024-01-25T00:00:00Z,4xp...JQc,DD6...f62,RECLAIMED,InactiveGracePeriodPassed,0.0020,5Mz...123
```

### Telegram Notifications

If configured, you'll receive alerts for:

- **High Rent Alert:** When total locked rent exceeds threshold
- **Reclamation Success:** Summary of each successful cycle
- **Heartbeat:** Periodic status updates confirming the daemon is alive

---

## 🔧 Advanced Configuration

### Custom RPC Endpoint

For production, use a dedicated RPC provider:

```bash
export SOLANA_RPC_URL=https://your-rpc-provider.com
```

Recommended providers:

- [Helius](https://helius.dev/)
- [QuickNode](https://www.quicknode.com/)
- [Triton](https://triton.one/)

### Whitelist Accounts

To prevent specific accounts from being closed, add them to `whitelist.json`:

```json
{
  "accounts": [
    "AccountAddress1111111111111111111111111",
    "AccountAddress2222222222222222222222222"
  ]
}
```

### Adjust Grace Period

Modify the grace period in your `.env`:

```dotenv
KORA_GRACE_PERIOD_HOURS=48  # Wait 48 hours instead of 24
```

---

## 🏆 Submission Checklist

- [x] Monitors Accounts: Scans all token accounts for specific signers.
- [x] Detects Inactive: Filters for 0 balance & tracks inactivity duration.
- [x] Reclaims Rent: Uses `spl_token::instruction::close_account`.
- [x] Open Source: MIT License.
- [x] Safety: 24-hour Grace Period Tracker (`grace_period.json`).
- [x] Clarity: TUI Dashboard + CSV Audit Trail.
- [x] Alerts: Visual Dashboard Alerts + Telegram Push Notifications + Heartbeat Reports.

---

## 🐛 Troubleshooting

### Common Issues

**"Command not found: make"**

- Install Make using the instructions in Prerequisites

**"Failed to connect to RPC"**

- Check your `SOLANA_RPC_URL` in `.env`
- Verify network connectivity
- Try using a different RPC endpoint

**"Invalid private key"**

- Ensure `KORA_PRIVATE_KEY` is a valid base58-encoded key
- Or verify your `signers.toml` points to a valid keypair file

---

## ⚠️ Disclaimer

This tool deals with private keys and account deletion. While a 24-hour safety mechanism is implemented, please run `make scan` first to verify the state of your accounts. Use at your own risk.

**Security Best Practices:**

- Never share your private keys or `.env` file
- Test on devnet before using on mainnet
- Start with manual scans before enabling automated reclamation
- Monitor the first few cycles closely

---

## 📄 License

Licensed under MIT, following the original License by Kora.

---

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

---

Built with ❤️ by xavierScript.
