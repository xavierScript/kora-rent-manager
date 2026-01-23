use std::sync::Arc;
use std::str::FromStr;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};

use crate::RentManagerCommands;
use kora_lib::{
    error::KoraError,
    signer::{init::init_signers, pool::SignerPool},
    state::{get_config, get_signer_pool},
    config::SplTokenConfig,
};
use kora_lib::SolanaSigner;
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_request::TokenAccountsFilter,
};
use solana_sdk::{
    pubkey::Pubkey,
    transaction::Transaction,
    instruction::Instruction,
    program_pack::Pack,
};
use solana_account_decoder::UiAccountData;
use base64::{Engine as _, engine::general_purpose};

// --- Constants ---
const GRACE_PERIOD_SECONDS: u64 = 24 * 60 * 60; // 24 Hours
const TRACKER_FILE: &str = "grace_period.json";

// --- Data Structures ---

struct TokenAccountInfo {
    pubkey: Pubkey,
    mint: Pubkey,
    amount: u64,
    lamports: u64,
    program_id: Pubkey,
}

// [NEW] Explicit Reason Codes
#[derive(Debug, PartialEq)]
enum ReclaimReason {
    ZeroBalance,                 // Eligible because balance is 0
    InactiveGracePeriodPassed,   // Eligible because grace period is over
    AllowedPaymentToken,         // Skipped because it's a whitelisted token
    GracePeriodActive,           // Skipped because it's too new
    FundedIgnored,               // Skipped because it has money
    NewDetection,                // Skipped because we just found it
    ForceClosed,                 // Closed despite whitelist (force flag)
}

// Simple DB for tracking timestamps
#[derive(Serialize, Deserialize, Default)]
struct GracePeriodTracker {
    // Map of Account Pubkey -> Unix Timestamp (First Seen Empty)
    pending_closures: HashMap<String, u64>,
}

impl GracePeriodTracker {
    fn load() -> Self {
        if Path::new(TRACKER_FILE).exists() {
            let data = fs::read_to_string(TRACKER_FILE).unwrap_or_default();
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    fn save(&self) {
        let json = serde_json::to_string_pretty(&self).unwrap();
        let _ = fs::write(TRACKER_FILE, json); 
    }
}

// --- Main Handler ---

pub async fn handle_rent_manager(
    command: RentManagerCommands,
    rpc_client: Arc<RpcClient>,
) -> Result<(), KoraError> {
    // 1. Unpack arguments
    let (rpc_args, execute, force_all, is_scan_only, show_all) = match command {
        RentManagerCommands::Scan { rpc_args, all } => (rpc_args, false, false, true, all),
        RentManagerCommands::Reclaim { rpc_args, execute, force_all } => (rpc_args, execute, force_all, false, false),
    };

    // 2. Initialize Signers
    if !rpc_args.skip_signer {
        init_signers(&rpc_args).await?;
    } else {
        return Err(KoraError::ValidationError(
            "Signer configuration is required for rent management.".to_string(),
        ));
    }

    let signer_pool = get_signer_pool()?;
    
    // 3. Load Tracker
    let mut tracker = GracePeriodTracker::load();
    println!("Loaded tracker with {} pending accounts.", tracker.pending_closures.len());

    // 4. Route to logic
    if is_scan_only {
        println!("Scanning for accounts...");
        scan_accounts(rpc_client, &signer_pool, show_all, &mut tracker).await?;
    } else {
        if !execute {
            println!("Running in DRY-RUN mode. Use --execute to perform reclamation.");
        }
        reclaim_rent(rpc_client, &signer_pool, execute, force_all, &mut tracker).await?;
    }

    // 5. Save Tracker updates
    tracker.save();
    println!("Updated tracker saved.");

    Ok(())
}

// --- Logic Implementation ---

async fn scan_accounts(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
    show_all: bool,
    tracker: &mut GracePeriodTracker,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    let (allowed_tokens, is_all_allowed) = get_allowed_tokens()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let mut total_rent = 0;
    let mut total_count = 0;

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        println!("\nSigner: {} ({})", signer_info.name, signer_pubkey);

        let accounts = fetch_all_token_accounts(&rpc_client, &signer_pubkey).await?;

        for acc in accounts {
            let pubkey_str = acc.pubkey.to_string();
            let is_allowed = is_all_allowed || allowed_tokens.contains(&acc.mint);
            let is_empty = acc.amount == 0;
            
            // Determine Reason
            let (reason, is_actionable) = if !is_empty {
                // If funded, remove from tracker
                if tracker.pending_closures.remove(&pubkey_str).is_some() {
                    // It was pending, but now funded
                    (ReclaimReason::FundedIgnored, false)
                } else {
                    (ReclaimReason::FundedIgnored, false)
                }
            } else if is_allowed {
                (ReclaimReason::AllowedPaymentToken, false)
            } else {
                // It is empty and not allowed. Check grace period.
                if let Some(&timestamp) = tracker.pending_closures.get(&pubkey_str) {
                    let age = now.saturating_sub(timestamp);
                    if age >= GRACE_PERIOD_SECONDS {
                        (ReclaimReason::InactiveGracePeriodPassed, true)
                    } else {
                        (ReclaimReason::GracePeriodActive, false)
                    }
                } else {
                    tracker.pending_closures.insert(pubkey_str.clone(), now);
                    (ReclaimReason::NewDetection, false)
                }
            };

            // Display Logic
            if is_actionable || show_all || reason == ReclaimReason::NewDetection || reason == ReclaimReason::AllowedPaymentToken {
                let status_str = match reason {
                    ReclaimReason::ZeroBalance => "RECLAIMABLE", // Fallback
                    ReclaimReason::InactiveGracePeriodPassed => "RECLAIMABLE (Safe)",
                    ReclaimReason::AllowedPaymentToken => "KEEP (Allowed)",
                    ReclaimReason::GracePeriodActive => "GRACE PERIOD",
                    ReclaimReason::FundedIgnored => "FUNDED (Ignored)",
                    ReclaimReason::NewDetection => "PENDING (Marked)",
                    ReclaimReason::ForceClosed => "FORCE CLOSED",
                };

                let display_balance = if is_empty { "0".to_string() } else { acc.amount.to_string() };

                println!(
                    "  - Account: {} | Mint: {} | Balance: {} | Reason: {:?} | Status: {}",
                    acc.pubkey, acc.mint, display_balance, reason, status_str
                );

                if is_actionable {
                    total_rent += acc.lamports;
                    total_count += 1;
                }
            }
        }
    }

    println!("\nSummary:");
    println!("Total Ready to Reclaim: {}", total_count);
    println!("Total Potential Rent: {} SOL", lamports_to_sol(total_rent));

    Ok(())
}

async fn reclaim_rent(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
    execute: bool,
    force_all: bool,
    tracker: &mut GracePeriodTracker,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    let (allowed_tokens, is_all_allowed) = get_allowed_tokens()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let mut reclaimed_rent = 0;
    let mut reclaimed_count = 0;

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        let signer = signer_pool.get_signer_by_pubkey(&signer_info.public_key)?;

        println!("\nProcessing Signer: {} ({})", signer_info.name, signer_pubkey);
        
        let accounts = fetch_all_token_accounts(&rpc_client, &signer_pubkey).await?;

        for acc in accounts {
            let pubkey_str = acc.pubkey.to_string();

            // 1. Safety Filter: Ignore Funded
            if acc.amount != 0 { 
                tracker.pending_closures.remove(&pubkey_str);
                continue; 
            }

            // 2. Determine Eligibility
            let is_allowed = is_all_allowed || allowed_tokens.contains(&acc.mint);
            
            // Check Grace Period Status
            let (is_safe_time, reason) = if let Some(&timestamp) = tracker.pending_closures.get(&pubkey_str) {
                if (now.saturating_sub(timestamp)) >= GRACE_PERIOD_SECONDS {
                    (true, ReclaimReason::InactiveGracePeriodPassed)
                } else {
                    (false, ReclaimReason::GracePeriodActive)
                }
            } else {
                tracker.pending_closures.insert(pubkey_str.clone(), now);
                (false, ReclaimReason::NewDetection)
            };

            // 3. Decision Matrix
            let should_close = if force_all {
                true 
            } else {
                !is_allowed && is_safe_time
            };

            let final_reason = if force_all { ReclaimReason::ForceClosed } else { reason };

            if should_close {
                println!("  - Account: {} | Reason: {:?} | Action: CLOSING", acc.pubkey, final_reason);

                if execute {
                    match close_account(&rpc_client, &signer, &acc, &signer_pubkey).await {
                        Ok(sig) => {
                            println!("    ✅ Closed. Sig: {}", sig);
                            reclaimed_rent += acc.lamports;
                            reclaimed_count += 1;
                            tracker.pending_closures.remove(&pubkey_str);
                        }
                        Err(e) => println!("    ❌ Failed: {}", e),
                    }
                } else {
                    reclaimed_rent += acc.lamports;
                    reclaimed_count += 1;
                }
            } else {
                // Log why we are skipping
                let skip_msg = if is_allowed {
                    "Allowed Payment Token"
                } else {
                    match final_reason {
                        ReclaimReason::NewDetection => "New Detection (Grace Period Started)",
                        ReclaimReason::GracePeriodActive => "Grace Period Active",
                        _ => "Unknown",
                    }
                };
                println!("  - Account: {} | Reason: {} | Action: SKIP", acc.pubkey, skip_msg);
            }
        }
    }

    let mode = if execute { "Operation" } else { "Dry Run" };
    println!("\n{} Complete.", mode);
    println!("Total Reclaimed Accounts: {}", reclaimed_count);
    println!("Total Rent: {} SOL", lamports_to_sol(reclaimed_rent));

    Ok(())
}

// --- Helpers ---

async fn fetch_all_token_accounts(
    rpc_client: &RpcClient,
    owner: &Pubkey,
) -> Result<Vec<TokenAccountInfo>, KoraError> {
    let mut all_accounts = Vec::new();
    
    let programs = [
        spl_token_interface::id(),
        spl_token_2022_interface::id(),
    ];

    for program_id in programs {
        let accounts = rpc_client
            .get_token_accounts_by_owner(owner, TokenAccountsFilter::ProgramId(program_id))
            .await
            .map_err(|e| KoraError::InternalServerError(format!("RPC Error: {}", e)))?;
        
        for keyed in accounts {
            if let Some((amount, mint)) = parse_token_account_data(&keyed.account.data) {
                if let Ok(pubkey) = Pubkey::from_str(&keyed.pubkey) {
                    all_accounts.push(TokenAccountInfo {
                        pubkey,
                        mint,
                        amount,
                        lamports: keyed.account.lamports,
                        program_id,
                    });
                }
            }
        }
    }

    Ok(all_accounts)
}

fn parse_token_account_data(data: &UiAccountData) -> Option<(u64, Pubkey)> {
    match data {
        UiAccountData::Json(parsed) => {
            let info = parsed.parsed.get("info")?;
            let mint = info.get("mint")?.as_str()?;
            let amount = info.get("tokenAmount")?.get("amount")?.as_str()?;
            
            Some((amount.parse().ok()?, Pubkey::from_str(mint).ok()?))
        },
        UiAccountData::Binary(data_str, _) => {
            let bytes = general_purpose::STANDARD.decode(data_str).ok()?;
            
            if let Ok(acc) = spl_token_interface::state::Account::unpack(&bytes) {
                return Some((acc.amount, acc.mint));
            }
            if let Ok(acc) = spl_token_2022_interface::state::Account::unpack(&bytes) {
                return Some((acc.amount, acc.mint));
            }
            None
        },
        _ => None,
    }
}

async fn close_account(
    rpc_client: &RpcClient,
    signer: &Arc<impl SolanaSigner>, 
    account: &TokenAccountInfo,
    owner: &Pubkey,
) -> Result<String, KoraError> {
    let ix: Instruction = if account.program_id == spl_token_interface::id() {
        spl_token_interface::instruction::close_account(
            &account.program_id, &account.pubkey, owner, owner, &[owner]
        ).unwrap()
    } else {
        spl_token_2022_interface::instruction::close_account(
            &account.program_id, &account.pubkey, owner, owner, &[owner]
        ).unwrap()
    };

    let recent_blockhash = rpc_client.get_latest_blockhash().await
        .map_err(|e| KoraError::InternalServerError(e.to_string()))?;

    let mut tx = Transaction::new_with_payer(&[ix], Some(owner));
    tx.message.recent_blockhash = recent_blockhash;
    
    let signature = signer.sign_message(&tx.message.serialize()).await
        .map_err(|e| KoraError::InternalServerError(e.to_string()))?;
    
    tx.signatures[0] = signature;

    rpc_client.send_and_confirm_transaction(&tx).await
        .map(|s| s.to_string())
        .map_err(|e| KoraError::InternalServerError(e.to_string()))
}

fn get_allowed_tokens() -> Result<(Vec<Pubkey>, bool), KoraError> {
    let config = get_config()?;
    let is_all = matches!(config.validation.allowed_spl_paid_tokens, SplTokenConfig::All);
    
    let tokens = if is_all {
        vec![]
    } else {
        config.validation.allowed_spl_paid_tokens
            .as_slice()
            .iter()
            .filter_map(|t| t.parse().ok())
            .collect()
    };
    
    Ok((tokens, is_all))
}

fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / 1_000_000_000.0
}