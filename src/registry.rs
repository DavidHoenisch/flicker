use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Represents a single file's position in the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePosition {
    pub position: u64,
    pub inode: u64,
    pub last_updated: DateTime<Utc>,
}

/// Represents a single container's position in the registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerPosition {
    pub last_timestamp: DateTime<Utc>,
}

/// The registry that tracks all file and container positions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub files: HashMap<String, FilePosition>,
    pub containers: HashMap<String, ContainerPosition>,
}

impl Registry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            version: 1,
            files: HashMap::new(),
            containers: HashMap::new(),
        }
    }

    /// Load registry from disk, returns empty registry if file doesn't exist or is corrupt
    pub fn load<P: AsRef<Path>>(path: P) -> Self {
        match fs::read_to_string(path.as_ref()) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(registry) => {
                    eprintln!("Loaded registry from {:?}", path.as_ref());
                    registry
                }
                Err(e) => {
                    eprintln!(
                        "Failed to parse registry at {:?}: {}. Starting with empty registry.",
                        path.as_ref(),
                        e
                    );
                    Self::new()
                }
            },
            Err(_) => {
                // File doesn't exist, that's fine - start fresh
                eprintln!(
                    "No existing registry at {:?}, starting fresh",
                    path.as_ref()
                );
                Self::new()
            }
        }
    }

    /// Save registry to disk atomically (write to temp file, then rename)
    pub fn save<P: AsRef<Path>>(&self, path: P) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;

        // Write to temp file first
        let temp_path = path.as_ref().with_extension("tmp");
        fs::write(&temp_path, json)?;

        // Atomic rename
        fs::rename(&temp_path, &path)?;

        Ok(())
    }

    /// Update a file's position in the registry
    pub fn update_file(&mut self, path: String, position: u64, inode: u64) {
        self.files.insert(
            path,
            FilePosition {
                position,
                inode,
                last_updated: Utc::now(),
            },
        );
    }

    /// Update a container's position in the registry
    pub fn update_container(&mut self, container: String, last_timestamp: DateTime<Utc>) {
        self.containers
            .insert(container, ContainerPosition { last_timestamp });
    }

    /// Get a file's position from the registry
    pub fn get_file_position(&self, path: &str) -> Option<FilePosition> {
        self.files.get(path).cloned()
    }

    /// Get a container's position from the registry
    pub fn get_container_position(&self, container: &str) -> Option<ContainerPosition> {
        self.containers.get(container).cloned()
    }
}

/// Messages sent to the registry writer task
#[derive(Debug, Clone)]
pub enum RegistryUpdate {
    UpdateFile {
        path: String,
        position: u64,
        inode: u64,
    },
    UpdateContainer {
        container: String,
        last_timestamp: DateTime<Utc>,
    },
    Shutdown,
}

/// Registry writer task that receives updates via channel and persists to disk
/// This task runs independently and batches writes to avoid excessive disk I/O
pub async fn registry_writer_task(
    registry_path: String,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<RegistryUpdate>,
) {
    use tokio::time::{Duration, interval};

    let mut registry = Registry::load(&registry_path);
    let mut dirty = false;

    // Flush registry to disk every 5 seconds (or on shutdown)
    let mut flush_interval = interval(Duration::from_secs(5));

    loop {
        tokio::select! {
            // Process incoming updates
            Some(update) = rx.recv() => {
                match update {
                    RegistryUpdate::UpdateFile { path, position, inode } => {
                        registry.update_file(path, position, inode);
                        dirty = true;
                    }
                    RegistryUpdate::UpdateContainer { container, last_timestamp } => {
                        registry.update_container(container, last_timestamp);
                        dirty = true;
                    }
                    RegistryUpdate::Shutdown => {
                        // Flush one last time before shutting down
                        if dirty {
                            if let Err(e) = registry.save(&registry_path) {
                                eprintln!("Failed to save registry on shutdown: {}", e);
                            } else {
                                eprintln!("Registry saved to {}", registry_path);
                            }
                        }
                        break;
                    }
                }
            }
            // Periodic flush to disk
            _ = flush_interval.tick() => {
                if dirty {
                    if let Err(e) = registry.save(&registry_path) {
                        eprintln!("Failed to save registry: {}", e);
                    }
                    dirty = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_new_registry() {
        let registry = Registry::new();
        assert_eq!(registry.version, 1);
        assert_eq!(registry.files.len(), 0);
        assert_eq!(registry.containers.len(), 0);
    }

    #[test]
    fn test_update_file() {
        let mut registry = Registry::new();
        registry.update_file("/var/log/app.log".to_string(), 1024, 12345);

        let pos = registry.get_file_position("/var/log/app.log").unwrap();
        assert_eq!(pos.position, 1024);
        assert_eq!(pos.inode, 12345);
    }

    #[test]
    fn test_update_container() {
        let mut registry = Registry::new();
        let timestamp = Utc::now();
        registry.update_container("nginx".to_string(), timestamp);

        let pos = registry.get_container_position("nginx").unwrap();
        assert_eq!(pos.last_timestamp, timestamp);
    }

    #[test]
    fn test_save_and_load() {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();

        // Create and save registry
        let mut registry = Registry::new();
        registry.update_file("/var/log/app.log".to_string(), 2048, 98765);
        registry.update_container("nginx".to_string(), Utc::now());

        registry.save(path).unwrap();

        // Load it back
        let loaded = Registry::load(path);
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.containers.len(), 1);

        let pos = loaded.get_file_position("/var/log/app.log").unwrap();
        assert_eq!(pos.position, 2048);
        assert_eq!(pos.inode, 98765);
    }

    #[test]
    fn test_load_nonexistent_file() {
        let registry = Registry::load("/tmp/does_not_exist_registry_12345.json");
        assert_eq!(registry.files.len(), 0);
        assert_eq!(registry.containers.len(), 0);
    }

    #[test]
    fn test_load_corrupt_file() {
        let mut temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();

        // Write invalid JSON
        use std::io::Write;
        temp_file.write_all(b"{ invalid json }").unwrap();
        temp_file.flush().unwrap();

        let registry = Registry::load(path);
        // Should gracefully fall back to empty registry
        assert_eq!(registry.files.len(), 0);
        assert_eq!(registry.containers.len(), 0);
    }
}
