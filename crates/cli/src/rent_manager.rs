use std::sync::Arc;
use std::str::FromStr;

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
    program_pack::Pack, // [FIX] This import is required for .unpack()
};
use solana_account_decoder::UiAccountData;
use base64::{Engine as _, engine::general_purpose}; // [FIX] New Base64 Engine

// --- Data Structures ---

struct TokenAccountInfo {
    pubkey: Pubkey,
    mint: Pubkey,
    amount: u64,
    lamports: u64,
    program_id: Pubkey,
}

// --- Main Handler ---

pub async fn handle_rent_manager(
    command: RentManagerCommands,
    rpc_client: Arc<RpcClient>,
) -> Result<(), KoraError> {
    // 1. Unpack arguments
    let (rpc_args, execute, force_all, is_scan_only) = match command {
        RentManagerCommands::Scan { rpc_args } => (rpc_args, false, false, true),
        RentManagerCommands::Reclaim { rpc_args, execute, force_all } => (rpc_args, execute, force_all, false),
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
    
    // 3. Route to logic
    if is_scan_only {
        println!("Scanning for reclaimable accounts...");
        scan_accounts(rpc_client, &signer_pool).await
    } else {
        if !execute {
            println!("Running in DRY-RUN mode. Use --execute to perform reclamation.");
        }
        reclaim_rent(rpc_client, &signer_pool, execute, force_all).await
    }
}

// --- Logic Implementation ---

async fn scan_accounts(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    let (allowed_tokens, is_all_allowed) = get_allowed_tokens()?;

    let mut total_rent = 0;
    let mut total_count = 0;

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        println!("\nSigner: {} ({})", signer_info.name, signer_pubkey);

        let accounts = fetch_all_token_accounts(&rpc_client, &signer_pubkey).await?;
        let mut found = 0;

        for acc in accounts {
            let is_allowed = is_all_allowed || allowed_tokens.contains(&acc.mint);

            if acc.amount == 0 {
                let status = if is_allowed { "KEEP (Allowed)" } else { "RECLAIMABLE" };
                println!(
                    "  - Account: {} | Mint: {} | Balance: 0 | Rent: {} | Status: {}",
                    acc.pubkey, acc.mint, acc.lamports, status
                );

                if !is_allowed {
                    total_rent += acc.lamports;
                    total_count += 1;
                    found += 1;
                }
            }
        }
        
        if found == 0 {
            println!("  No reclaimable accounts found.");
        }
    }

    println!("\nSummary:");
    println!("Total Reclaimable Accounts: {}", total_count);
    println!("Total Potential Rent Reclaim: {} SOL", lamports_to_sol(total_rent));

    Ok(())
}

async fn reclaim_rent(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
    execute: bool,
    force_all: bool,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    let (allowed_tokens, is_all_allowed) = get_allowed_tokens()?;

    let mut reclaimed_rent = 0;
    let mut reclaimed_count = 0;

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        let signer = signer_pool.get_signer_by_pubkey(&signer_info.public_key)?;

        println!("\nProcessing Signer: {} ({})", signer_info.name, signer_pubkey);
        
        let accounts = fetch_all_token_accounts(&rpc_client, &signer_pubkey).await?;

        for acc in accounts {
            if acc.amount != 0 { continue; }

            let is_allowed = is_all_allowed || allowed_tokens.contains(&acc.mint);
            let should_close = force_all || !is_allowed;

            if should_close {
                println!("  - Closing Account: {} (Rent: {})", acc.pubkey, acc.lamports);

                if execute {
                    match close_account(&rpc_client, &signer, &acc, &signer_pubkey).await {
                        Ok(sig) => {
                            println!("    ✅ Closed. Sig: {}", sig);
                            reclaimed_rent += acc.lamports;
                            reclaimed_count += 1;
                        }
                        Err(e) => println!("    ❌ Failed: {}", e),
                    }
                } else {
                    reclaimed_rent += acc.lamports;
                    reclaimed_count += 1;
                }
            } else {
                println!("  - Skipping: {} (Allowed Token)", acc.pubkey);
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
    
    // [FIX] Loop prevents async closure lifetime issues
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
            // [FIX] Use new Base64 engine
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

// [FIX] Use generic impl SolanaSigner
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