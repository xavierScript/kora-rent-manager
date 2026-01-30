use solana_sdk::pubkey::Pubkey;
use serde::{Deserialize, Serialize};
use ratatui::style::Color;

/// Represents the on-chain state of a Token Account
pub struct TokenAccountInfo {
    pub pubkey: Pubkey,
    pub mint: Pubkey,
    pub amount: u64,
    pub lamports: u64,
    pub program_id: Pubkey,
}

/// Internal enum to track why an account is being processed or skipped
#[derive(Debug, PartialEq, Serialize)]
pub enum ReclaimReason {
    ZeroBalance,
    InactiveGracePeriodPassed, 
    AllowedPaymentToken,       
    GracePeriodActive,         
    FundedIgnored,             
    NewDetection,              
    ForceClosed,               
}

/// Events sent from the Background Worker Thread -> UI Main Thread
pub enum UiEvent {
    Log(String, String, Color),                 
    StatsUpdate { reclaimed: f64, count: u64 }, 
    Status(String),                             
    TaskComplete,                               
    Alert(bool, f64),                           
}

/// Defines what logic the worker thread executes
pub enum OperationMode {
    Scan { all: bool },
    Reclaim { execute: bool, force_all: bool },
    Daemon { interval: String },
}

/// Structure for the CSV Audit Log.
#[derive(Serialize, Deserialize, Clone)] 
pub struct AuditRecord {
    pub timestamp: u64,
    pub date_utc: String,
    pub account: String,
    pub mint: String,
    pub action: String,
    pub reason: String,
    pub rent_reclaimed_sol: f64,
    pub signature: String,
}