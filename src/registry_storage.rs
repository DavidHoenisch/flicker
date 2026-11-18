use crate::registry::Registry;
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

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
    /// Held for the life of the process so two instances can't share a registry
    _lock: File,
}

impl FileSystemStorage {
    /// Open the registry at `path`, taking an exclusive lock on `<path>.lock`.
    /// DESIGN CHOICE: An OS file lock (not a marker file) is released automatically
    /// if the process dies, so a crash never leaves a stale lock behind.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let lock_path = sibling_path(&path, ".lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("Failed to open registry lock {:?}", lock_path))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(anyhow!(
                    "Registry {:?} is in use by another flicker instance (lock {:?})",
                    path,
                    lock_path
                ));
            }
            Err(TryLockError::Error(e)) => {
                return Err(anyhow!("Failed to lock registry {:?}: {}", lock_path, e));
            }
        }
        Ok(Self { path, _lock: lock })
    }
}

/// `<path><suffix>`, e.g. registry.json -> registry.json.lock
fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[async_trait]
impl RegistryStorage for FileSystemStorage {
    async fn load(&self) -> Result<Registry> {
        let contents = match tokio::fs::read_to_string(&self.path).await {
            Ok(contents) => contents,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("No existing registry at {:?}, starting fresh", self.path);
                return Ok(Registry::new());
            }
            Err(e) => return Err(anyhow!("Failed to read registry {:?}: {}", self.path, e)),
        };

        match serde_json::from_str(&contents) {
            Ok(registry) => {
                eprintln!("Loaded registry from {:?}", self.path);
                Ok(registry)
            }
            Err(e) => {
                // DESIGN CHOICE: Don't silently start empty (sources would skip to
                // the end of their files) or overwrite the evidence. Move the file
                // aside and stop; restarting then begins fresh, deliberately.
                let aside = sibling_path(
                    &self.path,
                    &format!(".corrupt-{}", chrono::Utc::now().timestamp()),
                );
                let moved = tokio::fs::rename(&self.path, &aside).await;
                Err(anyhow!(
                    "Registry {:?} is corrupt ({}). {}. Restart to begin with an empty registry.",
                    self.path,
                    e,
                    match moved {
                        Ok(()) => format!("Moved it to {:?}", aside),
                        Err(re) => format!("Could not move it aside: {}", re),
                    }
                ))
            }
        }
    }

    async fn save(&self, registry: &Registry) -> Result<()> {
        let json = serde_json::to_string_pretty(registry)?;
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || write_durably(&path, json.as_bytes())).await?
    }
}

/// Atomically replace `path` with `contents`, surviving power loss.
fn write_durably(path: &Path, contents: &[u8]) -> Result<()> {
    // Per-process temp name, so concurrent writers can't clobber each other's temp file
    let temp_path = sibling_path(path, &format!(".tmp.{}", std::process::id()));
    {
        let mut file = File::create(&temp_path)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    std::fs::rename(&temp_path, path)?;

    // Persist the rename itself (directory entry). Not possible on Windows.
    #[cfg(unix)]
    {
        let parent = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        File::open(parent)?.sync_all()?;
    }
    Ok(())
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
                    // Fail rather than start empty and overwrite it on the next save
                    Err(e) => Err(anyhow!(
                        "Registry s3://{}/{} is corrupt ({}). Fix or delete it and restart.",
                        self.bucket,
                        self.key,
                        e
                    )),
                }
            }
            Err(e) if e.as_service_error().is_some_and(|se| se.is_no_such_key()) => {
                eprintln!(
                    "No existing registry at s3://{}/{}, starting fresh",
                    self.bucket, self.key
                );
                Ok(Registry::new())
            }
            Err(e) => Err(anyhow!(
                "Failed to load registry from S3: {}",
                aws_sdk_s3::error::DisplayErrorContext(e)
            )),
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

    #[tokio::test]
    async fn test_fs_storage_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let storage = FileSystemStorage::new(&path).unwrap();

        let mut registry = storage.load().await.unwrap();
        assert!(registry.files.is_empty());
        registry.update_file("/var/log/a.log".to_string(), 42, 7);
        storage.save(&registry).await.unwrap();

        let loaded = storage.load().await.unwrap();
        assert_eq!(
            loaded.get_file_position("/var/log/a.log").unwrap().position,
            42
        );
        // No temp files left behind
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(names.iter().all(|n| !n.contains(".tmp")), "{:?}", names);
    }

    #[tokio::test]
    async fn test_fs_storage_corrupt_registry_fails_and_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        std::fs::write(&path, "{ not json").unwrap();
        let storage = FileSystemStorage::new(&path).unwrap();

        let err = storage.load().await.unwrap_err().to_string();
        assert!(err.contains("corrupt"), "{}", err);
        assert!(!path.exists());
        let moved = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .find(|n| n.starts_with("registry.json.corrupt-"))
            .expect("corrupt registry moved aside");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(moved)).unwrap(),
            "{ not json"
        );
    }

    #[test]
    fn test_fs_storage_lock_prevents_second_instance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let first = FileSystemStorage::new(&path).unwrap();
        let err = FileSystemStorage::new(&path).err().unwrap().to_string();
        assert!(err.contains("another flicker instance"), "{}", err);

        drop(first);
        assert!(FileSystemStorage::new(&path).is_ok());
    }
}
