// Destination implementations for log shipping
//
// DESIGN: Each destination type is a separate module implementing
// the common Destination trait. This allows per-file destination
// configuration and easy addition of new destination types.

pub mod elasticsearch;
pub mod file;
pub mod http;
pub mod syslog;

use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;

/// A log entry to be shipped
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub path: String,
    pub line: String,
}

/// Common interface for all destination types
#[async_trait]
pub trait Destination: Send + Sync {
    /// Send a single log entry (usually not used, prefer send_batch)
    #[allow(dead_code)]
    async fn send(&self, entry: LogEntry) -> Result<()>;

    /// Send a batch of log entries (more efficient)
    async fn send_batch(&self, entries: Vec<LogEntry>) -> Result<()>;
}

/// Factory function to create destinations from config
pub fn create_destination(
    config: &crate::config::DestinationConfig,
) -> Result<Box<dyn Destination>> {
    match config.dest_type.as_str() {
        "http" => {
            let endpoint = config
                .endpoint
                .clone()
                .ok_or_else(|| anyhow::anyhow!("HTTP destination requires 'endpoint' field"))?;
            Ok(Box::new(http::HttpDestination::new(endpoint)))
        }
        "syslog" => {
            let host = config
                .host
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Syslog destination requires 'host' field"))?;
            let port = config.port.unwrap_or(514);
            let protocol = config.protocol.as_deref().unwrap_or("udp");
            Ok(Box::new(syslog::SyslogDestination::new(
                host, port, protocol,
            )?))
        }
        "elasticsearch" => {
            let url = config
                .url
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Elasticsearch destination requires 'url' field"))?;
            let index = config.index.clone().ok_or_else(|| {
                anyhow::anyhow!("Elasticsearch destination requires 'index' field")
            })?;
            Ok(Box::new(elasticsearch::ElasticsearchDestination::new(
                url, index,
            )))
        }
        "file" => {
            let path = config
                .path
                .clone()
                .ok_or_else(|| anyhow::anyhow!("File destination requires 'path' field"))?;
            Ok(Box::new(file::FileDestination::new(path)?))
        }
        _ => {
            anyhow::bail!("Unknown destination type: {}", config.dest_type)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DestinationConfig;

    #[test]
    fn test_create_http_destination() {
        let config = DestinationConfig {
            dest_type: "http".to_string(),
            endpoint: Some("http://localhost:8000".to_string()),
            api_key: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_http_destination_missing_endpoint() {
        let config = DestinationConfig {
            dest_type: "http".to_string(),
            endpoint: None,
            api_key: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
        };

        let result = create_destination(&config);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("endpoint"));
    }

    #[test]
    fn test_create_syslog_destination() {
        let config = DestinationConfig {
            dest_type: "syslog".to_string(),
            endpoint: None,
            api_key: None,
            host: Some("localhost".to_string()),
            port: Some(514),
            protocol: Some("udp".to_string()),
            url: None,
            index: None,
            path: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_syslog_destination_defaults() {
        let config = DestinationConfig {
            dest_type: "syslog".to_string(),
            endpoint: None,
            api_key: None,
            host: Some("syslog.local".to_string()),
            port: None,     // Should default to 514
            protocol: None, // Should default to "udp"
            url: None,
            index: None,
            path: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_elasticsearch_destination() {
        let config = DestinationConfig {
            dest_type: "elasticsearch".to_string(),
            endpoint: None,
            api_key: None,
            host: None,
            port: None,
            protocol: None,
            url: Some("http://es:9200".to_string()),
            index: Some("logs".to_string()),
            path: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_file_destination() {
        let config = DestinationConfig {
            dest_type: "file".to_string(),
            endpoint: None,
            api_key: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: Some("/tmp/test-flicker.jsonl".to_string()),
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_unknown_destination_type() {
        let config = DestinationConfig {
            dest_type: "unknown".to_string(),
            endpoint: None,
            api_key: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
        };

        let result = create_destination(&config);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("Unknown destination type"));
    }
}
