// Destination implementations for log shipping
//
// DESIGN: Each destination type is a separate module implementing
// the common Destination trait. This allows per-file destination
// configuration and easy addition of new destination types.

pub mod http;
pub mod syslog;
pub mod elasticsearch;
pub mod file;

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
    async fn send(&self, entry: LogEntry) -> Result<()>;

    /// Send a batch of log entries (more efficient)
    async fn send_batch(&self, entries: Vec<LogEntry>) -> Result<()>;
}

/// Factory function to create destinations from config
pub fn create_destination(config: &crate::config::DestinationConfig) -> Result<Box<dyn Destination>> {
    match config.dest_type.as_str() {
        "http" => {
            let endpoint = config.endpoint.clone()
                .ok_or_else(|| anyhow::anyhow!("HTTP destination requires 'endpoint' field"))?;
            Ok(Box::new(http::HttpDestination::new(endpoint)))
        }
        "syslog" => {
            let host = config.host.clone()
                .ok_or_else(|| anyhow::anyhow!("Syslog destination requires 'host' field"))?;
            let port = config.port.unwrap_or(514);
            let protocol = config.protocol.as_deref().unwrap_or("udp");
            Ok(Box::new(syslog::SyslogDestination::new(host, port, protocol)?))
        }
        "elasticsearch" => {
            let url = config.url.clone()
                .ok_or_else(|| anyhow::anyhow!("Elasticsearch destination requires 'url' field"))?;
            let index = config.index.clone()
                .ok_or_else(|| anyhow::anyhow!("Elasticsearch destination requires 'index' field"))?;
            Ok(Box::new(elasticsearch::ElasticsearchDestination::new(url, index)))
        }
        "file" => {
            let path = config.path.clone()
                .ok_or_else(|| anyhow::anyhow!("File destination requires 'path' field"))?;
            Ok(Box::new(file::FileDestination::new(path)?))
        }
        _ => {
            anyhow::bail!("Unknown destination type: {}", config.dest_type)
        }
    }
}
