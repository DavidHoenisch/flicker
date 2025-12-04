use serde::Deserialize;
use std::fs;

// DESIGN CHOICE: Per-file configuration
// Each log file is an independent unit with its own polling frequency
// and destination. This allows maximum flexibility: different files
// can ship to different destinations at different rates.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub log_files: Vec<LogFileConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LogFileConfig {
    pub path: String,
    pub polling_frequency_ms: u64,
    pub destination: DestinationConfig,

    // DESIGN CHOICE: Dual-trigger buffering
    // Buffer flushes when EITHER condition is met (OR logic):
    // 1. Buffer reaches buffer_size lines
    // 2. flush_interval_ms elapsed since last flush
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    #[serde(default = "default_flush_interval_ms")]
    pub flush_interval_ms: u64,
}

// Default: Flush every 100 lines
fn default_buffer_size() -> usize {
    100
}

// Default: Flush every 30 seconds
fn default_flush_interval_ms() -> u64 {
    30_000
}

// DESIGN CHOICE: Flexible destination config
// Different destination types require different fields.
// We use a `type` field to determine which destination to create,
// and all other fields are optional to support any destination type.
#[derive(Debug, Deserialize, Clone)]
pub struct DestinationConfig {
    #[serde(rename = "type")]
    pub dest_type: String, // "http", "syslog", "elasticsearch", "file"

    // HTTP destination fields
    pub endpoint: Option<String>, // HTTP endpoint URL
    pub api_key: Option<String>,   // Optional API key for auth

    // Syslog destination fields
    pub host: Option<String>,      // Syslog server hostname
    pub port: Option<u16>,         // Syslog server port (default: 514)
    pub protocol: Option<String>,  // "udp" or "tcp" (default: "udp")

    // Elasticsearch destination fields
    pub url: Option<String>,       // Elasticsearch URL
    pub index: Option<String>,     // Index name

    // File destination fields
    pub path: Option<String>,      // Output file path
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path)?; // The '?' operator is "if err != nil { return err }"
        let config = serde_yaml::from_str(&content)?;
        Ok(config)
    }
}
