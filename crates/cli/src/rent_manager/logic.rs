use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use ratatui::style::Color;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use kora_lib::{error::KoraError, signer::pool::SignerPool};

use super::config::{GRACE_PERIOD_SECONDS, HIGH_RENT_THRESHOLD_SOL, HEARTBEAT_INTERVAL_SECS};
use super::types::{UiEvent, OperationMode, ReclaimReason, AuditRecord};
use super::state::{GracePeriodTracker, AppState};
use super::tui::ui;
use super::utils::{
    fetch_all_token_accounts, close_account, get_allowed_tokens,
    lamports_to_sol, log_to_audit_trail, send_telegram_alert
};
use crate::log_output; // Import the macro

pub async fn run_tui_task(
    rpc_client: Arc<RpcClient>,
    signer_pool: Arc<SignerPool>, 
    mode: OperationMode,
) -> Result<(), KoraError> {
    enable_raw_mode().unwrap();
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).unwrap();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).unwrap();

    let (tx, mut rx) = mpsc::unbounded_channel();
    
    let rpc_bg = rpc_client.clone();
    let pool_bg = signer_pool.clone();
    
    tokio::spawn(async move {
        let mut tracker = GracePeriodTracker::load();

        match mode {
            OperationMode::Scan { all } => {
                let _ = tx.send(UiEvent::Status("🔍 Scanning...".to_string()));
                if let Err(e) = scan_accounts(rpc_bg, &pool_bg, all, &mut tracker, Some(tx.clone())).await {
                    let _ = tx.send(UiEvent::Log("System".to_string(), format!("Error: {}", e), Color::Red));
                }
                let _ = tx.send(UiEvent::Status("✅ Scan Complete. Press 'q' to quit.".to_string()));
                let _ = tx.send(UiEvent::TaskComplete);
            },
            OperationMode::Reclaim { execute, force_all } => {
                let mode_str = if execute { "RECLAIMING" } else { "DRY RUN" };
                let _ = tx.send(UiEvent::Status(format!("⚡ {}...", mode_str)));
                
                // Manual reclaim is verbose (show_skipped = true)
                if let Err(e) = reclaim_rent(rpc_bg, &pool_bg, execute, force_all, true, &mut tracker, Some(tx.clone())).await {
                    let _ = tx.send(UiEvent::Log("System".to_string(), format!("Error: {}", e), Color::Red));
                }
                tracker.save();
                let _ = tx.send(UiEvent::Status("✅ Task Complete. Press 'q' to quit.".to_string()));
                let _ = tx.send(UiEvent::TaskComplete);
            },
            OperationMode::Daemon { interval } => {
                let cycle_duration = match humantime::parse_duration(&interval) {
                    Ok(d) => d,
                    Err(_) => Duration::from_secs(3600),
                };

                let mut last_report_time = Instant::now();
                let report_interval = Duration::from_secs(HEARTBEAT_INTERVAL_SECS);

                loop {
                    let _ = tx.send(UiEvent::Status("🚀 Daemon Cycle Starting...".to_string()));
                    let mut daemon_tracker = GracePeriodTracker::load();
                    
                    // Daemon is quiet (show_skipped = false) and safe (execute = false)
                    match reclaim_rent(rpc_bg.clone(), &pool_bg, false, false, false, &mut daemon_tracker, Some(tx.clone())).await {
                        Ok(_) => {
                            daemon_tracker.save();
                        },
                        Err(e) => {
                            let _ = tx.send(UiEvent::Log("System".to_string(), format!("⚠️ Job Failed: {}", e), Color::Red));
                        }
                    }

                    if last_report_time.elapsed() >= report_interval {
                        let msg = "📊 *Kora Rent Manager Heartbeat*\n\n✅ System is active and monitoring accounts.\nWaiting for next cycle.";
                        tokio::spawn(async move { send_telegram_alert(msg).await; });
                        let _ = tx.send(UiEvent::Log("System".to_string(), "❤️ Sending Heartbeat Report to Telegram".to_string(), Color::Cyan));
                        last_report_time = Instant::now();
                    }

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

    let mut app = AppState::default();

    loop {
        terminal.draw(|f| ui(f, &app)).unwrap();

        if let Ok(event) = rx.try_recv() {
            match event {
                UiEvent::Log(acc, details, color) => {
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
                    app.is_high_rent = is_active;
                    app.current_locked_rent = amount;
                }
            }
        }

        if app.is_working {
            app.spinner_idx = (app.spinner_idx + 1) % 4;
        }

        if event::poll(Duration::from_millis(100)).unwrap() {
            if let Event::Key(key) = event::read().unwrap() {
                if key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }
    }

    disable_raw_mode().unwrap();
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture).unwrap();
    terminal.show_cursor().unwrap();

    Ok(())
}

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

async fn reclaim_rent(
    rpc_client: Arc<RpcClient>,
    signer_pool: &SignerPool,
    execute: bool,
    force_all: bool,
    show_skipped: bool,
    tracker: &mut GracePeriodTracker,
    tx: Option<mpsc::UnboundedSender<UiEvent>>,
) -> Result<(), KoraError> {
    let signers_info = signer_pool.get_signers_info();
    let (allowed_tokens, is_all_allowed) = get_allowed_tokens()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let mut reclaimed_rent = 0;
    let mut reclaimed_count = 0;
    let mut locked_rent_accumulated = 0; 

    for signer_info in signers_info {
        let signer_pubkey = signer_info.public_key.parse::<Pubkey>().unwrap();
        let signer = signer_pool.get_signer_by_pubkey(&signer_info.public_key)?;

        if tx.is_some() && show_skipped {
             log_output!(&tx, "Processing".to_string(), signer_info.name.clone(), Color::White);
        }
        
        let accounts = fetch_all_token_accounts(&rpc_client, &signer_pubkey).await?;

        for acc in accounts {
            let pubkey_str = acc.pubkey.to_string();

            if acc.amount == 0 {
                locked_rent_accumulated += acc.lamports;
            }

            if acc.amount != 0 { 
                if !execute && show_skipped {
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

            let should_close = if execute {
                if force_all { true } else { !is_allowed && is_safe_time }
            } else {
                !is_allowed && is_safe_time
            };
            
            let final_reason = if force_all && execute { ReclaimReason::ForceClosed } else { reason };

            let rent_in_sol = lamports_to_sol(acc.lamports);

            if should_close {
                let action_label = if execute { "CLOSING" } else { "RECLAIMABLE" };
                let color = if execute { Color::Magenta } else { Color::Green };

                let details = format!(
                    "[{}] Mint: {} | Rent: {:.4} SOL | Bal: {}",
                    action_label, acc.mint, rent_in_sol, acc.amount
                );
                log_output!(&tx, acc.pubkey.to_string(), details, color);

                if execute {
                    match close_account(&rpc_client, &signer, &acc, &signer_pubkey).await {
                        Ok(sig) => {
                            log_output!(&tx, acc.pubkey.to_string(), format!("[CLOSED] Sig: {}", sig), Color::Green);
                            reclaimed_rent += acc.lamports;
                            reclaimed_count += 1;
                            
                            if let Some(ref t) = tx {
                                let _ = t.send(UiEvent::StatsUpdate { reclaimed: rent_in_sol, count: 1 });
                            }

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
                if !execute && show_skipped {
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

    let current_locked_sol = lamports_to_sol(locked_rent_accumulated);
    if let Some(ref t) = tx {
        if current_locked_sol > HIGH_RENT_THRESHOLD_SOL {
            let _ = t.send(UiEvent::Alert(true, current_locked_sol));
            let _ = t.send(UiEvent::Log("ALERT".to_string(), format!("High Rent Idle: {:.2} SOL", current_locked_sol), Color::Red));
            
            let msg = format!("🚨 *High Idle Rent Detected!*\n\nAmount: `{:.2} SOL`\nThreshold: `{:.2} SOL`", current_locked_sol, HIGH_RENT_THRESHOLD_SOL);
            tokio::spawn(async move { send_telegram_alert(&msg).await; });
        } else {
            let _ = t.send(UiEvent::Alert(false, current_locked_sol));
        }
    }

    if reclaimed_count == 0 {
        if show_skipped {
            log_output!(&tx, "SUMMARY".to_string(), "No accounts found eligible for reclaim.".to_string(), Color::Yellow);
        }
    } else {
        let label = if execute { "RECLAIMED" } else { "FOUND RECLAIMABLE" };
        let color = if execute { Color::Green } else { Color::LightGreen };
        log_output!(&tx, label.to_string(), format!("{} Accts ({:.4} SOL)", reclaimed_count, lamports_to_sol(reclaimed_rent)), color);
        
        if execute {
             let msg = format!("✅ *Kora Reclaim Success*\n\nClosed: {}\nRecovered: `{:.4} SOL`", reclaimed_count, lamports_to_sol(reclaimed_rent));
             tokio::spawn(async move { send_telegram_alert(&msg).await; });
        } else {
             let msg = format!("🔎 *Reclaim Opportunity Detected*\n\nFound: `{}` accounts\nRecoverable: `{:.4} SOL`\n\nRun `make reclaim` to secure these funds.", reclaimed_count, lamports_to_sol(reclaimed_rent));
             tokio::spawn(async move { send_telegram_alert(&msg).await; });
        }
    }

    Ok(())
}