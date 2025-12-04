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

    // DESIGN CHOICE: Regex-based filtering
    // match_on: Only ship lines matching at least one of these patterns (whitelist)
    // exclude_on: Skip lines matching any of these patterns (blacklist)
    // Logic: If match_on is non-empty, line must match. Then check exclude_on.
    #[serde(default)]
    pub match_on: Vec<String>,    // List of regex patterns to match (empty = match all)

    #[serde(default)]
    pub exclude_on: Vec<String>,  // List of regex patterns to exclude (empty = exclude none)
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

    #[cfg(test)]
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Self> {
        let config = serde_yaml::from_str(yaml)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_config() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "http"
      endpoint: "http://localhost:8000/ingest"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files.len(), 1);
        assert_eq!(config.log_files[0].path, "/var/log/test.log");
        assert_eq!(config.log_files[0].polling_frequency_ms, 500);
        assert_eq!(config.log_files[0].buffer_size, 100); // Default
        assert_eq!(config.log_files[0].flush_interval_ms, 30000); // Default
        assert_eq!(config.log_files[0].destination.dest_type, "http");
    }

    #[test]
    fn test_config_with_custom_buffering() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 250
    buffer_size: 50
    flush_interval_ms: 15000
    destination:
      type: "http"
      endpoint: "http://localhost:8000/ingest"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files[0].buffer_size, 50);
        assert_eq!(config.log_files[0].flush_interval_ms, 15000);
    }

    #[test]
    fn test_config_with_filters() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    match_on:
      - "ERROR"
      - "WARN"
    exclude_on:
      - "DEBUG"
    destination:
      type: "http"
      endpoint: "http://localhost:8000/ingest"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files[0].match_on.len(), 2);
        assert_eq!(config.log_files[0].match_on[0], "ERROR");
        assert_eq!(config.log_files[0].match_on[1], "WARN");
        assert_eq!(config.log_files[0].exclude_on.len(), 1);
        assert_eq!(config.log_files[0].exclude_on[0], "DEBUG");
    }

    #[test]
    fn test_http_destination() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "http"
      endpoint: "http://example.com/logs"
      api_key: "secret123"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "http");
        assert_eq!(dest.endpoint.as_ref().unwrap(), "http://example.com/logs");
        assert_eq!(dest.api_key.as_ref().unwrap(), "secret123");
    }

    #[test]
    fn test_syslog_destination() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "syslog"
      host: "syslog.example.com"
      port: 514
      protocol: "tcp"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "syslog");
        assert_eq!(dest.host.as_ref().unwrap(), "syslog.example.com");
        assert_eq!(dest.port.unwrap(), 514);
        assert_eq!(dest.protocol.as_ref().unwrap(), "tcp");
    }

    #[test]
    fn test_elasticsearch_destination() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "elasticsearch"
      url: "http://elasticsearch:9200"
      index: "logs-test"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "elasticsearch");
        assert_eq!(dest.url.as_ref().unwrap(), "http://elasticsearch:9200");
        assert_eq!(dest.index.as_ref().unwrap(), "logs-test");
    }

    #[test]
    fn test_file_destination() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "file"
      path: "/backup/logs.jsonl"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "file");
        assert_eq!(dest.path.as_ref().unwrap(), "/backup/logs.jsonl");
    }

    #[test]
    fn test_multiple_log_files() {
        let yaml = r#"
log_files:
  - path: "/var/log/app1.log"
    polling_frequency_ms: 250
    destination:
      type: "http"
      endpoint: "http://dest1:8000/ingest"

  - path: "/var/log/app2.log"
    polling_frequency_ms: 500
    destination:
      type: "syslog"
      host: "syslog.local"

  - path: "/var/log/app3.log"
    polling_frequency_ms: 1000
    destination:
      type: "elasticsearch"
      url: "http://es:9200"
      index: "logs"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files.len(), 3);
        assert_eq!(config.log_files[0].path, "/var/log/app1.log");
        assert_eq!(config.log_files[1].path, "/var/log/app2.log");
        assert_eq!(config.log_files[2].path, "/var/log/app3.log");
    }

    #[test]
    fn test_invalid_yaml() {
        let yaml = "invalid: yaml: syntax: [[[";
        assert!(Config::from_yaml(yaml).is_err());
    }

    #[test]
    fn test_missing_required_fields() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    # Missing polling_frequency_ms
    destination:
      type: "http"
      endpoint: "http://localhost:8000"
        "#;

        assert!(Config::from_yaml(yaml).is_err());
    }

    #[test]
    fn test_empty_log_files() {
        let yaml = r#"
log_files: []
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files.len(), 0);
    }
}
