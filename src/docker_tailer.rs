use crate::pipeline::SourceLine;
use crate::registry::RegistryUpdate;
use bollard::Docker;
use bollard::container::LogsOptions;
use chrono::{DateTime, Utc};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// Tracks state for a single container being tailed
#[derive(Clone, Default)]
struct ContainerState {
    last_timestamp: Option<DateTime<Utc>>,
    /// Hashes of lines already shipped at exactly `last_timestamp`
    boundary: HashSet<u64>,
}

impl ContainerState {
    /// Decide whether a line is new, updating the position if so.
    ///
    /// DESIGN CHOICE: Docker's `since` filter has one-second resolution, so each
    /// poll re-returns lines from the last second. Lines must arrive in timestamp
    /// order (as Docker returns them); anything older than the last shipped line,
    /// or identical to a line already shipped at that exact timestamp, is a repeat.
    fn admit(&mut self, timestamp: DateTime<Utc>, content: &str) -> bool {
        let hash = {
            let mut h = DefaultHasher::new();
            content.hash(&mut h);
            h.finish()
        };
        match self.last_timestamp {
            Some(last) if timestamp < last => false,
            Some(last) if timestamp == last => self.boundary.insert(hash),
            _ => {
                self.last_timestamp = Some(timestamp);
                self.boundary.clear();
                self.boundary.insert(hash);
                true
            }
        }
    }
}

/// Manages tailing multiple Docker containers
pub struct DockerTailer {
    docker: Docker,
    containers: HashMap<String, ContainerState>,
    initial_timestamps: HashMap<String, DateTime<Utc>>, // container -> last_timestamp
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
        })
    }

    /// Current position for a container, for persisting to the registry
    pub fn checkpoint(&self, container: &str) -> Option<RegistryUpdate> {
        let ts = self.containers.get(container)?.last_timestamp?;
        Some(RegistryUpdate::UpdateContainer {
            container: container.to_string(),
            last_timestamp: ts,
        })
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
    pub async fn poll(&mut self, container: &str) -> anyhow::Result<Vec<SourceLine>> {
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
                    boundary: HashSet::new(),
                },
            );
            return Ok(lines); // Return empty on first poll (similar to file tailer)
        }

        // Work on a copy so a mid-stream error doesn't advance the position
        let mut state = self.containers.get(container).unwrap().clone();

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

        // Process log stream
        while let Some(log_result) = stream.next().await {
            match log_result {
                Ok(log_output) => {
                    let log_str = log_output.to_string();

                    // Docker log format: "timestamp log_content"
                    // DESIGN CHOICE: Only strip the line terminator; leading whitespace
                    // matters for indented output like stack traces.
                    let log_str = log_str.trim_end_matches(['\n', '\r']);
                    match log_str.split_once(' ').and_then(|(ts, content)| {
                        Some((ts.parse::<DateTime<Utc>>().ok()?, content))
                    }) {
                        Some((timestamp, content)) => {
                            if state.admit(timestamp, content) {
                                lines.push(SourceLine::new(content, Some(timestamp)));
                            }
                        }
                        // No timestamp found, just use the whole line
                        None => lines.push(SourceLine::new(log_str, None)),
                    }
                }
                Err(e) => {
                    // Container might not exist or be stopped
                    eprintln!("Error fetching logs from {}: {}", container, e);
                    return Err(e.into());
                }
            }
        }

        self.containers.insert(container.to_string(), state);

        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn test_admit_skips_repeats_from_rounded_since() {
        let mut state = ContainerState::default();
        assert!(state.admit(ts("2030-01-01T00:00:00.100Z"), "a"));
        assert!(state.admit(ts("2030-01-01T00:00:00.200Z"), "b"));

        // Next poll: since= rounds down, Docker re-returns both lines plus a new one
        assert!(!state.admit(ts("2030-01-01T00:00:00.100Z"), "a"));
        assert!(!state.admit(ts("2030-01-01T00:00:00.200Z"), "b"));
        assert!(state.admit(ts("2030-01-01T00:00:00.300Z"), "c"));
    }

    #[test]
    fn test_admit_keeps_distinct_lines_with_same_timestamp() {
        let mut state = ContainerState::default();
        let t = ts("2030-01-01T00:00:00Z");
        assert!(state.admit(t, "a"));
        assert!(state.admit(t, "b"));
        // Re-fetched on the next poll
        assert!(!state.admit(t, "a"));
        assert!(!state.admit(t, "b"));
    }

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
