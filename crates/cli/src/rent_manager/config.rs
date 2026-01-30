// Safety Mechanism: Accounts must be empty for 24 hours before we reclaim them.
pub const GRACE_PERIOD_SECONDS: u64 = 60; // Set to 60s for demo, usually 24*60*60

// File paths
pub const TRACKER_FILE: &str = "grace_period.json";
pub const AUDIT_FILE: &str = "audit_log.csv";

// Thresholds
pub const HIGH_RENT_THRESHOLD_SOL: f64 = 1.0; 
pub const HEARTBEAT_INTERVAL_SECS: u64 = 60;