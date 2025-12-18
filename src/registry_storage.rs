use crate::registry::Registry;
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use std::path::PathBuf;

/// Trait for different registry storage backends
#[async_trait]
pub trait RegistryStorage: Send + Sync {
    /// Load the registry from storage
    async fn load(&self) -> Result<Registry>;

    /// Save the registry to storage
    async fn save(&self, registry: &Registry) -> Result<()>;
}

/// Filesystem-based registry storage
pub struct FileSystemStorage {
    path: PathBuf,
}

impl FileSystemStorage {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[async_trait]
impl RegistryStorage for FileSystemStorage {
    async fn load(&self) -> Result<Registry> {
        // Use tokio's async file operations
        match tokio::fs::read_to_string(&self.path).await {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(registry) => {
                    eprintln!("Loaded registry from {:?}", self.path);
                    Ok(registry)
                }
                Err(e) => {
                    eprintln!(
                        "Failed to parse registry at {:?}: {}. Starting with empty registry.",
                        self.path, e
                    );
                    Ok(Registry::new())
                }
            },
            Err(_) => {
                eprintln!("No existing registry at {:?}, starting fresh", self.path);
                Ok(Registry::new())
            }
        }
    }

    async fn save(&self, registry: &Registry) -> Result<()> {
        let json = serde_json::to_string_pretty(registry)?;

        // Write to temp file first
        let temp_path = self.path.with_extension("tmp");
        tokio::fs::write(&temp_path, json).await?;

        // Atomic rename
        tokio::fs::rename(&temp_path, &self.path).await?;

        Ok(())
    }
}

/// S3-based registry storage
pub struct S3Storage {
    client: aws_sdk_s3::Client,
    bucket: String,
    key: String,
}

impl S3Storage {
    /// Create a new S3 storage backend
    ///
    /// The endpoint URL can be customized via the AWS_ENDPOINT_URL environment variable
    /// to support S3-compatible services like MinIO, Wasabi, DigitalOcean Spaces, etc.
    pub async fn new(bucket: String, key: String) -> Result<Self> {
        // Load AWS configuration from environment
        // This respects:
        // - AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY
        // - AWS_REGION
        // - AWS_ENDPOINT_URL (for S3-compatible services)
        // - AWS_PROFILE
        // - EC2 instance metadata (IAM roles)
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;

        let mut s3_config_builder = aws_sdk_s3::config::Builder::from(&config);

        // Check for custom endpoint URL for S3-compatible services
        if let Ok(endpoint) = std::env::var("AWS_ENDPOINT_URL") {
            eprintln!("Using custom S3 endpoint: {}", endpoint);
            s3_config_builder = s3_config_builder.endpoint_url(endpoint);
        }

        // Force path-style addressing for S3-compatible services
        // This is needed for MinIO and other S3-compatible storage
        if std::env::var("AWS_S3_FORCE_PATH_STYLE").is_ok() {
            s3_config_builder = s3_config_builder.force_path_style(true);
        }

        let s3_config = s3_config_builder.build();
        let client = aws_sdk_s3::Client::from_conf(s3_config);

        Ok(Self {
            client,
            bucket,
            key,
        })
    }

    /// Parse an s3:// URL into bucket and key components
    /// Format: s3://bucket-name/key/path/to/file.json
    pub fn parse_s3_url(url: &str) -> Result<(String, String)> {
        if !url.starts_with("s3://") {
            return Err(anyhow!("URL must start with s3://"));
        }

        let without_prefix = &url[5..]; // Remove "s3://"

        let parts: Vec<&str> = without_prefix.splitn(2, '/').collect();

        if parts.len() != 2 {
            return Err(anyhow!(
                "Invalid S3 URL format. Expected: s3://bucket/key, got: {}",
                url
            ));
        }

        let bucket = parts[0].to_string();
        let key = parts[1].to_string();

        if bucket.is_empty() {
            return Err(anyhow!("Bucket name cannot be empty"));
        }

        if key.is_empty() {
            return Err(anyhow!("Key cannot be empty"));
        }

        Ok((bucket, key))
    }
}

#[async_trait]
impl RegistryStorage for S3Storage {
    async fn load(&self) -> Result<Registry> {
        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .send()
            .await
        {
            Ok(output) => {
                let bytes = output
                    .body
                    .collect()
                    .await
                    .context("Failed to read S3 object body")?
                    .into_bytes();

                let contents =
                    String::from_utf8(bytes.to_vec()).context("S3 object is not valid UTF-8")?;

                match serde_json::from_str(&contents) {
                    Ok(registry) => {
                        eprintln!("Loaded registry from s3://{}/{}", self.bucket, self.key);
                        Ok(registry)
                    }
                    Err(e) => {
                        eprintln!(
                            "Failed to parse registry from s3://{}/{}: {}. Starting with empty registry.",
                            self.bucket, self.key, e
                        );
                        Ok(Registry::new())
                    }
                }
            }
            Err(e) => {
                // Check if it's a 404 (object doesn't exist)
                let err_msg = format!("{:?}", e);
                if err_msg.contains("NoSuchKey") || err_msg.contains("404") {
                    eprintln!(
                        "No existing registry at s3://{}/{}, starting fresh",
                        self.bucket, self.key
                    );
                    Ok(Registry::new())
                } else {
                    Err(anyhow!("Failed to load registry from S3: {}", e))
                }
            }
        }
    }

    async fn save(&self, registry: &Registry) -> Result<()> {
        let json = serde_json::to_string_pretty(registry)?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .body(json.into_bytes().into())
            .send()
            .await
            .context("Failed to save registry to S3")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_s3_url_valid() {
        let (bucket, key) =
            S3Storage::parse_s3_url("s3://my-bucket/path/to/registry.json").unwrap();
        assert_eq!(bucket, "my-bucket");
        assert_eq!(key, "path/to/registry.json");
    }

    #[test]
    fn test_parse_s3_url_simple() {
        let (bucket, key) = S3Storage::parse_s3_url("s3://bucket/key.json").unwrap();
        assert_eq!(bucket, "bucket");
        assert_eq!(key, "key.json");
    }

    #[test]
    fn test_parse_s3_url_nested_path() {
        let (bucket, key) = S3Storage::parse_s3_url("s3://my-bucket/a/b/c/registry.json").unwrap();
        assert_eq!(bucket, "my-bucket");
        assert_eq!(key, "a/b/c/registry.json");
    }

    #[test]
    fn test_parse_s3_url_invalid_no_prefix() {
        let result = S3Storage::parse_s3_url("https://bucket/key");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_s3_url_invalid_no_key() {
        let result = S3Storage::parse_s3_url("s3://bucket");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_s3_url_invalid_empty_bucket() {
        let result = S3Storage::parse_s3_url("s3:///key");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_s3_url_invalid_empty_key() {
        let result = S3Storage::parse_s3_url("s3://bucket/");
        assert!(result.is_err());
    }
}
