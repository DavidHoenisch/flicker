// HTTP destination - sends logs via HTTP POST with JSON payload
//
// DESIGN: Generic HTTP destination that works with Vector, custom
// HTTP endpoints, or any service accepting JSON log arrays.

use super::{Destination, LogEntry, build_http_client};
use crate::config::DestinationConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use flate2::Compression;
use flate2::write::GzEncoder;
use reqwest::header::CONTENT_ENCODING;
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

        let client = build_http_client(config, "HTTP")?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn test_http_destination_times_out_on_hung_endpoint() {
        // Accept connections but never respond
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });

        let config: DestinationConfig = serde_yaml::from_str(&format!(
            "type: http\nendpoint: http://{}/\ntimeout_ms: 200\n",
            addr
        ))
        .unwrap();
        let dest = HttpDestination::new(&config).unwrap();

        let start = Instant::now();
        let result = dest
            .send_batch(vec![LogEntry::new("/test.log", "hello")])
            .await;
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
