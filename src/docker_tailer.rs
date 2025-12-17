use crate::registry::RegistryUpdate;
use bollard::Docker;
use bollard::container::LogsOptions;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Tracks state for a single container being tailed
struct ContainerState {
    last_timestamp: Option<DateTime<Utc>>,
}

/// Manages tailing multiple Docker containers
pub struct DockerTailer {
    docker: Docker,
    containers: HashMap<String, ContainerState>,
    initial_timestamps: HashMap<String, DateTime<Utc>>, // container -> last_timestamp
    registry_tx: Option<tokio::sync::mpsc::UnboundedSender<RegistryUpdate>>,
}

impl DockerTailer {
    /// Create a new DockerTailer
    /// Connects to Docker daemon using default connection (unix socket or named pipe)
    pub fn new() -> anyhow::Result<Self> {
        let docker = Docker::connect_with_local_defaults()?;
        Ok(Self {
            docker,
            containers: HashMap::new(),
            initial_timestamps: HashMap::new(),
            registry_tx: None,
        })
    }

    /// Set the registry channel for sending position updates
    pub fn set_registry_sender(&mut self, tx: tokio::sync::mpsc::UnboundedSender<RegistryUpdate>) {
        self.registry_tx = Some(tx);
    }

    /// Set initial timestamp for a container (loaded from registry)
    pub fn set_initial_timestamp(&mut self, container: String, timestamp: DateTime<Utc>) {
        self.initial_timestamps.insert(container, timestamp);
    }

    /// Read new log lines from a Docker container since last poll
    /// Returns a Vec of new lines found
    ///
    /// # Arguments
    /// * `container` - Container name or ID
    pub async fn poll(&mut self, container: &str) -> anyhow::Result<Vec<String>> {
        use futures_util::StreamExt;

        let mut lines = Vec::new();

        // Check if we're already tracking this container
        let is_new_container = !self.containers.contains_key(container);

        if is_new_container {
            // First time seeing this container
            // Check if we have an initial timestamp from registry
            let initial_timestamp = if let Some(saved_ts) = self.initial_timestamps.get(container) {
                eprintln!(
                    "Resuming Docker container {} from registry timestamp {}",
                    container, saved_ts
                );
                Some(*saved_ts)
            } else {
                // Start from "now" to only capture new logs
                eprintln!("Now tailing Docker container: {}", container);
                Some(Utc::now())
            };

            self.containers.insert(
                container.to_string(),
                ContainerState {
                    last_timestamp: initial_timestamp,
                },
            );
            return Ok(lines); // Return empty on first poll (similar to file tailer)
        }

        let state = self.containers.get_mut(container).unwrap();

        // Build log options
        let mut options = LogsOptions::<String> {
            stdout: true,
            stderr: true,
            follow: false,    // Don't follow, just get what's available
            timestamps: true, // We need timestamps to track position
            ..Default::default()
        };

        // If we have a last timestamp, only get logs after that
        if let Some(since) = state.last_timestamp {
            // Docker accepts Unix timestamp as "since" parameter
            options.since = since.timestamp();
        }

        // Fetch logs from Docker
        let mut stream = self.docker.logs(container, Some(options));

        let mut latest_timestamp = state.last_timestamp;

        // Process log stream
        while let Some(log_result) = stream.next().await {
            match log_result {
                Ok(log_output) => {
                    let log_str = log_output.to_string();

                    // Docker log format: "timestamp log_content"
                    // We need to parse the timestamp and extract just the content
                    if let Some((timestamp_str, content)) = log_str.split_once(' ') {
                        // Parse timestamp to track our position
                        if let Ok(timestamp) = timestamp_str.parse::<DateTime<Utc>>() {
                            // Update latest timestamp
                            if latest_timestamp.is_none() || timestamp > latest_timestamp.unwrap() {
                                latest_timestamp = Some(timestamp);
                            }
                        }

                        // Add the content (without timestamp prefix)
                        lines.push(content.trim().to_string());
                    } else {
                        // No timestamp found, just use the whole line
                        lines.push(log_str.trim().to_string());
                    }
                }
                Err(e) => {
                    // Container might not exist or be stopped
                    eprintln!("Error fetching logs from {}: {}", container, e);
                    return Err(e.into());
                }
            }
        }

        // Update state with latest timestamp
        state.last_timestamp = latest_timestamp;

        // Send update to registry if enabled
        if let Some(ref tx) = self.registry_tx {
            if let Some(ts) = latest_timestamp {
                let _ = tx.send(RegistryUpdate::UpdateContainer {
                    container: container.to_string(),
                    last_timestamp: ts,
                });
            }
        }

        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: These tests require Docker to be running
    // They are integration tests and may be skipped in CI environments

    #[tokio::test]
    #[ignore] // Ignore by default since it requires Docker
    async fn test_docker_tailer_creation() {
        // This test just verifies we can connect to Docker
        let result = DockerTailer::new();

        // If Docker is running, this should succeed
        // If not, we expect a connection error
        match result {
            Ok(_) => println!("Successfully connected to Docker"),
            Err(e) => println!("Docker not available: {}", e),
        }
    }

    #[tokio::test]
    #[ignore] // Ignore by default since it requires Docker
    async fn test_docker_tailer_nonexistent_container() {
        let mut tailer = DockerTailer::new().unwrap();

        // Try to poll a container that doesn't exist
        let result = tailer.poll("nonexistent-container-12345").await;

        // Should return an error
        assert!(result.is_err());
    }
}
