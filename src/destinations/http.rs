// HTTP destination - sends logs via HTTP POST with JSON payload
//
// DESIGN: Generic HTTP destination that works with Vector, custom
// HTTP endpoints, or any service accepting JSON log arrays.

use super::{Destination, LogEntry};
use anyhow::Result;
use async_trait::async_trait;

pub struct HttpDestination {
    client: reqwest::Client,
    endpoint: String,
}

impl HttpDestination {
    pub fn new(endpoint: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
        }
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
            "[HTTP] Sending batch of {} entries to {}",
            entries.len(),
            self.endpoint
        );

        // Send HTTP POST with JSON array payload
        let response = self
            .client
            .post(&self.endpoint)
            .json(&entries)
            .send()
            .await?;

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
