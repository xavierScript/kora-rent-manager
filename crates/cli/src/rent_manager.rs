use std::sync::Arc;
use std::str::FromStr;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions}; // Added OpenOptions
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
const AUDIT_FILE: &str = "audit_log.csv"; // [NEW] Audit File Name

// --- Data Structures ---

struct TokenAccountInfo {
    pubkey: Pubkey,
    mint: Pubkey,
    amount: u64,
    lamports: u64,
    program_id: Pubkey,
}

// Explicit Reason Codes
#[derive(Debug, PartialEq, Serialize)] // Added Serialize for CSV logging
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

// [NEW] Audit Log Record
#[derive(Serialize)]
struct AuditRecord {
    timestamp: u64,
    date_utc: String,
    account: String,
    mint: String,
    action: String,
    reason: String,
    rent_reclaimed_sol: f64,
    signature: String,
}

// --- Main Handler ---

pub async fn handle_rent_manager(
    command: RentManagerCommands,
    rpc_client: Arc<RpcClient>,
) -> Result<(), KoraError> {
    
    // Initialize Signers
    let rpc_args = match &command {
        RentManagerCommands::Scan { rpc_args, .. } => rpc_args,
        RentManagerCommands::Reclaim { rpc_args, .. } => rpc_args,
        RentManagerCommands::Run { rpc_args, .. } => rpc_args,
    };

    if !rpc_args.skip_signer {
        init_signers(rpc_args).await?;
    } else {
        return Err(KoraError::ValidationError(
            "Signer configuration is required for rent management.".to_string(),
        ));
    }

    let signer_pool = get_signer_pool()?;

    // Route Command
    match command {
        RentManagerCommands::Scan { all, .. } => {
            let mut tracker = GracePeriodTracker::load();
            println!("Scanning for accounts...");
            scan_accounts(rpc_client, &signer_pool, all, &mut tracker).await?;
            tracker.save();
        },
        RentManagerCommands::Reclaim { execute, force_all, .. } => {
            let mut tracker = GracePeriodTracker::load();
            if !execute {
                println!("Running in DRY-RUN mode. Use --execute to perform reclamation.");
            }
            reclaim_rent(rpc_client, &signer_pool, execute, force_all, &mut tracker).await?;
            tracker.save();
        },
        RentManagerCommands::Run { interval, force_all, .. } => {
            run_daemon(rpc_client, &signer_pool, interval, force_all).await?;
        }
    }

    Ok(())
}

// --- Daemon Logic ---

async fn run_daemon(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
    interval_str: String,
    force_all: bool,
) -> Result<(), KoraError> {
    let duration = humantime::parse_duration(&interval_str)
        .map_err(|e| KoraError::ValidationError(format!("Invalid interval format '{}': {}", interval_str, e)))?;

    println!("🤖 Kora Rent Manager Bot Started");
    println!("   Interval: {:?}", duration);
    println!("   Mode: AUTO-EXECUTE (Live Reclaims)");
    println!("   Tracking File: {}", TRACKER_FILE);
    println!("   Audit Log: {}", AUDIT_FILE);
    println!("----------------------------------------");

    loop {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        println!("\n--- [Job Start: {}] ---", timestamp);

        let mut tracker = GracePeriodTracker::load();

        match reclaim_rent(rpc_client.clone(), signer_pool, true, force_all, &mut tracker).await {
            Ok(_) => {
                tracker.save();
                println!("--- [Job Complete] ---");
            },
            Err(e) => {
                eprintln!("⚠️ Job Failed (Will retry next cycle): {}", e);
            }
        }

        println!("Sleeping for {}...", interval_str);
        tokio::time::sleep(duration).await;
    }
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
            
            let (reason, is_actionable) = if !is_empty {
                if tracker.pending_closures.remove(&pubkey_str).is_some() {
                    (ReclaimReason::FundedIgnored, false)
                } else {
                    (ReclaimReason::FundedIgnored, false)
                }
            } else if is_allowed {
                (ReclaimReason::AllowedPaymentToken, false)
            } else {
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

            if is_actionable || show_all || reason == ReclaimReason::NewDetection || reason == ReclaimReason::AllowedPaymentToken {
                let status_str = match reason {
                    ReclaimReason::ZeroBalance => "RECLAIMABLE",
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

        if !execute {
             println!("\nProcessing Signer: {} ({})", signer_info.name, signer_pubkey);
        }
        
        let accounts = fetch_all_token_accounts(&rpc_client, &signer_pubkey).await?;

        for acc in accounts {
            let pubkey_str = acc.pubkey.to_string();

            if acc.amount != 0 { 
                tracker.pending_closures.remove(&pubkey_str);
                continue; 
            }

            let is_allowed = is_all_allowed || allowed_tokens.contains(&acc.mint);
            
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

            let should_close = if force_all { true } else { !is_allowed && is_safe_time };
            let final_reason = if force_all { ReclaimReason::ForceClosed } else { reason };

            if should_close {
                println!("  - Account: {} | Reason: {:?} | Action: CLOSING", acc.pubkey, final_reason);

                if execute {
                    match close_account(&rpc_client, &signer, &acc, &signer_pubkey).await {
                        Ok(sig) => {
                            println!("    ✅ Closed. Sig: {}", sig);
                            let rent_sol = lamports_to_sol(acc.lamports);
                            reclaimed_rent += acc.lamports;
                            reclaimed_count += 1;
                            
                            // [NEW] Log to CSV
                            log_to_audit_trail(&AuditRecord {
                                timestamp: now,
                                date_utc: humantime::format_rfc3339_seconds(SystemTime::now()).to_string(),
                                account: pubkey_str.clone(),
                                mint: acc.mint.to_string(),
                                action: "RECLAIMED".to_string(),
                                reason: format!("{:?}", final_reason),
                                rent_reclaimed_sol: rent_sol,
                                signature: sig,
                            });

                            tracker.pending_closures.remove(&pubkey_str);
                        }
                        Err(e) => println!("    ❌ Failed: {}", e),
                    }
                } else {
                    reclaimed_rent += acc.lamports;
                    reclaimed_count += 1;
                }
            } else {
                if !execute {
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
    }

    if !execute {
        println!("\nDry Run Complete.");
        println!("Total Reclaimed Accounts: {}", reclaimed_count);
        println!("Total Rent: {} SOL", lamports_to_sol(reclaimed_rent));
    } else if reclaimed_count > 0 {
        println!("Cycle Summary: Reclaimed {} Accounts ({} SOL)", reclaimed_count, lamports_to_sol(reclaimed_rent));
    }

    Ok(())
}

// --- Helpers ---

// [NEW] Audit Logging Function
fn log_to_audit_trail(record: &AuditRecord) {
    // Check if file exists to know if we need headers
    let file_exists = Path::new(AUDIT_FILE).exists();
    
    // Open file in Append mode
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(AUDIT_FILE)
        .unwrap_or_else(|e| panic!("Failed to open log file: {}", e));

    let mut wtr = csv::WriterBuilder::new()
        .has_headers(!file_exists) // Only write headers if file is new
        .from_writer(file);

    if let Err(e) = wtr.serialize(record) {
        eprintln!("⚠️ Failed to write audit log: {}", e);
    }
    wtr.flush().unwrap();
}

async fn fetch_all_token_accounts(
    rpc_client: &RpcClient,
    owner: &Pubkey,
) -> Result<Vec<TokenAccountInfo>, KoraError> {
    let mut all_accounts = Vec::new();
    let programs = [spl_token_interface::id(), spl_token_2022_interface::id()];

    for program_id in programs {
        let accounts = rpc_client
            .get_token_accounts_by_owner(owner, TokenAccountsFilter::ProgramId(program_id))
            .await
            .map_err(|e| KoraError::InternalServerError(format!("RPC Error: {}", e)))?;
        
        for keyed in accounts {
            if let Some((amount, mint)) = parse_token_account_data(&keyed.account.data) {
                if let Ok(pubkey) = Pubkey::from_str(&keyed.pubkey) {
                    all_accounts.push(TokenAccountInfo {
                        pubkey, mint, amount, lamports: keyed.account.lamports, program_id,
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
    let tokens = if is_all { vec![] } else {
        config.validation.allowed_spl_paid_tokens.as_slice().iter().filter_map(|t| t.parse().ok()).collect()
    };
    Ok((tokens, is_all))
}

fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / 1_000_000_000.0
}