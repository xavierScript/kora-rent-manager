# Kora Rent Reclaim Bot

## Overview

This tool helps Kora node operators monitor and reclaim "rent-locked" SOL from their signer accounts. 
When a Kora node operates as a Paymaster or signs transactions, it often creates or funds accounts (e.g., Associated Token Accounts for payments, or temporary accounts). 
Over time, these accounts may become inactive or empty (0 balance), but the SOL used for rent remains locked on-chain.

The **Rent Reclaim Bot** (`kora rent-manager`) allows operators to:
1.  **Scan** all accounts owned by their configured Kora signers.
2.  **Identify** "reclaimable" accounts (Empty accounts that are not for allowed payment tokens).
3.  **Reclaim** the rent by closing these accounts and sending the SOL back to the signer.

## How Rent Locking Happens in Kora

Kora acts as a "Gasless Relayer". When a user interacts with an app, the app constructs a transaction and asks Kora to sign it as the **Fee Payer**.

1.  **Transaction Fees**: Kora pays the network fee (in SOL) for every transaction it signs. This is an operational cost.
2.  **Account Creation**: 
    *   If a transaction includes a `SystemProgram::CreateAccount` instruction funded by the Kora Fee Payer, the Kora node pays the **Rent Exemption** deposit (usually ~0.002 SOL per account).
    *   This SOL is "locked" in the new account.
    *   If the account belongs to a user (e.g. a new Token Account for the user), the user owns the rent.
    *   If the account belongs to the Kora Operator (e.g. an Associated Token Account for receiving fees), the Operator owns the rent.
3.  **Payment Collection**:
    *   Kora collects fees in SPL Tokens (e.g. USDC).
    *   To receive these tokens, the Kora signer often needs an Associated Token Account (ATA).
    *   Initializing this ATA costs rent.
    *   If the operator stops accepting a token, or empties the account, the ATA remains on-chain, holding the rent.

This tool focuses on cleaning up **Type 3** (Operator-owned accounts) and potentially detecting other accounts owned by the signer.

## Usage

The Rent Manager is integrated into the Kora CLI.

### Prerequisites
*   Ensure your `kora.toml` and `signers.toml` are configured correctly.
*   The tool uses the same signer configuration as the RPC server.

### Commands

#### 1. Scan for Reclaimable Accounts
This command scans all signers in your pool and lists potential accounts to close.

```bash
kora rent-manager scan --config kora.toml --signers-config signers.toml
```

**Output:**
```
Signer: my-signer (Pubkey...)
  - Account: ... | Mint: ... | Balance: 0 | Rent: 2039280 lamports | Status: RECLAIMABLE
  - Account: ... | Mint: USDC... | Balance: 0 | Rent: 2039280 lamports | Status: KEEP (Allowed Payment)

Summary:
Total Reclaimable Accounts: 1
Total Potential Rent Reclaim: 0.00203928 SOL
```

#### 2. Reclaim Rent (Dry Run)
By default, the `reclaim` command runs in dry-run mode to show what *would* happen.

```bash
kora rent-manager reclaim --config kora.toml --signers-config signers.toml
```

#### 3. Execute Reclaim
To actually close the accounts and recover SOL:

```bash
kora rent-manager reclaim --execute --config kora.toml --signers-config signers.toml
```

#### 4. Force Reclaim All
To close ALL empty accounts, even those for tokens listed in `allowed_spl_paid_tokens` (e.g. if you are shutting down the node):

```bash
kora rent-manager reclaim --execute --force-all
```

## Safety Features

*   **Allowed Tokens Protection**: By default, the bot will NOT close empty accounts for tokens listed in `allowed_spl_paid_tokens` in `kora.toml`. This prevents accidental deletion of your active payment receiving accounts.
*   **Zero Balance Check**: The bot only targets accounts with a balance of 0. It does not attempt to sweep or burn tokens.
*   **Dry Run Default**: Reclaim operations require an explicit `--execute` flag.

## Technical Details

*   **Source Code**: `crates/cli/src/rent_manager.rs`
*   **Dependencies**: Uses `spl-token-interface` and `spl-token-2022-interface` to parse account state and construct `CloseAccount` instructions.
*   **Signers**: Leverages Kora's `SignerPool` to support Local, Turnkey, Privy, and Vault signers.
