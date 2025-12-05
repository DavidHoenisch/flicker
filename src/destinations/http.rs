// HTTP destination - sends logs via HTTP POST with JSON payload
//
// DESIGN: Generic HTTP destination that works with Vector, custom
// HTTP endpoints, or any service accepting JSON log arrays.

use super::{Destination, LogEntry};
use crate::config::DestinationConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use base64::prelude::*;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

pub struct HttpDestination {
    client: reqwest::Client,
    endpoint: String,
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

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self { client, endpoint })
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
