// Elasticsearch destination - sends logs via Elasticsearch Bulk API
//
// DESIGN: Uses Elasticsearch's _bulk API for efficient batch indexing.
// Documents are indexed with @timestamp field for time-series queries.
// Compatible with Elasticsearch 7.x and 8.x.
//
// Bulk API format (NDJSON):
// {"index":{"_index":"logs"}}
// {"@timestamp":"2025-12-03T14:23:45Z","path":"/var/log/app.log","message":"..."}
// {"index":{"_index":"logs"}}
// {"@timestamp":"2025-12-03T14:23:46Z","path":"/var/log/app.log","message":"..."}

use super::{Destination, LogEntry};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::Serialize;

pub struct ElasticsearchDestination {
    client: reqwest::Client,
    url: String,
    index: String,
}

#[derive(Serialize)]
struct BulkIndexAction {
    index: BulkIndexTarget,
}

#[derive(Serialize)]
struct BulkIndexTarget {
    _index: String,
}

#[derive(Serialize)]
struct ElasticsearchDocument {
    #[serde(rename = "@timestamp")]
    timestamp: String,
    path: String,
    message: String,
    // DESIGN CHOICE: Include source file path as field
    // Allows filtering by log file in Kibana/ES queries
}

impl ElasticsearchDestination {
    pub fn new(url: String, index: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: url.trim_end_matches('/').to_string(),
            index,
        }
    }

    /// Build Elasticsearch bulk API request body (NDJSON format)
    /// Each document requires two lines:
    /// 1. Action metadata ({"index":{"_index":"..."}})
    /// 2. Document data (the actual log entry)
    fn build_bulk_body(&self, entries: &[LogEntry]) -> String {
        let mut body = String::new();

        for entry in entries {
            // Line 1: Index action
            let action = BulkIndexAction {
                index: BulkIndexTarget {
                    _index: self.index.clone(),
                },
            };
            body.push_str(&serde_json::to_string(&action).unwrap());
            body.push('\n');

            // Line 2: Document
            let doc = ElasticsearchDocument {
                timestamp: Utc::now().to_rfc3339(),
                path: entry.path.clone(),
                message: entry.line.clone(),
            };
            body.push_str(&serde_json::to_string(&doc).unwrap());
            body.push('\n');
        }

        body
    }
}

#[async_trait]
impl Destination for ElasticsearchDestination {
    async fn send(&self, entry: LogEntry) -> Result<()> {
        self.send_batch(vec![entry]).await
    }

    async fn send_batch(&self, entries: Vec<LogEntry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        println!(
            "[ELASTICSEARCH] Sending batch of {} entries to {} (index: {})",
            entries.len(),
            self.url,
            self.index
        );

        // Build NDJSON bulk request body
        let body = self.build_bulk_body(&entries);

        // Send to Elasticsearch _bulk API
        let bulk_url = format!("{}/_bulk", self.url);
        let response = self
            .client
            .post(&bulk_url)
            .header("Content-Type", "application/x-ndjson")
            .body(body)
            .send()
            .await?;

        // Check HTTP status
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            anyhow::bail!("Elasticsearch HTTP {}: {}", status, body);
        }

        // Parse bulk response to check for errors
        // DESIGN CHOICE: Check bulk response for individual item errors
        // ES bulk API returns 200 even if some items failed, so we must
        // check the response body for errors
        let response_text = response.text().await?;
        let response_json: serde_json::Value = serde_json::from_str(&response_text)?;

        if let Some(errors) = response_json.get("errors")
            && errors.as_bool() == Some(true)
        {
            // Some items failed - log details
            eprintln!("[ELASTICSEARCH] Bulk request had errors: {}", response_text);
            anyhow::bail!("Elasticsearch bulk request contained errors");
        }

        println!("[ELASTICSEARCH] Batch indexed successfully");

        Ok(())
    }
}
