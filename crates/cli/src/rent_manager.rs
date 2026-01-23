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
    program_pack::Pack,
};
// [FIX] Import UiAccountData to handle JSON responses
use solana_account_decoder::UiAccountData;

pub async fn handle_rent_manager(
    command: RentManagerCommands,
    rpc_client: Arc<RpcClient>,
) -> Result<(), KoraError> {
    match command {
        RentManagerCommands::Scan { rpc_args } => {
            if !rpc_args.skip_signer {
                init_signers(&rpc_args).await?;
            } else {
                return Err(KoraError::ValidationError(
                    "Cannot scan for signer accounts without a signer configuration.".to_string(),
                ));
            }
            let signer_pool = get_signer_pool()?;
            println!("Scanning for reclaimable accounts...");
            scan_accounts(rpc_client, &signer_pool).await?;
        }
        RentManagerCommands::Reclaim {
            rpc_args,
            execute,
            force_all,
        } => {
            if !rpc_args.skip_signer {
                init_signers(&rpc_args).await?;
            } else {
                return Err(KoraError::ValidationError(
                    "Cannot reclaim rent without a signer configuration.".to_string(),
                ));
            }
            let signer_pool = get_signer_pool()?;
            if !execute {
                println!("Running in DRY-RUN mode. Use --execute to perform reclamation.");
            }
            reclaim_rent(rpc_client, &signer_pool, execute, force_all).await?;
        }
    }
    Ok(())
}

async fn scan_accounts(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    let config = get_config()?;
    
    let is_all_allowed = matches!(config.validation.allowed_spl_paid_tokens, SplTokenConfig::All);
    let allowed_paid_tokens: Vec<Pubkey> = if is_all_allowed {
        vec![]
    } else {
        config
            .validation
            .allowed_spl_paid_tokens
            .as_slice()
            .iter()
            .filter_map(|t| t.parse().ok())
            .collect()
    };

    let mut total_reclaimable_rent = 0;
    let mut total_reclaimable_accounts = 0;

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        println!("\nSigner: {} ({})", signer_info.name, signer_pubkey);

        let mut accounts_found = 0;
        
        // [FIX] Reverted to standard call (no _with_config)
        let spl_accounts = rpc_client
            .get_token_accounts_by_owner(
                &signer_pubkey,
                TokenAccountsFilter::ProgramId(spl_token_interface::id()),
            )
            .await
            .map_err(|e| KoraError::InternalServerError(format!("Failed to fetch token accounts: {}", e)))?;

        let token_2022_accounts = rpc_client
            .get_token_accounts_by_owner(
                &signer_pubkey,
                TokenAccountsFilter::ProgramId(spl_token_2022_interface::id()),
            )
            .await
            .map_err(|e| KoraError::InternalServerError(format!("Failed to fetch token-2022 accounts: {}", e)))?;

        let all_accounts = spl_accounts
            .into_iter()
            .chain(token_2022_accounts.into_iter());

        for keyed_account in all_accounts {
            let account_pubkey = Pubkey::from_str(&keyed_account.pubkey).unwrap();
            
            // [FIX] Hybrid parsing logic: Handle both JSON and Binary
            let (amount, mint) = match &keyed_account.account.data {
                UiAccountData::Json(parsed_account) => {
                    // Handle JSON response
                    let info = parsed_account.parsed.get("info");
                    if let Some(info) = info {
                         let mint_str = info.get("mint").and_then(|v| v.as_str());
                         let amount_str = info.get("tokenAmount").and_then(|t| t.get("amount")).and_then(|v| v.as_str());
                         
                         if let (Some(m), Some(a)) = (mint_str, amount_str) {
                             if let (Ok(parsed_mint), Ok(parsed_amount)) = (Pubkey::from_str(m), a.parse::<u64>()) {
                                 (parsed_amount, parsed_mint)
                             } else { continue; }
                         } else { continue; }
                    } else { continue; }
                },
                UiAccountData::Binary(data_str, _encoding) => {
                    // Handle Binary response
                     if let Ok(bytes) = base64::decode(data_str) {
                         if let Ok(token_account) = spl_token_interface::state::Account::unpack(&bytes) {
                             (token_account.amount, token_account.mint)
                         } else if let Ok(token_account) = spl_token_2022_interface::state::Account::unpack(&bytes) {
                             (token_account.amount, token_account.mint)
                         } else { continue; }
                     } else { continue; }
                },
                _ => continue,
            };
            
            let is_allowed_payment = is_all_allowed || allowed_paid_tokens.contains(&mint);
            
            if amount == 0 {
                let rent = keyed_account.account.lamports;
                let status = if is_allowed_payment { "KEEP (Allowed Payment)" } else { "RECLAIMABLE" };
                
                println!(
                    "  - Account: {} | Mint: {} | Balance: 0 | Rent: {} lamports | Status: {}",
                    account_pubkey,
                    mint,
                    rent,
                    status
                );

                if !is_allowed_payment {
                    total_reclaimable_rent += rent;
                    total_reclaimable_accounts += 1;
                    accounts_found += 1;
                }
            }
        }
        
        if accounts_found == 0 {
            println!("  No reclaimable accounts found.");
        }
    }

    println!("\nSummary:");
    println!("Total Reclaimable Accounts: {}", total_reclaimable_accounts);
    println!("Total Potential Rent Reclaim: {} SOL", total_reclaimable_rent as f64 / 1_000_000_000.0);

    Ok(())
}

async fn reclaim_rent(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
    execute: bool,
    force_all: bool,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    let config = get_config()?;
    
    let is_all_allowed = matches!(config.validation.allowed_spl_paid_tokens, SplTokenConfig::All);
    let allowed_paid_tokens: Vec<Pubkey> = if is_all_allowed {
        vec![]
    } else {
        config
            .validation
            .allowed_spl_paid_tokens
            .as_slice()
            .iter()
            .filter_map(|t| t.parse().ok())
            .collect()
    };

    let mut reclaimed_lamports = 0;
    let mut reclaimed_count = 0;

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        let signer = signer_pool.get_signer_by_pubkey(&signer_info.public_key)?;

        println!("\nProcessing Signer: {} ({})", signer_info.name, signer_pubkey);

        let spl_accounts = rpc_client
            .get_token_accounts_by_owner(
                &signer_pubkey,
                TokenAccountsFilter::ProgramId(spl_token_interface::id()),
            )
            .await
            .map_err(|e| KoraError::InternalServerError(format!("Failed to fetch token accounts: {}", e)))?;

        let token_2022_accounts = rpc_client
            .get_token_accounts_by_owner(
                &signer_pubkey,
                TokenAccountsFilter::ProgramId(spl_token_2022_interface::id()),
            )
            .await
            .map_err(|e| KoraError::InternalServerError(format!("Failed to fetch token-2022 accounts: {}", e)))?;

        let all_accounts = spl_accounts
            .into_iter()
            .map(|a| (a, spl_token_interface::id()))
            .chain(token_2022_accounts.into_iter().map(|a| (a, spl_token_2022_interface::id())));

        for (keyed_account, program_id) in all_accounts {
            let account_pubkey = Pubkey::from_str(&keyed_account.pubkey).unwrap();
            
            // [FIX] Hybrid parsing logic again
            let (amount, mint) = match &keyed_account.account.data {
                UiAccountData::Json(parsed_account) => {
                    let info = parsed_account.parsed.get("info");
                    if let Some(info) = info {
                         let mint_str = info.get("mint").and_then(|v| v.as_str());
                         let amount_str = info.get("tokenAmount").and_then(|t| t.get("amount")).and_then(|v| v.as_str());
                         
                         if let (Some(m), Some(a)) = (mint_str, amount_str) {
                             if let (Ok(parsed_mint), Ok(parsed_amount)) = (Pubkey::from_str(m), a.parse::<u64>()) {
                                 (parsed_amount, parsed_mint)
                             } else { continue; }
                         } else { continue; }
                    } else { continue; }
                },
                UiAccountData::Binary(data_str, _) => {
                     if let Ok(bytes) = base64::decode(data_str) {
                         if let Ok(token_account) = spl_token_interface::state::Account::unpack(&bytes) {
                             (token_account.amount, token_account.mint)
                         } else if let Ok(token_account) = spl_token_2022_interface::state::Account::unpack(&bytes) {
                             (token_account.amount, token_account.mint)
                         } else { continue; }
                     } else { continue; }
                },
                _ => continue,
            };

            let is_allowed_payment = is_all_allowed || allowed_paid_tokens.contains(&mint);
            
            if amount == 0 {
                let should_close = force_all || !is_allowed_payment;
                
                if should_close {
                    println!("  - Closing Account: {} (Rent: {})", account_pubkey, keyed_account.account.lamports);
                    
                    if execute {
                        let ix = if program_id == spl_token_interface::id() {
                             spl_token_interface::instruction::close_account(
                                &program_id,
                                &account_pubkey,
                                &signer_pubkey,
                                &signer_pubkey,
                                &[&signer_pubkey],
                            ).unwrap()
                        } else {
                             spl_token_2022_interface::instruction::close_account(
                                &program_id,
                                &account_pubkey,
                                &signer_pubkey,
                                &signer_pubkey,
                                &[&signer_pubkey],
                            ).unwrap()
                        };

                        let recent_blockhash = rpc_client.get_latest_blockhash().await
                            .map_err(|e| KoraError::InternalServerError(format!("Failed to get blockhash: {}", e)))?;

                        let mut tx = Transaction::new_with_payer(
                            &[ix],
                            Some(&signer_pubkey),
                        );
                        
                        tx.message.recent_blockhash = recent_blockhash;
                        let message_bytes = tx.message.serialize();
                        
                        let signature = signer.sign_message(&message_bytes).await
                            .map_err(|e| KoraError::InternalServerError(format!("Failed to sign transaction: {}", e)))?;
                            
                        tx.signatures[0] = signature;

                        match rpc_client.send_and_confirm_transaction(&tx).await {
                            Ok(sig) => {
                                println!("    ✅ Closed. Sig: {}", sig);
                                reclaimed_lamports += keyed_account.account.lamports;
                                reclaimed_count += 1;
                            },
                            Err(e) => {
                                println!("    ❌ Failed to close: {}", e);
                            }
                        }
                    } else {
                        reclaimed_lamports += keyed_account.account.lamports;
                        reclaimed_count += 1;
                    }
                } else {
                    println!("  - Skipping Account: {} (Allowed Payment Token)", account_pubkey);
                }
            }
        }
    }

    if execute {
        println!("\nOperation Complete.");
        println!("Reclaimed {} Accounts.", reclaimed_count);
        println!("Total Reclaimed Rent: {} SOL", reclaimed_lamports as f64 / 1_000_000_000.0);
    } else {
        println!("\nDry Run Complete.");
        println!("Potential Reclaim: {} Accounts", reclaimed_count);
        println!("Potential Rent: {} SOL", reclaimed_lamports as f64 / 1_000_000_000.0);
    }

    Ok(())
}