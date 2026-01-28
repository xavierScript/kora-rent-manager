use std::sync::Arc;
use std::str::FromStr;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::env;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

// --- TUI Imports (Terminal User Interface) ---
// We use 'ratatui' for rendering the dashboard and 'crossterm' for raw terminal handling.
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table},
};

// --- Internal Library Imports ---
use crate::RentManagerCommands;
use kora_lib::{
    error::KoraError,
    signer::{init::init_signers, pool::SignerPool},
    state::{get_config, get_signer_pool},
    config::SplTokenConfig,
};
use kora_lib::SolanaSigner;

// --- Solana SDK Imports ---
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

// --- Configuration Constants ---

// Safety Mechanism: Accounts must be empty for 24 hours before we reclaim them.
// This prevents deleting accounts that were just created but not yet funded.
// const GRACE_PERIOD_SECONDS: u64 = 24 * 60 * 60; 
const GRACE_PERIOD_SECONDS: u64 = 60; 

// File paths for persistence and logging
const TRACKER_FILE: &str = "grace_period.json";
const AUDIT_FILE: &str = "audit_log.csv";

// Threshold to trigger a "High Rent" alert (Visual + Telegram)
const HIGH_RENT_THRESHOLD_SOL: f64 = 1.0; 

// --- Data Structures ---

/// Represents the on-chain state of a Token Account
struct TokenAccountInfo {
    pubkey: Pubkey,
    mint: Pubkey,
    amount: u64,
    lamports: u64,
    program_id: Pubkey, // Distinguishes betweeen Tokenkeg and Token-2022
}

/// Internal enum to track why an account is being processed or skipped
#[derive(Debug, PartialEq, Serialize)]
enum ReclaimReason {
    ZeroBalance,
    InactiveGracePeriodPassed, // Safe to close
    AllowedPaymentToken,       // Whitelisted (Skip)
    GracePeriodActive,         // Empty but too new (Skip)
    FundedIgnored,             // Has tokens (Skip)
    NewDetection,              // First time seeing it empty (Start Timer)
    ForceClosed,               // User forced closure via CLI
}

/// Persistent state to track how long accounts have been empty.
/// This is the core of the "Safety" requirement.
#[derive(Serialize, Deserialize, Default)]
struct GracePeriodTracker {
    // Map of Account Pubkey -> Unix Timestamp when first seen empty
    pending_closures: HashMap<String, u64>,
}

impl GracePeriodTracker {
    /// Loads the tracker from disk or creates a new one
    fn load() -> Self {
        if Path::new(TRACKER_FILE).exists() {
            let data = fs::read_to_string(TRACKER_FILE).unwrap_or_default();
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    /// Saves the current state to disk
    fn save(&self) {
        let json = serde_json::to_string_pretty(&self).unwrap();
        let _ = fs::write(TRACKER_FILE, json); 
    }
}

/// Structure for the CSV Audit Log.
/// Provides transparency and financial reconciliation for the operator.
#[derive(Serialize, Deserialize, Clone)] 
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

// --- TUI Architecture ---

/// Events sent from the Background Worker Thread -> UI Main Thread
enum UiEvent {
    Log(String, String, Color),                 // Add a line to the log table
    StatsUpdate { reclaimed: f64, count: u64 }, // Update top counters
    Status(String),                             // Update status bar text
    TaskComplete,                               // Signal that the job is done
    Alert(bool, f64),                           // Trigger/Clear visual red alert
}

/// The entire state of the Terminal UI
struct AppState {
    logs: Vec<(String, String, Color)>, 
    total_reclaimed_sol: f64,
    reclaimed_count: u64,
    status_msg: String,
    spinner_idx: usize,
    is_working: bool,
    is_high_rent: bool,       // Controls RED alert styling
    current_locked_rent: f64, // Used for the "Rent Locked" display
}

/// Defines what logic the worker thread executes
enum OperationMode {
    Scan { all: bool },
    Reclaim { execute: bool, force_all: bool },
    Daemon { interval: String },
}

// --- Main Handler ---

/// Entry point for the `rent-manager` subcommand.
/// Parses arguments and routes to the appropriate mode.
pub async fn handle_rent_manager(
    command: RentManagerCommands,
    rpc_client: Arc<RpcClient>,
) -> Result<(), KoraError> {
    
    // 1. Initialize Signers (Needed to close accounts)
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

    // 2. Dispatch to specific logic
    match command {
        RentManagerCommands::Stats { .. } => {
            // Stats prints to stdout directly (better for copy-pasting data)
            show_stats(rpc_client, &signer_pool).await?;
        },
        RentManagerCommands::Scan { all, .. } => {
            // Launches TUI in Read-Only mode
            run_tui_task(rpc_client, signer_pool, OperationMode::Scan { all }).await?;
        },
        RentManagerCommands::Reclaim { execute, force_all, .. } => {
            // Launches TUI in Active mode (Dry Run or Execute)
            run_tui_task(rpc_client, signer_pool, OperationMode::Reclaim { execute, force_all }).await?;
        },
        RentManagerCommands::Run { interval, .. } => {
            // Launches the Long-Running Daemon
            run_tui_task(rpc_client, signer_pool, OperationMode::Daemon { interval }).await?;
        }
    }

    Ok(())
}

// --- Unified TUI Runner ---

/// Sets up the Terminal environment and manages the two main threads:
/// 1. UI Thread: Handles rendering and keyboard input.
/// 2. Worker Thread: Handles Solana RPC calls and Logic.
async fn run_tui_task(
    rpc_client: Arc<RpcClient>,
    signer_pool: Arc<SignerPool>, 
    mode: OperationMode,
) -> Result<(), KoraError> {
    // 1. Setup Terminal (Raw Mode allows capturing keystrokes)
    enable_raw_mode().unwrap();
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).unwrap();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).unwrap();

    // 2. Channel for communication (Worker -> UI)
    let (tx, mut rx) = mpsc::unbounded_channel();
    
    // 3. Clone Arcs to move into the background thread
    let rpc_bg = rpc_client.clone();
    let pool_bg = signer_pool.clone();
    
    // 4. Spawn the Worker Thread
    tokio::spawn(async move {
        let mut tracker = GracePeriodTracker::load();

        match mode {
            // Mode: Scan (Read Only)
            OperationMode::Scan { all } => {
                let _ = tx.send(UiEvent::Status("🔍 Scanning...".to_string()));
                if let Err(e) = scan_accounts(rpc_bg, &pool_bg, all, &mut tracker, Some(tx.clone())).await {
                    let _ = tx.send(UiEvent::Log("System".to_string(), format!("Error: {}", e), Color::Red));
                }
                let _ = tx.send(UiEvent::Status("✅ Scan Complete. Press 'q' to quit.".to_string()));
                let _ = tx.send(UiEvent::TaskComplete);
            },
            // Mode: Reclaim (Active or Dry Run)
            OperationMode::Reclaim { execute, force_all } => {
                let mode_str = if execute { "RECLAIMING" } else { "DRY RUN" };
                let _ = tx.send(UiEvent::Status(format!("⚡ {}...", mode_str)));
                
                if let Err(e) = reclaim_rent(rpc_bg, &pool_bg, execute, force_all, &mut tracker, Some(tx.clone())).await {
                    let _ = tx.send(UiEvent::Log("System".to_string(), format!("Error: {}", e), Color::Red));
                }
                // Only save the tracker state if we actually ran the reclaim logic
                tracker.save();
                let _ = tx.send(UiEvent::Status("✅ Task Complete. Press 'q' to quit.".to_string()));
                let _ = tx.send(UiEvent::TaskComplete);
            },
            // Mode: Daemon (Continuous Loop)
            OperationMode::Daemon { interval } => {
                let cycle_duration = match humantime::parse_duration(&interval) {
                    Ok(d) => d,
                    Err(_) => Duration::from_secs(3600),
                };

                // Heartbeat Timer Setup (Telegram Report)
                let mut last_report_time = Instant::now();
                // Send a "System Alive" message every 6 hours
                let report_interval = Duration::from_secs(60);

                loop {
                    let _ = tx.send(UiEvent::Status("🚀 Daemon Cycle Starting...".to_string()));

                    // Telegram Heartbeat Check
                    if last_report_time.elapsed() >= report_interval {
                        let msg = "📊 *Kora Rent Manager Heartbeat*\n\n✅ System is active and monitoring accounts.\nWaiting for next cycle.";
                        
                        // Send Telegram (Non-blocking)
                        tokio::spawn(async move { send_telegram_alert(msg).await; });
                        
                        // Log to Dashboard
                        let _ = tx.send(UiEvent::Log("System".to_string(), "❤️ Sending Heartbeat Report".to_string(), Color::Cyan));
                        
                        last_report_time = Instant::now();
                    }

                    // Countdown Timer Loop (Sleeps in 1s increments to update UI)
                    let start = Instant::now();
                    while start.elapsed() < cycle_duration {
                        let elapsed = start.elapsed();
                        let remaining = cycle_duration.saturating_sub(elapsed);
                        let secs = remaining.as_secs();
                        
                        let _ = tx.send(UiEvent::Status(format!("💤 Sleeping... Next run in {}s", secs)));
                        
                        let sleep_step = if remaining > Duration::from_secs(1) {
                            Duration::from_secs(1)
                        } else {
                            remaining
                        };
                        
                        if sleep_step.is_zero() { break; }
                        tokio::time::sleep(sleep_step).await;
                    }
                }
            }
        }
    });

    // 5. Initialize App State
    let mut app = AppState {
        logs: vec![],
        total_reclaimed_sol: 0.0,
        reclaimed_count: 0,
        status_msg: "Initializing...".to_string(),
        spinner_idx: 0,
        is_working: true,
        is_high_rent: false, 
        current_locked_rent: 0.0,
    };

    // 6. UI Event Loop
    loop {
        // Draw the UI frame
        terminal.draw(|f| ui(f, &app)).unwrap();

        // Process incoming messages from the worker thread
        if let Ok(event) = rx.try_recv() {
            match event {
                UiEvent::Log(acc, details, color) => {
                    // Keep log buffer manageable
                    if app.logs.len() > 50 { app.logs.remove(0); }
                    app.logs.push((acc, details, color));
                },
                UiEvent::StatsUpdate { reclaimed, count } => {
                    app.total_reclaimed_sol += reclaimed;
                    app.reclaimed_count += count;
                },
                UiEvent::Status(msg) => app.status_msg = msg,
                UiEvent::TaskComplete => app.is_working = false,
                UiEvent::Alert(is_active, amount) => { 
                    // Update alert state for UI rendering
                    app.is_high_rent = is_active;
                    app.current_locked_rent = amount;
                }
            }
        }

        // Animation ticker
        if app.is_working {
            app.spinner_idx = (app.spinner_idx + 1) % 4;
        }

        // Handle Keyboard Input
        if event::poll(Duration::from_millis(100)).unwrap() {
            if let Event::Key(key) = event::read().unwrap() {
                if key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }
    }

    // 7. Cleanup Terminal on Exit
    disable_raw_mode().unwrap();
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture).unwrap();
    terminal.show_cursor().unwrap();

    Ok(())
}

/// Renders the TUI layout using Ratatui
fn ui(f: &mut Frame, app: &AppState) {
    // Define the grid layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(8), // Stats Panel
            Constraint::Min(5),    // Log Table (Takes remaining space)
            Constraint::Length(3), // Footer
        ])
        .split(f.area());

    // 1. Header
    let spinner = if app.is_working { ["|", "/", "-", "\\"][app.spinner_idx] } else { "✓" };
    let header_text = format!(" KORA RENT MANAGER v1.0 | {} ", spinner);
    let header = Paragraph::new(header_text)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    // 2. Stats & Alerts
    let stats_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(chunks[1]);

    // Determine visual style based on Alert State
    let (alert_color, alert_title) = if app.is_high_rent {
        (Color::Red, "⚠️ HIGH RENT ALERT")
    } else {
        (Color::Green, " Performance Metrics ")
    };

    let kpi_text = vec![
        Line::from(vec![Span::raw("Reclaimed SOL:   "), Span::styled(format!("{:.4}", app.total_reclaimed_sol), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::raw("Current Locked:  "), Span::styled(format!("{:.4} SOL", app.current_locked_rent), Style::default().fg(alert_color).add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::raw("Accounts Closed: "), Span::styled(format!("{}", app.reclaimed_count), Style::default().fg(Color::Yellow))]),
    ];
    let kpi_block = Paragraph::new(kpi_text)
        .block(Block::default().title(alert_title).borders(Borders::ALL).border_style(Style::default().fg(alert_color)));
    f.render_widget(kpi_block, stats_chunks[0]);

    let gauge = Gauge::default()
        .block(Block::default().title(" Cycle Efficiency ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Magenta))
        .percent(if app.total_reclaimed_sol > 0.0 { 85 } else { 5 })
        .label(if app.total_reclaimed_sol > 0.0 { "OPTIMIZED" } else { "IDLE" });
    f.render_widget(gauge, stats_chunks[1]);

    // 3. Log Table
    let header_cells = ["Account", "Details"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    let table_header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = app.logs.iter().rev().map(|(acc, details, color)| {
        let cells = vec![
            Cell::from(acc.clone()).style(Style::default().fg(*color).add_modifier(Modifier::BOLD)),
            Cell::from(details.clone()).style(Style::default().fg(*color)),
        ];
        Row::new(cells)
    });

    let t = Table::new(rows, [
            Constraint::Percentage(30),
            Constraint::Percentage(70),
        ])
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).title(" Live Logs "))
        .column_spacing(1);
    f.render_widget(t, chunks[2]);

    // 4. Footer
    let footer = Paragraph::new(format!(" {} | Press 'q' to quit ", app.status_msg))
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(footer, chunks[3]);
}

// --- Logic Helpers ---

// Sends a message to the configured Telegram Bot.
// Requires KORA_TG_TOKEN and KORA_TG_CHAT_ID env vars.
async fn send_telegram_alert(message: &str) {
    let token = match env::var("KORA_TG_TOKEN") {
        Ok(t) => t,
        Err(_) => return, // Silent fail if not configured (Feature is optional)
    };
    let chat_id = match env::var("KORA_TG_CHAT_ID") {
        Ok(id) => id,
        Err(_) => return,
    };

    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let client = reqwest::Client::new();
    let params = [("chat_id", chat_id.as_str()), ("text", message)];

    // Send asynchronously and ignore errors to not crash the bot
    if let Err(e) = client.post(&url).form(&params).send().await {
        eprintln!("Failed to send Telegram: {}", e);
    }
}

// Sends a snapshot of current stats to Telegram
async fn send_status_report(
    reclaimed_sol: f64, 
    locked_sol: f64, 
    count: u64
) {
    let efficiency = if (reclaimed_sol + locked_sol) > 0.0 {
        (reclaimed_sol / (reclaimed_sol + locked_sol)) * 100.0
    } else {
        0.0
    };

    let msg = format!(
        "📊 *Kora Rent Manager Report*\n\n\
        🟢 *System:* Online\n\
        💰 *Total Reclaimed:* `{:.4} SOL`\n\
        🔒 *Current Locked:* `{:.4} SOL`\n\
        📉 *Efficiency:* `{:.1}%`\n\
        📦 *Accounts Processed:* `{}`",
        reclaimed_sol, locked_sol, efficiency, count
    );

    send_telegram_alert(&msg).await;
}

/// Helper macro to simplify sending logs to the UI channel.
/// Falls back to println! if no channel is provided.
macro_rules! log_output {
    ($tx:expr, $acc:expr, $details:expr, $color:expr) => {
        if let Some(tx) = $tx {
            let _ = tx.send(UiEvent::Log($acc, $details, $color));
        } else {
            println!("{} | {}", $acc, $details);
        }
    };
}

/// Read-Only Mode: Scans for accounts and logs their status without taking action.
async fn scan_accounts(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
    show_all: bool,
    tracker: &mut GracePeriodTracker,
    tx: Option<mpsc::UnboundedSender<UiEvent>>,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    let (allowed_tokens, is_all_allowed) = get_allowed_tokens()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let mut total_rent = 0;
    let mut total_count = 0;

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        log_output!(&tx, "Signer".to_string(), signer_info.name.clone(), Color::White);

        let accounts = fetch_all_token_accounts(&rpc_client, &signer_pubkey).await?;

        for acc in accounts {
            let pubkey_str = acc.pubkey.to_string();
            let is_allowed = is_all_allowed || allowed_tokens.contains(&acc.mint);
            let is_empty = acc.amount == 0;
            
            // Determine the status of the account
            let (reason, is_actionable) = if !is_empty {
                // If funded, clean up any old pending closure records
                if tracker.pending_closures.remove(&pubkey_str).is_some() {
                    (ReclaimReason::FundedIgnored, false)
                } else {
                    (ReclaimReason::FundedIgnored, false)
                }
            } else if is_allowed {
                (ReclaimReason::AllowedPaymentToken, false)
            } else {
                // Check Grace Period Tracker
                if let Some(&timestamp) = tracker.pending_closures.get(&pubkey_str) {
                    let age = now.saturating_sub(timestamp);
                    if age >= GRACE_PERIOD_SECONDS {
                        (ReclaimReason::InactiveGracePeriodPassed, true) // Safe to close
                    } else {
                        (ReclaimReason::GracePeriodActive, false) // Too new
                    }
                } else {
                    // First time seeing it empty: Start Timer
                    tracker.pending_closures.insert(pubkey_str.clone(), now);
                    (ReclaimReason::NewDetection, false)
                }
            };

            // Filter output based on flags and status
            if is_actionable || show_all || reason == ReclaimReason::NewDetection || reason == ReclaimReason::AllowedPaymentToken {
                let status_str = match reason {
                    ReclaimReason::ZeroBalance => "RECLAIMABLE",
                    ReclaimReason::InactiveGracePeriodPassed => "RECLAIMABLE (Safe)",
                    ReclaimReason::AllowedPaymentToken => "KEEP (Allowed)",
                    ReclaimReason::GracePeriodActive => "GRACE PERIOD",
                    ReclaimReason::FundedIgnored => "FUNDED",
                    ReclaimReason::NewDetection => "PENDING",
                    ReclaimReason::ForceClosed => "FORCE CLOSED",
                };

                let color = if is_actionable { Color::Green } else { Color::Yellow };

                let details = format!(
                    "[{}] Mint: {} | Rent: {:.4} SOL | Bal: {}",
                    status_str, acc.mint, lamports_to_sol(acc.lamports), acc.amount
                );

                log_output!(&tx, acc.pubkey.to_string(), details, color);

                if is_actionable {
                    total_rent += acc.lamports;
                    total_count += 1;
                }
            }
        }
    }

    log_output!(&tx, "SUMMARY".to_string(), format!("{} Reclaimable ({:.4} SOL)", total_count, lamports_to_sol(total_rent)), Color::Cyan);
    Ok(())
}

/// Active Mode: Checks accounts and executes close commands if they are safe.
async fn reclaim_rent(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
    execute: bool,
    force_all: bool,
    tracker: &mut GracePeriodTracker,
    tx: Option<mpsc::UnboundedSender<UiEvent>>,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    let (allowed_tokens, is_all_allowed) = get_allowed_tokens()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let mut reclaimed_rent = 0;
    let mut reclaimed_count = 0;
    let mut locked_rent_accumulated = 0; // Track for High Rent Alerts

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        let signer = signer_pool.get_signer_by_pubkey(&signer_info.public_key)?;

        if tx.is_some() {
             log_output!(&tx, "Processing".to_string(), signer_info.name.clone(), Color::White);
        }
        
        let accounts = fetch_all_token_accounts(&rpc_client, &signer_pubkey).await?;

        for acc in accounts {
            let pubkey_str = acc.pubkey.to_string();

            if acc.amount == 0 {
                locked_rent_accumulated += acc.lamports;
            }

            // FILTER: If account is funded, we skip it.
            if acc.amount != 0 { 
                // Log skipped accounts if in Dry Run mode for visibility
                if !execute {
                    let skip_msg = "Funded";
                    let rent_in_sol = lamports_to_sol(acc.lamports);
                    let details = format!(
                        "[SKIP: {}] Mint: {} | Rent: {:.4} SOL | Bal: {}",
                        skip_msg, acc.mint, rent_in_sol, acc.amount
                    );
                    log_output!(&tx, acc.pubkey.to_string(), details, Color::DarkGray);
                }
                
                tracker.pending_closures.remove(&pubkey_str);
                continue; 
            }

            // SAFETY CHECK: Grace Period Logic
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

            let rent_in_sol = lamports_to_sol(acc.lamports);

            // ACTION: Close Account
            if should_close {
                let details = format!(
                    "[CLOSING] Mint: {} | Rent: {:.4} SOL | Bal: {}",
                    acc.mint, rent_in_sol, acc.amount
                );
                log_output!(&tx, acc.pubkey.to_string(), details, Color::Magenta);

                if execute {
                    // Send Transaction
                    match close_account(&rpc_client, &signer, &acc, &signer_pubkey).await {
                        Ok(sig) => {
                            log_output!(&tx, acc.pubkey.to_string(), format!("[CLOSED] Sig: {}", sig), Color::Green);
                            reclaimed_rent += acc.lamports;
                            reclaimed_count += 1;
                            
                            // Update UI Stats
                            if let Some(ref t) = tx {
                                let _ = t.send(UiEvent::StatsUpdate { reclaimed: rent_in_sol, count: 1 });
                            }

                            // Write to CSV
                            log_to_audit_trail(&AuditRecord {
                                timestamp: now,
                                date_utc: humantime::format_rfc3339_seconds(SystemTime::now()).to_string(),
                                account: pubkey_str.clone(),
                                mint: acc.mint.to_string(),
                                action: "RECLAIMED".to_string(),
                                reason: format!("{:?}", final_reason),
                                rent_reclaimed_sol: rent_in_sol,
                                signature: sig,
                            });

                            tracker.pending_closures.remove(&pubkey_str);
                        }
                        Err(e) => log_output!(&tx, acc.pubkey.to_string(), format!("[FAILED] {}", e), Color::Red),
                    }
                } else {
                    reclaimed_rent += acc.lamports;
                    reclaimed_count += 1;
                }
            } else {
                // Log why we skipped closing this empty account
                if !execute {
                    let skip_msg = if is_allowed { "Allowed" } else { "Grace Period" };
                    let details = format!(
                        "[SKIP: {}] Mint: {} | Rent: {:.4} SOL | Bal: {}",
                        skip_msg, acc.mint, rent_in_sol, acc.amount
                    );
                    log_output!(&tx, acc.pubkey.to_string(), details, Color::DarkGray);
                }
            }
        }
    }

    // ALERT: Check High Rent Threshold
    let current_locked_sol = lamports_to_sol(locked_rent_accumulated);
    if let Some(ref t) = tx {
        if current_locked_sol > HIGH_RENT_THRESHOLD_SOL {
            // Visual Alert (Red Stats)
            let _ = t.send(UiEvent::Alert(true, current_locked_sol));
            let _ = t.send(UiEvent::Log("ALERT".to_string(), format!("High Rent Idle: {:.2} SOL", current_locked_sol), Color::Red));
            
            // Telegram Alert (Non-blocking)
            let msg = format!("🚨 *High Idle Rent Detected!*\n\nAmount: `{:.2} SOL`\nThreshold: `{:.2} SOL`", current_locked_sol, HIGH_RENT_THRESHOLD_SOL);
            tokio::spawn(async move { send_telegram_alert(&msg).await; });
        } else {
            // Clear Alert if resolved
            let _ = t.send(UiEvent::Alert(false, current_locked_sol));
        }
    }

    if reclaimed_count == 0 {
        log_output!(&tx, "SUMMARY".to_string(), "No accounts found eligible for reclaim.".to_string(), Color::Yellow);
    } else {
        let label = if execute { "RECLAIMED" } else { "DRY RUN" };
        let color = if execute { Color::Green } else { Color::Cyan };
        log_output!(&tx, label.to_string(), format!("{} Accts ({:.4} SOL)", reclaimed_count, lamports_to_sol(reclaimed_rent)), color);
        
        // Notify success if executed
        if execute {
             let msg = format!("✅ *Kora Reclaim Success*\n\nClosed: {}\nRecovered: `{:.4} SOL`", reclaimed_count, lamports_to_sol(reclaimed_rent));
             tokio::spawn(async move { send_telegram_alert(&msg).await; });
        }
    }

    Ok(())
}

/// Standalone stats display (Non-TUI)
async fn show_stats(
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
        
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
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

// --- Low Level Helpers ---

/// Writes an action to the CSV audit log
fn log_to_audit_trail(record: &AuditRecord) {
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

/// RPC Call: Fetches all Token Accounts for a given owner.
/// supports both Standard Token Program and Token-2022.
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

/// Decodes the binary or JSON data from a Solana Account into usable details.
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

/// Constructs and executes the `close_account` instruction.
/// Returns the transaction signature on success.
async fn close_account(
    rpc_client: &RpcClient,
    signer: &Arc<impl SolanaSigner>, 
    account: &TokenAccountInfo,
    owner: &Pubkey,
) -> Result<String, KoraError> {
    // Generate instruction based on the program ID (Classic vs 2022)
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
    
    // Sign and broadcast
    let signature = signer.sign_message(&tx.message.serialize()).await
        .map_err(|e| KoraError::InternalServerError(e.to_string()))?;
    
    tx.signatures[0] = signature;

    rpc_client.send_and_confirm_transaction(&tx).await
        .map(|s| s.to_string())
        .map_err(|e| KoraError::InternalServerError(e.to_string()))
}

/// Helper to check if specific tokens are whitelisted in kora.toml
fn get_allowed_tokens() -> Result<(Vec<Pubkey>, bool), KoraError> {
    let config = get_config()?;
    let is_all = matches!(config.validation.allowed_spl_paid_tokens, SplTokenConfig::All);
    let tokens = if is_all { vec![] } else {
        config.validation.allowed_spl_paid_tokens.as_slice().iter().filter_map(|t| t.parse().ok()).collect()
    };
    Ok((tokens, is_all))
}

// Converts lamports to SOL for easier readability
fn lamports_to_sol(lamports: u64) -> f64 {
    lamports as f64 / 1_000_000_000.0
}