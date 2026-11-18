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

use super::{Destination, LogEntry, build_http_client};
use crate::config::DestinationConfig;
use anyhow::Result;
use async_trait::async_trait;
use serde::Serialize;

pub struct ElasticsearchDestination {
    client: reqwest::Client,
    url: String,
    index: String,
}

#[derive(Serialize)]
struct BulkIndexAction<'a> {
    index: BulkIndexTarget<'a>,
}

#[derive(Serialize)]
struct BulkIndexTarget<'a> {
    _index: &'a str,
    // DESIGN CHOICE: A fixed _id per entry makes resending a batch idempotent.
    // ES reports bulk failures per item but we retry whole batches; without an
    // _id the items that had already succeeded would be indexed twice.
    _id: &'a str,
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
    pub fn new(url: String, index: String, config: &DestinationConfig) -> Result<Self> {
        // Same auth (api_key/basic), TLS and timeout handling as the HTTP destination
        let client = build_http_client(config, "Elasticsearch")?;
        Ok(Self {
            client,
            url: url.trim_end_matches('/').to_string(),
            index,
        })
    }

    /// Build Elasticsearch bulk API request body (NDJSON format)
    /// Each document requires two lines:
    /// 1. Action metadata ({"index":{"_index":"..."}})
    /// 2. Document data (the actual log entry)
    fn build_bulk_body(&self, entries: &[LogEntry]) -> Result<String> {
        let mut body = String::new();

        for entry in entries {
            // Line 1: Index action
            let action = BulkIndexAction {
                index: BulkIndexTarget {
                    _index: &self.index,
                    _id: &entry.id,
                },
            };
            body.push_str(&serde_json::to_string(&action)?);
            body.push('\n');

            // Line 2: Document (timestamped with when the line was logged, not shipped)
            let doc = ElasticsearchDocument {
                timestamp: entry.timestamp.to_rfc3339(),
                path: entry.path.clone(),
                message: entry.line.clone(),
            };
            body.push_str(&serde_json::to_string(&doc)?);
            body.push('\n');
        }

        Ok(body)
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
        let body = self.build_bulk_body(&entries)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn dest() -> ElasticsearchDestination {
        let config: DestinationConfig =
            serde_yaml::from_str("type: elasticsearch\nurl: http://localhost:9200\nindex: logs\n")
                .unwrap();
        ElasticsearchDestination::new("http://localhost:9200".into(), "logs".into(), &config)
            .unwrap()
    }

    #[test]
    fn test_bulk_body_uses_entry_id_and_timestamp() {
        let ts: DateTime<Utc> = "2024-01-02T03:04:05Z".parse().unwrap();
        let entry = LogEntry {
            path: "/var/log/app.log".into(),
            line: "hello".into(),
            timestamp: ts,
            id: "abc-1".into(),
        };
        let body = dest().build_bulk_body(&[entry.clone(), entry]).unwrap();
        let lines: Vec<serde_json::Value> = body
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0]["index"]["_id"], "abc-1");
        assert_eq!(lines[0]["index"]["_index"], "logs");
        assert_eq!(lines[1]["@timestamp"], ts.to_rfc3339());
        assert_eq!(lines[1]["message"], "hello");
        // Resending the same entry produces the same _id (idempotent)
        assert_eq!(lines[2]["index"]["_id"], "abc-1");
    }

    #[tokio::test]
    async fn test_sends_configured_auth() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap();
            let body = r#"{"errors":false,"items":[]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });

        let config: DestinationConfig = serde_yaml::from_str(&format!(
            "type: elasticsearch\nurl: http://{}\nindex: logs\napi_key: secret123\n",
            addr
        ))
        .unwrap();
        let es = ElasticsearchDestination::new(format!("http://{}", addr), "logs".into(), &config)
            .unwrap();
        es.send_batch(vec![LogEntry::new("/a.log", "x")])
            .await
            .unwrap();

        let request = server.await.unwrap().to_lowercase();
        assert!(
            request.contains("authorization: bearer secret123"),
            "{}",
            request
        );
    }
}
