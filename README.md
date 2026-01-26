# 🛡️ Kora Rent Manager

[![Rust](https://img.shields.io/badge/Built_with-Rust-orange?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Solana](https://img.shields.io/badge/Solana-Devnet%2FMainnet-green?style=flat-square&logo=solana)](https://solana.com/)
[![License](https://img.shields.io/badge/License-MIT-blue?style=flat-square)](LICENSE)

> **Automated Treasury Recovery for Kora Node Operators.**
> *Monitor, detect, and reclaim idle rent-locked SOL with safety and clarity.*

---

## 📺 Demo & Deep Dive
**[▶️ WATCH THE LIVE WALKTHROUGH VIDEO HERE]**
*(Replace this line with your YouTube/Loom link. This is a mandatory requirement!)*

---

## 🚨 The Problem: Silent Capital Loss
Kora makes onboarding users to Solana seamless by sponsoring account creation fees. However, this convenience creates an operational gap: **Rent-Locked SOL**.

When a Kora node creates a Token Account for a user, it deposits ~0.002 SOL (Rent Exempt Minimum).
* If 1,000 users churn or empty their wallets, **2 SOL** remains locked on-chain.
* If 100,000 users churn, **200 SOL** is lost to "zombie" accounts.

Operators rarely have the time to manually audit thousands of accounts, check balances, and sign close transactions.

## 🛠️ The Solution: Kora Rent Manager
This tool is a **set-and-forget** CLI utility and background service designed to close this operational gap. It provides a visual dashboard to monitor rent status and an automated daemon to reclaim funds safely.

### Key Features
* **📊 TUI Dashboard:** Real-time visualization of cycle efficiency, reclaimed funds, and active tasks using `Ratatui`.
* **🛡️ Safety Grace Period:** Built-in tracking ensures newly detected empty accounts are **never** closed immediately. They must remain empty for **24 hours** before being flagged as reclaimable.
* **📲 Telegram Alerts:** Passive monitoring. Get notified on your phone if idle rent exceeds a threshold (e.g., 5 SOL) or if a reclaim cycle succeeds.
* **💓 Heartbeat Reporting:** The daemon sends periodic "System Alive" snapshots to Telegram, ensuring operators know the bot is active without checking the terminal.
* **📜 Audit Trail:** Every action is logged to `audit_log.csv` for financial reconciliation.
* **⚙️ Configurable:** Customize scan intervals, thresholds, and whitelists via environment variables and CLI args.

---

## 🧠 Technical Context: How It Works

### Solana Rent Mechanics
On Solana, every account must hold a minimum amount of SOL (approx. 0.002039 SOL) to remain "rent-exempt." If an account has 0 tokens but still holds this SOL, it is essentially wasting space and money.

### The Reclaim Logic
The bot performs the following cycle:
1.  **Scan:** It queries the RPC for all Token Accounts owned by the configured Signer.
2.  **Filter:** It identifies accounts with `amount: 0` (Empty).
3.  **Safety Check (The Tracker):**
    * *Is this account whitelisted?* (Skip)
    * *Is this the first time we've seen it empty?* (Mark as "Pending", start 24h timer).
    * *Has it been empty for >24 hours?* (Mark as "Reclaimable").
4.  **Execution:** If enabled, it constructs a `closeAccount` instruction, signs it with the operator's keypair, and sends it to the network.
5.  **Alerting:** If the total rent reclaimed > 0 or total locked rent > Threshold, it fires a notification.

---

## 🚀 Getting Started

### Prerequisites
* Rust installed (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
* Solana CLI installed (optional, for key management)
* A Kora Node Operator Keypair (`.json` file)

### Installation
```bash
git clone [https://github.com/YOUR_USERNAME/kora-rent-manager.git](https://github.com/YOUR_USERNAME/kora-rent-manager.git)
cd kora-rent-manager
cargo build --release
```

### Configuration
Signers Config: Ensure your signers.toml points to your operator keypair.

Telegram Alerts (Optional): Export your bot credentials to receive phone notifications.
```bash
export KORA_TG_TOKEN="123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"
export KORA_TG_CHAT_ID="987654321"
```

## 🎮 Usage Guide
We provide a Makefile for easy operation.

### 1. 🔍 Scan (Read-Only)
View the state of your accounts without sending transactions. This populates the Dashboard with "Pending" or "Funded" statuses.
```bash
make scan
```

### 2. ⚡ Reclaim (Action)
Execute the cleanup. This will only close accounts that have passed the 24h Grace Period.
```bash
make reclaim
```

### 3. 🤖 Run Daemon (Background Service)
Run the bot continuously. It will sleep for the specified interval (default 10s) and wake up to process accounts.
```bash
# Run with default 10s interval
make run

# Run with custom interval (e.g., 1 hour)
make run INTERVAL=1h
```

## 📊 Dashboard & Monitoring

### The TUI (Terminal User Interface)
When running, the bot displays a rich terminal interface:

Performance Metrics: Real-time counter of SOL reclaimed.

Cycle Efficiency: A gauge showing how "optimized" your treasury is.

Live Logs: Detailed color-coded logs of every account checked.

<span style="color:yellow">YELLOW</span>: Account is empty but inside Grace Period.

<span style="color:green">GREEN</span>: Account successfully closed & rent recovered.

<span style="color:grey">GREY</span>: Account is funded (Skipped).

<span style="color:red">RED</span>: High Rent Alert or Error.

### The Audit Log
Check audit_log.csv for a permanent record:
```
timestamp,date_utc,account,mint,action,reason,rent_reclaimed_sol,signature
1706131200,2024-01-25T00:00:00Z,4xp...JQc,DD6...f62,RECLAIMED,InactiveGracePeriodPassed,0.0020,5Mz...123
```

## 🏆 Submission Checklist
- [x] Monitors Accounts: Scans all token accounts for specific signers.
- [x] Detects Inactive: Filters for 0 balance & tracks inactivity duration.
- [x] Reclaims Rent: Uses spl_token::instruction::close_account.
- [x] Open Source: MIT License.
- [x] Safety: 24-hour Grace Period Tracker (grace_period.json).
- [x] Clarity: TUI Dashboard + CSV Audit Trail.
- [x] Alerts: Visual Dashboard Alerts + Telegram Push Notifications + Heartbeat Reports.

## ⚠️ Disclaimer
This tool deals with private keys and account deletion. While a 24-hour safety mechanism is implemented, please run make scan first to verify the state of your accounts. Use at your own risk.

Licensed under MIT. See [LICENSE](LICENSE) for details.

Built with ❤️ by xavierScript.