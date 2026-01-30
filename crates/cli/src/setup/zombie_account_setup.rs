use dotenv::dotenv;
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
    program_pack::Pack,
};
use solana_system_interface::instruction::create_account;
use std::env;
use std::sync::Arc;
use spl_token_interface::instruction::initialize_mint;
use kora_lib::signer::KeypairUtil;
use spl_associated_token_account_interface::instruction::create_associated_token_account;

#[tokio::main]
async fn main() {
    dotenv().ok();
    env_logger::init();

    // 1. Setup RPC
    let rpc_url = env::var("RPC_URL").unwrap_or_else(|_| "https://api.devnet.solana.com".to_string());
    println!("Connecting to RPC: {}", rpc_url);
    
    let rpc_client = Arc::new(RpcClient::new_with_commitment(
        rpc_url.clone(),
        CommitmentConfig::confirmed(),
    ));

    // 2. Load Signer
    let private_key = env::var("KORA_PRIVATE_KEY").expect("KORA_PRIVATE_KEY must be set in .env");
    let signer = KeypairUtil::from_private_key_string(&private_key).expect("Failed to parse private key");
    let signer_pubkey = signer.pubkey();

    println!("Signer Pubkey: {}", signer_pubkey);
    
    // 3. Check Balance
    let balance = rpc_client.get_balance(&signer_pubkey).expect("Failed to get balance");
    println!("Signer Balance: {} SOL", balance as f64 / 1_000_000_000.0);

    if balance < 10_000_000 { // 0.01 SOL
        println!("⚠️  Low balance! Requesting airdrop...");
        match rpc_client.request_airdrop(&signer_pubkey, 1_000_000_000) {
            Ok(sig) => {
                println!("  Airdrop requested. Sig: {}", sig);
                std::thread::sleep(std::time::Duration::from_secs(5));
            },
            Err(e) => println!("  Failed to request airdrop: {}", e),
        }
    }

    // 4. Create New Mint
    let mint_keypair = Keypair::new();
    let mint_pubkey = mint_keypair.pubkey();
    println!("Creating new Mint: {}", mint_pubkey);

    let rent = rpc_client.get_minimum_balance_for_rent_exemption(spl_token_interface::state::Mint::LEN).unwrap();
    
    // Instruction 1: Create Account for Mint
    // solana_system_interface::instruction::create_account(from, to, lamports, space, owner)
    let create_account_ix = create_account(
        &signer_pubkey,
        &mint_pubkey,
        rent,
        spl_token_interface::state::Mint::LEN as u64,
        &spl_token_interface::id(),
    );

    // Instruction 2: Initialize Mint
    let init_mint_ix = initialize_mint(
        &spl_token_interface::id(),
        &mint_pubkey,
        &signer_pubkey,
        Some(&signer_pubkey),
        9,
    ).unwrap();

    // Instruction 3: Create Associated Token Account
    let create_ata_ix = create_associated_token_account(
        &signer_pubkey, // Payer
        &signer_pubkey, // Wallet/Owner
        &mint_pubkey,   // Mint
        &spl_token_interface::id(), // Token Program
    );
    
    // ATA address for printing
    let ata_program_id: Pubkey = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL".parse().unwrap();
    let (ata_pubkey, _) = Pubkey::find_program_address(
        &[
            &signer_pubkey.to_bytes(),
            &spl_token_interface::id().to_bytes(),
            &mint_pubkey.to_bytes(),
        ],
        &ata_program_id,
    );
    
    println!("Creating ATA: {}", ata_pubkey);

    // 5. Send Transaction
    let recent_blockhash = rpc_client.get_latest_blockhash().unwrap();
    
    let tx = Transaction::new_signed_with_payer(
        &[create_account_ix, init_mint_ix, create_ata_ix],
        Some(&signer_pubkey),
        &[&signer, &mint_keypair],
        recent_blockhash,
    );

    println!("Sending transaction...");
    match rpc_client.send_and_confirm_transaction(&tx) {
        Ok(sig) => {
            println!("✅ Success!");
            println!("Transaction Signature: {}", sig);
            println!("Created Zero-Balance Account: {} for Mint: {}", ata_pubkey, mint_pubkey);
            println!("---------------------------------------------------");
            println!("Now run:");
            println!("make scan");
        },
        Err(e) => {
            println!("❌ Failed: {}", e);
        }
    }
}