pub mod config;
pub mod types;
pub mod state;
pub mod tui;
pub mod logic;
pub mod utils;

use std::sync::Arc;
use solana_client::nonblocking::rpc_client::RpcClient;
use kora_lib::error::KoraError;
use kora_lib::signer::init::init_signers;
use kora_lib::state::get_signer_pool;
use crate::RentManagerCommands;
use self::logic::run_tui_task;
use self::utils::show_stats;
use self::types::OperationMode;

// --- Main Handler ---

pub async fn handle_rent_manager(
    command: RentManagerCommands,
    rpc_client: Arc<RpcClient>,
) -> Result<(), KoraError> {
    
    let rpc_args = match &command {
        RentManagerCommands::Scan { rpc_args, .. } => rpc_args,
        RentManagerCommands::Reclaim { rpc_args, .. } => rpc_args,
        RentManagerCommands::Run { rpc_args, .. } => rpc_args,
        RentManagerCommands::Stats { rpc_args } => rpc_args,
    };

    if !rpc_args.skip_signer {
        init_signers(rpc_args).await?;
    } else {
        return Err(KoraError::ValidationError(
            "Signer configuration is required.".to_string(),
        ));
    }

    let signer_pool = get_signer_pool()?;

    match command {
        RentManagerCommands::Stats { .. } => {
            show_stats(rpc_client, &signer_pool).await?;
        },
        RentManagerCommands::Scan { all, .. } => {
            run_tui_task(rpc_client, signer_pool, OperationMode::Scan { all }).await?;
        },
        RentManagerCommands::Reclaim { execute, force_all, .. } => {
            run_tui_task(rpc_client, signer_pool, OperationMode::Reclaim { execute, force_all }).await?;
        },
        RentManagerCommands::Run { interval, .. } => {
            run_tui_task(rpc_client, signer_pool, OperationMode::Daemon { interval }).await?;
        }
    }

    Ok(())
}