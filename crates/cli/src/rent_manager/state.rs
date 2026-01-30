use std::collections::HashMap;
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use ratatui::style::Color;
use super::config::TRACKER_FILE;

#[derive(Serialize, Deserialize, Default)]
pub struct GracePeriodTracker {
    pub pending_closures: HashMap<String, u64>,
}

impl GracePeriodTracker {
    pub fn load() -> Self {
        if Path::new(TRACKER_FILE).exists() {
            let data = fs::read_to_string(TRACKER_FILE).unwrap_or_default();
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) {
        let json = serde_json::to_string_pretty(&self).unwrap();
        let _ = fs::write(TRACKER_FILE, json); 
    }
}

pub struct AppState {
    pub logs: Vec<(String, String, Color)>, 
    pub total_reclaimed_sol: f64,
    pub reclaimed_count: u64,
    pub status_msg: String,
    pub spinner_idx: usize,
    pub is_working: bool,
    pub is_high_rent: bool,       
    pub current_locked_rent: f64, 
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            logs: vec![],
            total_reclaimed_sol: 0.0,
            reclaimed_count: 0,
            status_msg: "Initializing...".to_string(),
            spinner_idx: 0,
            is_working: true,
            is_high_rent: false, 
            current_locked_rent: 0.0,
        }
    }
}