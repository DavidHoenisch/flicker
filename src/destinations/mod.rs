// Destination implementations for log shipping
//
// DESIGN: Each destination type is a separate module implementing
// the common Destination trait. This allows per-file destination
// configuration and easy addition of new destination types.

pub mod elasticsearch;
pub mod file;
pub mod http;
pub mod s3;
pub mod syslog;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;

/// A log entry to be shipped
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub path: String,
    pub line: String,
    /// When the line was logged: the source's timestamp if it has one, else when
    /// it was read. Not serialized, to keep existing payload formats unchanged.
    #[serde(skip)]
    pub timestamp: DateTime<Utc>,
    /// Unique per entry and unchanged across retries, so destinations can make
    /// resends idempotent (e.g. Elasticsearch `_id`).
    #[serde(skip)]
    pub id: String,
}

impl LogEntry {
    /// Entry stamped with the current time and a random id
    #[allow(dead_code)]
    pub fn new(path: impl Into<String>, line: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line: line.into(),
            timestamp: Utc::now(),
            id: uuid::Uuid::new_v4().simple().to_string(),
        }
    }
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

/// Build a reqwest client with the destination's auth, TLS/mTLS and timeouts.
/// Shared by the HTTP and Elasticsearch destinations. `kind` is used in messages.
pub(crate) fn build_http_client(
    config: &crate::config::DestinationConfig,
    kind: &str,
) -> Result<reqwest::Client> {
    use anyhow::Context;
    use base64::prelude::*;
    use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
    use std::fs;

    let require_auth = config.require_auth.unwrap_or(false);
    if require_auth && !config.has_auth() {
        anyhow::bail!(
            "{} destination requires auth, but no API key or basic auth was provided",
            kind
        );
    }

    let mut headers = HeaderMap::new();
    if let Some(api_key) = &config.api_key {
        let mut auth_value = HeaderValue::from_str(&format!("Bearer {}", api_key))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);
    } else if let Some(basic) = &config.basic {
        let auth_string = format!("{}:{}", basic.username, basic.password);
        let mut auth_value =
            HeaderValue::from_str(&format!("Basic {}", BASE64_STANDARD.encode(auth_string)))?;
        auth_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_value);
    }

    let mut client_builder = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(config.request_timeout())
        .connect_timeout(config.connect_timeout());

    // Configure TLS/mTLS if specified
    if let Some(tls_config) = &config.tls {
        // Read client certificate and key
        let cert_pem = fs::read(&tls_config.cert_path).with_context(|| {
            format!("Failed to read certificate file: {}", tls_config.cert_path)
        })?;
        let key_pem = fs::read(&tls_config.key_path)
            .with_context(|| format!("Failed to read key file: {}", tls_config.key_path))?;

        // Create identity for client certificate authentication
        // native-tls requires separate cert and key PEM files
        let identity = reqwest::Identity::from_pkcs8_pem(&cert_pem, &key_pem)
            .context("Failed to create identity from certificate and key")?;
        client_builder = client_builder.identity(identity);

        // Add custom CA certificate if provided
        if let Some(ca_path) = &tls_config.ca_cert_path {
            let ca_cert = fs::read(ca_path)
                .with_context(|| format!("Failed to read CA certificate file: {}", ca_path))?;
            let cert = reqwest::Certificate::from_pem(&ca_cert)
                .context("Failed to parse CA certificate")?;
            client_builder = client_builder.add_root_certificate(cert);
        }

        // Optionally disable certificate verification (dangerous, but sometimes needed for testing)
        if tls_config.accept_invalid_certs.unwrap_or(false) {
            eprintln!(
                "WARNING: {} destination has tls.accept_invalid_certs enabled. Server certificates \
                 are NOT verified; traffic can be intercepted. Use only for testing.",
                kind
            );
            client_builder = client_builder.danger_accept_invalid_certs(true);
        }

        println!(
            "[{}] Configured mTLS with client certificate from {}",
            kind.to_uppercase(),
            tls_config.cert_path
        );
    }

    Ok(client_builder.build()?)
}

/// Factory function to create destinations from config
pub fn create_destination(
    config: &crate::config::DestinationConfig,
) -> Result<Box<dyn Destination>> {
    match config.dest_type.as_str() {
        "http" => {
            let dest = http::HttpDestination::new(config)?;
            Ok(Box::new(dest))
        }
        "syslog" => {
            let host = config
                .host
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Syslog destination requires 'host' field"))?;
            let port = config.port.unwrap_or(514);
            let protocol = config.protocol.as_deref().unwrap_or("udp");
            Ok(Box::new(syslog::SyslogDestination::new(
                host,
                port,
                protocol,
                config.framing.as_deref(),
                config.request_timeout(),
                config.connect_timeout(),
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
                url, index, config,
            )?))
        }
        "file" => {
            let path = config
                .path
                .clone()
                .ok_or_else(|| anyhow::anyhow!("File destination requires 'path' field"))?;
            Ok(Box::new(file::FileDestination::new(path)?))
        }
        "s3" => Ok(Box::new(s3::S3Destination::new(config)?)),
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
            require_auth: Some(false),
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
            bucket: None,
            prefix: None,
            region: None,
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_http_destination_missing_endpoint() {
        let config = DestinationConfig {
            dest_type: "http".to_string(),
            endpoint: None,
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
            bucket: None,
            prefix: None,
            region: None,
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
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
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: Some("localhost".to_string()),
            port: Some(514),
            protocol: Some("udp".to_string()),
            url: None,
            index: None,
            path: None,
            bucket: None,
            prefix: None,
            region: None,
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_syslog_destination_defaults() {
        let config = DestinationConfig {
            dest_type: "syslog".to_string(),
            endpoint: None,
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: Some("syslog.local".to_string()),
            port: None,     // Should default to 514
            protocol: None, // Should default to "udp"
            url: None,
            index: None,
            path: None,
            bucket: None,
            prefix: None,
            region: None,
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_elasticsearch_destination() {
        let config = DestinationConfig {
            dest_type: "elasticsearch".to_string(),
            endpoint: None,
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: None,
            port: None,
            protocol: None,
            url: Some("http://es:9200".to_string()),
            index: Some("logs".to_string()),
            path: None,
            bucket: None,
            prefix: None,
            region: None,
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_file_destination() {
        let config = DestinationConfig {
            dest_type: "file".to_string(),
            endpoint: None,
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: Some("/tmp/test-flicker.jsonl".to_string()),
            bucket: None,
            prefix: None,
            region: None,
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_s3_destination() {
        let config = DestinationConfig {
            dest_type: "s3".to_string(),
            endpoint: None,
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
            bucket: Some("my-log-bucket".to_string()),
            prefix: Some("logs/app/".to_string()),
            region: Some("us-west-2".to_string()),
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_s3_destination_missing_bucket() {
        let config = DestinationConfig {
            dest_type: "s3".to_string(),
            endpoint: None,
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
            bucket: None,
            prefix: None,
            region: None,
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("bucket"));
    }

    #[test]
    fn test_create_s3_destination_minimal() {
        let config = DestinationConfig {
            dest_type: "s3".to_string(),
            endpoint: None,
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
            bucket: Some("my-bucket".to_string()),
            prefix: None,
            region: None,
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_s3_destination_with_all_fields() {
        let config = DestinationConfig {
            dest_type: "s3".to_string(),
            endpoint: None,
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
            bucket: Some("my-bucket".to_string()),
            prefix: Some("logs/".to_string()),
            region: Some("us-east-1".to_string()),
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_create_unknown_destination_type() {
        let config = DestinationConfig {
            dest_type: "unknown".to_string(),
            endpoint: None,
            require_auth: None,
            api_key: None,
            basic: None,
            compression: None,
            tls: None,
            host: None,
            port: None,
            protocol: None,
            url: None,
            index: None,
            path: None,
            bucket: None,
            prefix: None,
            region: None,
            timeout_ms: None,
            connect_timeout_ms: None,
            framing: None,
        };

        let result = create_destination(&config);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("Unknown destination type"));
    }
}
