use std::fs::{File, OpenOptions};
use std::path::Path;
use std::env;
use std::sync::Arc;
use std::str::FromStr;
use std::time::SystemTime;
use base64::{Engine as _, engine::general_purpose};
use csv;
use reqwest;
use solana_sdk::{
    pubkey::Pubkey,
    transaction::Transaction,
    instruction::Instruction,
    program_pack::Pack,
};
use solana_client::{
    nonblocking::rpc_client::RpcClient,
    rpc_request::TokenAccountsFilter,
};
use solana_account_decoder::UiAccountData;
use kora_lib::{
    error::KoraError,
    SolanaSigner,
    state::get_config,
    config::SplTokenConfig,
    signer::pool::SignerPool,
};
use super::types::{TokenAccountInfo, AuditRecord};
use super::config::AUDIT_FILE;

// --- Macros ---
#[macro_export]
macro_rules! log_output {
    ($tx:expr, $acc:expr, $details:expr, $color:expr) => {
        if let Some(tx) = $tx {
            let _ = tx.send(crate::rent_manager::types::UiEvent::Log($acc, $details, $color));
        } else {
            println!("{} | {}", $acc, $details);
        }
    };
}

// --- Functions ---

// Send Telegram Alert
pub async fn send_telegram_alert(message: &str) {
    let token = match env::var("KORA_TG_TOKEN") {
        Ok(t) => t,
        Err(_) => return, 
    };
    let chat_id = match env::var("KORA_TG_CHAT_ID") {
        Ok(id) => id,
        Err(_) => return,
    };

    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let client = reqwest::Client::new();
    let params = [("chat_id", chat_id.as_str()), ("text", message)];

    if let Err(e) = client.post(&url).form(&params).send().await {
        eprintln!("Failed to send Telegram: {}", e);
    }
}

// Log to Audit Trail CSV
pub fn log_to_audit_trail(record: &AuditRecord) {
    let file_exists = Path::new(AUDIT_FILE).exists();
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(AUDIT_FILE)
        .unwrap_or_else(|e| panic!("Failed to open log file: {}", e));

    let mut wtr = csv::WriterBuilder::new()
        .has_headers(!file_exists)
        .from_writer(file);

    if let Err(e) = wtr.serialize(record) {
        eprintln!("⚠️ Failed to write audit log: {}", e);
    }
    wtr.flush().unwrap();
}

// Fetch all token accounts for a given owner
pub async fn fetch_all_token_accounts(
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

// Parse token account data from UiAccountData
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

// Close a token account
pub async fn close_account(
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

// Get allowed SPL tokens from config
pub fn get_allowed_tokens() -> Result<(Vec<Pubkey>, bool), KoraError> {
    let config = get_config()?;
    let is_all = matches!(config.validation.allowed_spl_paid_tokens, SplTokenConfig::All);
    let tokens = if is_all { vec![] } else {
        config.validation.allowed_spl_paid_tokens.as_slice().iter().filter_map(|t| t.parse().ok()).collect()
    };
    Ok((tokens, is_all))
}

// Convert lamports to SOL
pub fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / 1_000_000_000.0
}

// Standalone Stats function
pub async fn show_stats(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    
    let mut total_accounts = 0;
    let mut idle_accounts = 0;
    let mut rent_locked_lamports = 0;

    println!("Gathering live blockchain data (this may take a moment)...");

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        let accounts = fetch_all_token_accounts(&rpc_client, &signer_pubkey).await?;
        
        for acc in accounts {
            total_accounts += 1;
            rent_locked_lamports += acc.lamports;
            if acc.amount == 0 {
                idle_accounts += 1;
            }
        }
    }

    let mut rent_reclaimed_30d = 0.0;
    let mut total_reclaimed_ever = 0.0;
    
    if Path::new(AUDIT_FILE).exists() {
        let file = File::open(AUDIT_FILE).map_err(|e| KoraError::InternalServerError(e.to_string()))?;
        let mut rdr = csv::Reader::from_reader(file);
        
        let now = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let thirty_days_ago = now.saturating_sub(30 * 24 * 60 * 60);

        for result in rdr.deserialize() {
            if let Ok(record) = result {
                let record: AuditRecord = record;
                total_reclaimed_ever += record.rent_reclaimed_sol;
                
                if record.timestamp >= thirty_days_ago {
                    rent_reclaimed_30d += record.rent_reclaimed_sol;
                }
            }
        }
    }

    let rent_locked_sol = lamports_to_sol(rent_locked_lamports);
    let total_capital_deployed = rent_locked_sol + total_reclaimed_ever;
    
    let efficiency = if total_capital_deployed > 0.0 {
        (total_reclaimed_ever / total_capital_deployed) * 100.0
    } else {
        0.0
    };

    println!("\n📊 KORA RENT MANAGER STATS");
    println!("--------------------------");
    println!("Total Sponsored Accounts: {}", total_accounts);
    println!("Idle Accounts:            {}", idle_accounts);
    println!("Rent Locked:              {:.4} SOL", rent_locked_sol);
    println!("Rent Reclaimed (30d):     {:.4} SOL", rent_reclaimed_30d);
    println!("Efficiency Gain:          {:.2}%", efficiency);
    println!("--------------------------");

    Ok(())
}