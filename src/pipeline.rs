// Shared source -> filter -> mask -> buffer -> destination loop
//
// DESIGN: Files, Docker containers and API sources all run the same loop.
// Each source type only implements the Source trait (poll + checkpoint).

use crate::api_tailer::ApiTailer;
use crate::config::ApiSourceConfig;
use crate::destinations::{Destination, LogEntry};
use crate::docker_tailer::DockerTailer;
use crate::filter::LogFilter;
use crate::masking::MaskingEngine;
use crate::registry::RegistryUpdate;
use crate::retry_queue::{RetryConfig, RetryQueue};
use crate::tailer::LogTailer;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;
use tokio::time;

/// A pollable log source
#[async_trait]
pub trait Source: Send {
    /// Human-readable name for log messages
    fn label(&self) -> String;
    /// Value used for `LogEntry::path`
    fn entry_path(&self) -> String;
    /// Read new lines since the last poll
    async fn poll(&mut self) -> anyhow::Result<Vec<String>>;
    /// Position covering everything returned by `poll` so far
    fn checkpoint(&self) -> Option<RegistryUpdate>;
}

pub struct FileSource {
    pub tailer: LogTailer,
    pub path: String,
}

#[async_trait]
impl Source for FileSource {
    fn label(&self) -> String {
        self.path.clone()
    }
    fn entry_path(&self) -> String {
        self.path.clone()
    }
    async fn poll(&mut self) -> anyhow::Result<Vec<String>> {
        self.tailer.poll(&self.path)
    }
    fn checkpoint(&self) -> Option<RegistryUpdate> {
        self.tailer.checkpoint(&self.path)
    }
}

pub struct DockerSource {
    pub tailer: DockerTailer,
    pub container: String,
}

#[async_trait]
impl Source for DockerSource {
    fn label(&self) -> String {
        format!("Docker container {}", self.container)
    }
    fn entry_path(&self) -> String {
        format!("docker://{}", self.container)
    }
    async fn poll(&mut self) -> anyhow::Result<Vec<String>> {
        self.tailer.poll(&self.container).await
    }
    fn checkpoint(&self) -> Option<RegistryUpdate> {
        self.tailer.checkpoint(&self.container)
    }
}

pub struct ApiSource {
    pub tailer: ApiTailer,
    pub config: ApiSourceConfig,
}

#[async_trait]
impl Source for ApiSource {
    fn label(&self) -> String {
        format!("API source {}", self.config.name)
    }
    fn entry_path(&self) -> String {
        format!("api://{}", self.config.name)
    }
    async fn poll(&mut self) -> anyhow::Result<Vec<String>> {
        self.tailer.poll(&self.config).await
    }
    fn checkpoint(&self) -> Option<RegistryUpdate> {
        self.tailer.checkpoint(&self.config.name)
    }
}

/// Everything the loop needs besides the source itself
pub struct PipelineConfig {
    pub polling_frequency: Duration,
    pub buffer_size: usize,
    pub flush_interval: Duration,
    pub filter: LogFilter,
    pub masking: Option<MaskingEngine>,
    pub destination: Box<dyn Destination>,
    pub retry: RetryConfig,
    pub registry_tx: Option<UnboundedSender<RegistryUpdate>>,
}

/// Tracks which source positions are safe to persist.
///
/// DESIGN CHOICE: A checkpoint is only committed once every batch read before
/// it has been delivered (or permanently dropped by the retry queue). This gives
/// at-least-once delivery: after a crash, undelivered lines are re-read.
#[derive(Default)]
pub struct CheckpointTracker {
    next_seq: u64,
    /// Sealed batches in read order: (seq, checkpoint at seal time, resolved)
    inflight: VecDeque<(u64, Option<RegistryUpdate>, bool)>,
    /// Checkpoint covering the current (unsealed) buffer
    open: Option<RegistryUpdate>,
}

impl CheckpointTracker {
    /// Record the source position after a poll
    pub fn observe(&mut self, checkpoint: Option<RegistryUpdate>) {
        if checkpoint.is_some() {
            self.open = checkpoint;
        }
    }

    /// Seal the current buffer into a batch; returns its seq
    pub fn seal(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.inflight.push_back((seq, self.open.take(), false));
        seq
    }

    /// Mark a batch as delivered or permanently dropped
    pub fn resolve(&mut self, seq: u64) {
        if let Some(entry) = self.inflight.iter_mut().find(|(s, _, _)| *s == seq) {
            entry.2 = true;
        }
    }

    /// Return the newest checkpoint that is safe to persist, if it changed
    pub fn take_committable(&mut self, buffer_is_empty: bool) -> Option<RegistryUpdate> {
        let mut latest = None;
        while let Some((_, _, true)) = self.inflight.front() {
            let (_, cp, _) = self.inflight.pop_front().unwrap();
            if cp.is_some() {
                latest = cp;
            }
        }
        // Lines read but not buffered (e.g. filtered out) are safe once nothing is pending
        if self.inflight.is_empty() && buffer_is_empty && self.open.is_some() {
            latest = self.open.take();
        }
        latest
    }
}

/// Per-source runtime state
struct Runner<S: Source> {
    source: S,
    cfg: PipelineConfig,
    label: String,
    entry_path: String,
    retry_queue: RetryQueue,
    checkpoints: CheckpointTracker,
    buffer: Vec<LogEntry>,
    last_flush: Instant,
}

impl<S: Source> Runner<S> {
    /// Resend queued batches. `force` ignores backoff timers (used on shutdown).
    async fn send_retries(&mut self, force: bool) {
        let batches = if force {
            self.retry_queue.drain_all()
        } else {
            self.retry_queue.get_ready_batches()
        };
        for (seq, entries, retry_count) in batches {
            println!(
                "[Retry] Attempting to resend batch of {} entries from {} (retry {})",
                entries.len(),
                self.label,
                retry_count + 1
            );

            if let Err(e) = self.cfg.destination.send_batch(entries.clone()).await {
                eprintln!("[Retry] Failed to resend batch from {}: {}", self.label, e);
                for dropped in self.retry_queue.retry_failed(seq, entries, retry_count) {
                    self.checkpoints.resolve(dropped);
                }
            } else {
                println!("[Retry] Successfully resent batch from {}", self.label);
                self.checkpoints.resolve(seq);
            }
        }
    }

    async fn poll(&mut self) {
        match self.source.poll().await {
            Ok(lines) => {
                let lines_read = lines.len();
                let before = self.buffer.len();
                for line in lines {
                    if self.cfg.filter.should_ship(&line) {
                        let line = match &self.cfg.masking {
                            Some(m) => m.apply(&line),
                            None => line,
                        };
                        self.buffer.push(LogEntry {
                            path: self.entry_path.clone(),
                            line,
                        });
                    }
                }
                self.checkpoints.observe(self.source.checkpoint());

                if lines_read > 0 {
                    println!(
                        "[{}] Read {} lines, shipped {} (buffer: {})",
                        self.label,
                        lines_read,
                        self.buffer.len() - before,
                        self.buffer.len()
                    );
                }
            }
            Err(e) => {
                eprintln!("Error polling {}: {}", self.label, e);
            }
        }
    }

    async fn flush(&mut self, reason: &str) {
        if self.buffer.is_empty() {
            return;
        }
        println!(
            "Flushing {} entries from {} ({})",
            self.buffer.len(),
            self.label,
            reason
        );

        let seq = self.checkpoints.seal();
        let batch = std::mem::take(&mut self.buffer);
        if let Err(e) = self.cfg.destination.send_batch(batch.clone()).await {
            eprintln!("Failed to ship batch from {}: {}", self.label, e);
            if let Some(dropped) = self.retry_queue.add_failed_batch(seq, batch, 0) {
                self.checkpoints.resolve(dropped);
            }
        } else {
            self.checkpoints.resolve(seq);
        }
        self.last_flush = Instant::now();
    }

    fn commit_checkpoint(&mut self) {
        if let Some(cp) = self.checkpoints.take_committable(self.buffer.is_empty())
            && let Some(tx) = &self.cfg.registry_tx
        {
            let _ = tx.send(cp);
        }
    }

    async fn tick(&mut self) {
        self.send_retries(false).await;
        self.poll().await;

        if self.buffer.len() >= self.cfg.buffer_size {
            self.flush("buffer full").await;
        } else if self.last_flush.elapsed() >= self.cfg.flush_interval {
            self.flush("time elapsed").await;
        }

        self.commit_checkpoint();
    }

    async fn shutdown(&mut self) {
        println!("Shutting down {}, flushing buffers", self.label);
        self.flush("shutdown").await;
        self.send_retries(true).await;
        self.commit_checkpoint();
        if !self.retry_queue.is_empty() {
            eprintln!(
                "{}: {} undelivered batch(es) at shutdown; they will be re-read on restart if tracking is enabled",
                self.label,
                self.retry_queue.len()
            );
        }
    }
}

/// Run a source until `shutdown` is signalled, then flush and return
pub async fn run_source(
    source: impl Source,
    cfg: PipelineConfig,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut interval = time::interval(cfg.polling_frequency);
    let mut runner = Runner {
        label: source.label(),
        entry_path: source.entry_path(),
        retry_queue: RetryQueue::new(cfg.retry.clone()),
        checkpoints: CheckpointTracker::default(),
        buffer: Vec::with_capacity(cfg.buffer_size),
        last_flush: Instant::now(),
        source,
        cfg,
    };

    loop {
        if *shutdown.borrow() {
            break;
        }
        tokio::select! {
            _ = interval.tick() => runner.tick().await,
            _ = shutdown.changed() => {}
        }
    }

    runner.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(position: u64) -> Option<RegistryUpdate> {
        Some(RegistryUpdate::UpdateFile {
            path: "/test.log".to_string(),
            position,
            inode: 1,
        })
    }

    fn position(update: Option<RegistryUpdate>) -> Option<u64> {
        match update {
            Some(RegistryUpdate::UpdateFile { position, .. }) => Some(position),
            _ => None,
        }
    }

    #[test]
    fn test_checkpoint_not_committed_until_delivered() {
        let mut t = CheckpointTracker::default();
        t.observe(cp(10));
        // Lines are buffered, not yet sent
        assert_eq!(position(t.take_committable(false)), None);

        let seq = t.seal();
        assert_eq!(position(t.take_committable(true)), None);

        t.resolve(seq);
        assert_eq!(position(t.take_committable(true)), Some(10));
        // Not returned twice
        assert_eq!(position(t.take_committable(true)), None);
    }

    #[test]
    fn test_checkpoint_blocked_by_older_failed_batch() {
        let mut t = CheckpointTracker::default();
        t.observe(cp(10));
        let first = t.seal();
        t.observe(cp(20));
        let second = t.seal();

        // Newer batch delivered, older still retrying: nothing safe to commit
        t.resolve(second);
        assert_eq!(position(t.take_committable(true)), None);

        t.resolve(first);
        assert_eq!(position(t.take_committable(true)), Some(20));
    }

    #[test]
    fn test_checkpoint_filtered_lines_commit_when_idle() {
        let mut t = CheckpointTracker::default();
        // All lines filtered out: buffer empty, nothing in flight
        t.observe(cp(10));
        assert_eq!(position(t.take_committable(true)), Some(10));
    }

    #[test]
    fn test_checkpoint_filtered_lines_wait_for_inflight() {
        let mut t = CheckpointTracker::default();
        t.observe(cp(10));
        let seq = t.seal();
        // Later poll with only filtered lines
        t.observe(cp(20));
        assert_eq!(position(t.take_committable(true)), None);

        t.resolve(seq);
        assert_eq!(position(t.take_committable(true)), Some(20));
    }
}
