// S3 destination - uploads logs to Amazon S3 or S3-compatible storage
//
// DESIGN: Batches log entries into JSON Lines format and uploads as
// individual S3 objects with timestamped keys to prevent collisions.
// Supports S3-compatible services like MinIO via AWS_ENDPOINT_URL.
//
// Output format: JSON Lines (JSONL) - one JSON object per line
// Key format: {prefix}logs/YYYY-MM-DD/HH-MM-SS-{uuid}.jsonl
//
// AWS credentials loaded from environment:
// - AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
// - AWS_REGION (optional, defaults to us-east-1)
// - AWS_ENDPOINT_URL (for S3-compatible services)
// - AWS_S3_FORCE_PATH_STYLE (for MinIO compatibility)

use super::{Destination, LogEntry};
use crate::config::DestinationConfig;
use anyhow::Result;
use async_trait::async_trait;
use aws_sdk_s3::Client;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

pub struct S3Destination {
    client: Arc<Mutex<Option<Client>>>,
    bucket: String,
    prefix: String,
    region: Option<String>,
}

impl S3Destination {
    pub fn new(config: &DestinationConfig) -> Result<Self> {
        let bucket = config
            .bucket
            .clone()
            .ok_or_else(|| anyhow::anyhow!("S3 destination requires 'bucket' field"))?;

        let prefix = config.prefix.clone().unwrap_or_default();

        // Ensure prefix ends with '/' if not empty
        let prefix = if !prefix.is_empty() && !prefix.ends_with('/') {
            format!("{}/", prefix)
        } else {
            prefix
        };

        Ok(Self {
            client: Arc::new(Mutex::new(None)),
            bucket,
            prefix,
            region: config.region.clone(),
        })
    }

    /// Lazily initialize the S3 client
    async fn get_client(&self) -> Result<Client> {
        let mut client_guard = self.client.lock().await;
        if let Some(client) = &*client_guard {
            return Ok(client.clone());
        }

        // Initialize client following registry_storage.rs pattern
        let mut config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest());

        // Set region if specified in config
        if let Some(region) = &self.region {
            config_loader = config_loader.region(aws_config::Region::new(region.clone()));
        }

        let config = config_loader.load().await;

        let mut s3_config_builder = aws_sdk_s3::config::Builder::from(&config);

        // Support custom endpoints for S3-compatible services
        if let Ok(endpoint) = std::env::var("AWS_ENDPOINT_URL") {
            println!("[S3] Using custom S3 endpoint: {}", endpoint);
            s3_config_builder = s3_config_builder.endpoint_url(endpoint);
        }

        // Force path-style addressing for MinIO and other S3-compatible services
        if std::env::var("AWS_S3_FORCE_PATH_STYLE").is_ok() {
            s3_config_builder = s3_config_builder.force_path_style(true);
        }

        let s3_config = s3_config_builder.build();
        let client = Client::from_conf(s3_config);

        *client_guard = Some(client.clone());
        Ok(client)
    }

    /// Generate a unique S3 object key with timestamp and UUID
    fn generate_key(&self) -> String {
        let now = Utc::now();
        let date = now.format("%Y-%m-%d");
        let time = now.format("%H-%M-%S");
        let uuid = Uuid::new_v4().simple();
        format!("{}logs/{}/{}-{}.jsonl", self.prefix, date, time, uuid)
    }
}

#[async_trait]
impl Destination for S3Destination {
    async fn send(&self, entry: LogEntry) -> Result<()> {
        self.send_batch(vec![entry]).await
    }

    async fn send_batch(&self, entries: Vec<LogEntry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let client = self.get_client().await?;
        let key = self.generate_key();

        println!(
            "[S3] Uploading batch of {} entries to s3://{}/{}",
            entries.len(),
            self.bucket,
            key
        );

        // Serialize entries to JSONL format
        let jsonl_data: String = entries
            .iter()
            .map(|entry| serde_json::to_string(entry).unwrap())
            .collect::<Vec<_>>()
            .join("\n");

        // Upload to S3
        client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(jsonl_data.into_bytes().into())
            .content_type("application/jsonlines")
            .send()
            .await?;

        println!("[S3] Batch uploaded successfully");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DestinationConfig;
    use regex::Regex;

    #[test]
    fn test_s3_destination_new_with_bucket() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: None,
            region: None,
        };

        let result = S3Destination::new(&config);
        assert!(result.is_ok());
        let dest = result.unwrap();
        assert_eq!(dest.bucket, "test-bucket");
        assert_eq!(dest.prefix, "");
        assert!(dest.region.is_none());
    }

    #[test]
    fn test_s3_destination_new_with_prefix() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: Some("my-prefix".to_string()),
            region: None,
        };

        let result = S3Destination::new(&config);
        assert!(result.is_ok());
        let dest = result.unwrap();
        assert_eq!(dest.bucket, "test-bucket");
        assert_eq!(dest.prefix, "my-prefix/");
        assert!(dest.region.is_none());
    }

    #[test]
    fn test_s3_destination_new_with_prefix_no_trailing_slash() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: Some("my-prefix".to_string()),
            region: None,
        };

        let result = S3Destination::new(&config);
        assert!(result.is_ok());
        let dest = result.unwrap();
        assert_eq!(dest.prefix, "my-prefix/");
    }

    #[test]
    fn test_s3_destination_new_with_prefix_with_trailing_slash() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: Some("my-prefix/".to_string()),
            region: None,
        };

        let result = S3Destination::new(&config);
        assert!(result.is_ok());
        let dest = result.unwrap();
        assert_eq!(dest.prefix, "my-prefix/");
    }

    #[test]
    fn test_s3_destination_new_with_empty_prefix() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: Some("".to_string()),
            region: None,
        };

        let result = S3Destination::new(&config);
        assert!(result.is_ok());
        let dest = result.unwrap();
        assert_eq!(dest.prefix, "");
    }

    #[test]
    fn test_s3_destination_new_with_region() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: None,
            region: Some("us-west-2".to_string()),
        };

        let result = S3Destination::new(&config);
        assert!(result.is_ok());
        let dest = result.unwrap();
        assert_eq!(dest.region, Some("us-west-2".to_string()));
    }

    #[test]
    fn test_s3_destination_new_missing_bucket() {
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
        };

        let result = S3Destination::new(&config);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("bucket"));
    }

    #[test]
    fn test_s3_destination_generate_key_format() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: Some("my-prefix".to_string()),
            region: None,
        };

        let dest = S3Destination::new(&config).unwrap();
        let key = dest.generate_key();

        // Regex to match: my-prefix/logs/YYYY-MM-DD/HH-MM-SS-uuid.jsonl
        let re =
            Regex::new(r"^my-prefix/logs/\d{4}-\d{2}-\d{2}/\d{2}-\d{2}-\d{2}-[0-9a-f]{32}\.jsonl$")
                .unwrap();
        assert!(re.is_match(&key));
    }

    #[test]
    fn test_s3_destination_generate_key_uniqueness() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: None,
            region: None,
        };

        let dest = S3Destination::new(&config).unwrap();
        let key1 = dest.generate_key();
        let key2 = dest.generate_key();
        let key3 = dest.generate_key();

        assert_ne!(key1, key2);
        assert_ne!(key2, key3);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_s3_destination_generate_key_with_empty_prefix() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: Some("".to_string()),
            region: None,
        };

        let dest = S3Destination::new(&config).unwrap();
        let key = dest.generate_key();

        // Regex to match: logs/YYYY-MM-DD/HH-MM-SS-uuid.jsonl
        let re =
            Regex::new(r"^logs/\d{4}-\d{2}-\d{2}/\d{2}-\d{2}-\d{2}-[0-9a-f]{32}\.jsonl$").unwrap();
        assert!(re.is_match(&key));
    }

    #[test]
    fn test_s3_destination_generate_key_with_prefix() {
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
            bucket: Some("test-bucket".to_string()),
            prefix: Some("my-prefix".to_string()),
            region: None,
        };

        let dest = S3Destination::new(&config).unwrap();
        let key = dest.generate_key();

        // Regex to match: my-prefix/logs/YYYY-MM-DD/HH-MM-SS-uuid.jsonl
        let re =
            Regex::new(r"^my-prefix/logs/\d{4}-\d{2}-\d{2}/\d{2}-\d{2}-\d{2}-[0-9a-f]{32}\.jsonl$")
                .unwrap();
        assert!(re.is_match(&key));
    }
}
