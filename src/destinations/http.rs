// HTTP destination - sends logs via HTTP POST with JSON payload
//
// DESIGN: Generic HTTP destination that works with Vector, custom
// HTTP endpoints, or any service accepting JSON log arrays.

use super::{Destination, LogEntry};
use crate::config::DestinationConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::prelude::*;
use flate2::Compression;
use flate2::write::GzEncoder;
use reqwest::header::{AUTHORIZATION, CONTENT_ENCODING, HeaderMap, HeaderValue};
use std::fs;
use std::io::Write;

pub struct HttpDestination {
    client: reqwest::Client,
    endpoint: String,
    compression: bool,
}

impl HttpDestination {
    pub fn new(config: &DestinationConfig) -> Result<Self> {
        let endpoint = config
            .endpoint
            .clone()
            .context("HTTP destination requires an endpoint")?;

        let require_auth = config.require_auth.unwrap_or(false);
        if require_auth && !config.has_auth() {
            anyhow::bail!(
                "HTTP destination requires auth, but no API key or basic auth was provided"
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

        let mut client_builder = reqwest::Client::builder().default_headers(headers);

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
                client_builder = client_builder.danger_accept_invalid_certs(true);
            }

            println!(
                "[HTTP] Configured mTLS with client certificate from {}",
                tls_config.cert_path
            );
        }

        let client = client_builder.build()?;
        let compression = config.compression.unwrap_or(false);

        Ok(Self {
            client,
            endpoint,
            compression,
        })
    }
}

#[async_trait]
impl Destination for HttpDestination {
    async fn send(&self, entry: LogEntry) -> Result<()> {
        // Single entry - wrap in array for consistency
        self.send_batch(vec![entry]).await
    }

    async fn send_batch(&self, entries: Vec<LogEntry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        println!(
            "[HTTP] Sending batch of {} entries to {}{}",
            entries.len(),
            self.endpoint,
            if self.compression {
                " (gzip compressed)"
            } else {
                ""
            }
        );

        // Serialize entries to JSON
        let json_bytes = serde_json::to_vec(&entries)?;

        let response = if self.compression {
            // Compress the JSON payload with gzip
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&json_bytes)?;
            let compressed_bytes = encoder.finish()?;

            println!(
                "[HTTP] Compressed {} bytes to {} bytes ({:.1}% reduction)",
                json_bytes.len(),
                compressed_bytes.len(),
                100.0 - (compressed_bytes.len() as f64 / json_bytes.len() as f64 * 100.0)
            );

            // Send compressed payload with Content-Encoding header
            self.client
                .post(&self.endpoint)
                .header(CONTENT_ENCODING, "gzip")
                .header("Content-Type", "application/json")
                .body(compressed_bytes)
                .send()
                .await?
        } else {
            // Send uncompressed JSON payload
            self.client
                .post(&self.endpoint)
                .json(&entries)
                .send()
                .await?
        };

        // Check for HTTP errors
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            anyhow::bail!("HTTP {} from {}: {}", status, self.endpoint, body);
        }

        println!(
            "[HTTP] Batch sent successfully (HTTP {})",
            response.status()
        );

        Ok(())
    }
}
