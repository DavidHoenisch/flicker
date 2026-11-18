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
use crate::retry_queue::{RetryBatch, RetryConfig, RetryQueue};
use crate::tailer::LogTailer;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;
use tokio::time;

/// A line read from a source
#[derive(Debug, Clone, PartialEq)]
pub struct SourceLine {
    pub text: String,
    /// When the source says the line was logged, if it knows
    pub timestamp: Option<DateTime<Utc>>,
}

impl SourceLine {
    pub fn new(text: impl Into<String>, timestamp: Option<DateTime<Utc>>) -> Self {
        Self {
            text: text.into(),
            timestamp,
        }
    }
}

/// A pollable log source
#[async_trait]
pub trait Source: Send {
    /// Human-readable name for log messages
    fn label(&self) -> String;
    /// Value used for `LogEntry::path`
    fn entry_path(&self) -> String;
    /// Read new lines since the last poll
    async fn poll(&mut self) -> anyhow::Result<Vec<SourceLine>>;
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
    async fn poll(&mut self) -> anyhow::Result<Vec<SourceLine>> {
        // Plain files carry no structured timestamp; the read time is used
        Ok(self
            .tailer
            .poll(&self.path)?
            .into_iter()
            .map(|line| SourceLine::new(line, None))
            .collect())
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
    async fn poll(&mut self) -> anyhow::Result<Vec<SourceLine>> {
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
    async fn poll(&mut self) -> anyhow::Result<Vec<SourceLine>> {
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
    /// Reading paused because the retry queue is full
    paused: bool,
    /// Entries dropped after exceeding max retries
    dropped_entries: u64,
    /// Random per-run prefix for entry ids; a counter makes each id unique
    id_prefix: String,
    next_entry_id: u64,
}

impl<S: Source> Runner<S> {
    /// Resend queued batches oldest-first, stopping at the first failure.
    /// `force` ignores the backoff timer (used on shutdown).
    async fn send_retries(&mut self, force: bool) {
        while let Some(batch) = self.retry_queue.ready(force) {
            let (seq, entries) = (batch.seq, batch.entries.clone());
            if batch.failures > 0 {
                println!(
                    "[Retry] Attempting to resend batch of {} entries from {} (retry {})",
                    entries.len(),
                    self.label,
                    batch.failures
                );
            }

            match self.cfg.destination.send_batch(entries).await {
                Ok(()) => {
                    self.retry_queue.ack();
                    self.checkpoints.resolve(seq);
                }
                Err(e) => {
                    eprintln!("[Retry] Failed to resend batch from {}: {}", self.label, e);
                    if let Some(dropped) = self.retry_queue.fail() {
                        self.drop_batch(dropped);
                    }
                    break;
                }
            }
        }
    }

    /// Record a batch that will never be delivered
    fn drop_batch(&mut self, batch: RetryBatch) {
        self.dropped_entries += batch.entries.len() as u64;
        match self.retry_queue.dead_letter(&batch) {
            Ok(true) => eprintln!(
                "{}: wrote {} undeliverable entries to dead-letter file ({} dropped total)",
                self.label,
                batch.entries.len(),
                self.dropped_entries
            ),
            Ok(false) => eprintln!(
                "{}: dropped {} undeliverable entries ({} dropped total); set retry.dead_letter_path to keep them",
                self.label,
                batch.entries.len(),
                self.dropped_entries
            ),
            Err(e) => eprintln!(
                "{}: failed to write dead-letter file, dropped {} entries ({} dropped total): {}",
                self.label,
                batch.entries.len(),
                self.dropped_entries,
                e
            ),
        }
        // Lost either way; don't let it hold back the registry position
        self.checkpoints.resolve(batch.seq);
    }

    async fn poll(&mut self) {
        match self.source.poll().await {
            Ok(lines) => {
                let lines_read = lines.len();
                let before = self.buffer.len();
                let read_at = Utc::now();
                for SourceLine { text, timestamp } in lines {
                    if self.cfg.filter.should_ship(&text) {
                        let line = match &self.cfg.masking {
                            Some(m) => m.apply(&text),
                            None => text,
                        };
                        self.next_entry_id += 1;
                        self.buffer.push(LogEntry {
                            path: self.entry_path.clone(),
                            line,
                            timestamp: timestamp.unwrap_or(read_at),
                            id: format!("{}-{}", self.id_prefix, self.next_entry_id),
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
        self.last_flush = Instant::now();

        // Keep delivery in order: wait behind batches that are still being retried
        if !self.retry_queue.is_empty() {
            self.retry_queue.enqueue(seq, batch);
            return;
        }

        if let Err(e) = self.cfg.destination.send_batch(batch.clone()).await {
            eprintln!("Failed to ship batch from {}: {}", self.label, e);
            self.retry_queue.add_failed_batch(seq, batch);
        } else {
            self.checkpoints.resolve(seq);
        }
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

        // DESIGN CHOICE: Back-pressure. With the retry queue full, stop reading (the
        // source position stays put) rather than dropping batches to make room.
        if self.retry_queue.is_full() {
            if !self.paused {
                eprintln!(
                    "{}: retry queue full ({} batches), pausing reads until the destination recovers",
                    self.label,
                    self.retry_queue.len()
                );
                self.paused = true;
            }
            self.commit_checkpoint();
            return;
        }
        if self.paused {
            println!("{}: retry queue drained, resuming reads", self.label);
            self.paused = false;
        }

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
        paused: false,
        dropped_entries: 0,
        id_prefix: uuid::Uuid::new_v4().simple().to_string(),
        next_entry_id: 0,
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

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Emits one numbered line per poll
    struct CountingSource {
        polls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Source for CountingSource {
        fn label(&self) -> String {
            "test".to_string()
        }
        fn entry_path(&self) -> String {
            "test".to_string()
        }
        async fn poll(&mut self) -> anyhow::Result<Vec<SourceLine>> {
            let n = self.polls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![SourceLine::new(format!("line{}", n), None)])
        }
        fn checkpoint(&self) -> Option<RegistryUpdate> {
            None
        }
    }

    /// Records delivered lines; fails while `up` is false
    struct FlakyDestination {
        up: Arc<AtomicBool>,
        received: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Destination for FlakyDestination {
        async fn send(&self, entry: LogEntry) -> anyhow::Result<()> {
            self.send_batch(vec![entry]).await
        }
        async fn send_batch(&self, entries: Vec<LogEntry>) -> anyhow::Result<()> {
            if !self.up.load(Ordering::SeqCst) {
                anyhow::bail!("destination down");
            }
            let mut received = self.received.lock().unwrap();
            received.extend(entries.into_iter().map(|e| e.line));
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_backpressure_and_in_order_delivery() {
        let polls = Arc::new(AtomicUsize::new(0));
        let up = Arc::new(AtomicBool::new(false));
        let received = Arc::new(Mutex::new(Vec::new()));
        let cfg = PipelineConfig {
            polling_frequency: Duration::from_millis(1),
            buffer_size: 1,
            flush_interval: Duration::from_secs(60),
            filter: LogFilter::new(vec![], vec![]).unwrap(),
            masking: None,
            destination: Box::new(FlakyDestination {
                up: up.clone(),
                received: received.clone(),
            }),
            retry: RetryConfig {
                max_retries: 1000,
                initial_delay_ms: 0,
                max_delay_ms: 0,
                max_queue_size: 3,
                dead_letter_path: None,
            },
            registry_tx: None,
        };
        let source = CountingSource {
            polls: polls.clone(),
        };
        let mut runner = Runner {
            label: source.label(),
            entry_path: source.entry_path(),
            retry_queue: RetryQueue::new(cfg.retry.clone()),
            checkpoints: CheckpointTracker::default(),
            buffer: Vec::new(),
            last_flush: Instant::now(),
            paused: false,
            dropped_entries: 0,
            id_prefix: "test".to_string(),
            next_entry_id: 0,
            source,
            cfg,
        };

        // Destination down: reading stops once the queue is full, nothing dropped
        for _ in 0..10 {
            runner.tick().await;
        }
        assert_eq!(polls.load(Ordering::SeqCst), 3);
        assert_eq!(runner.retry_queue.len(), 3);
        assert_eq!(runner.dropped_entries, 0);

        // Destination recovers: backlog delivered first, in order, then reading resumes
        up.store(true, Ordering::SeqCst);
        for _ in 0..2 {
            runner.tick().await;
        }
        assert_eq!(
            *received.lock().unwrap(),
            vec!["line0", "line1", "line2", "line3", "line4"]
        );
    }
}
