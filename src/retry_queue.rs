// Retry queue for failed log batches with exponential backoff
//
// DESIGN: When log batches fail to send to destinations, they are queued
// for retry with exponential backoff. This prevents data loss during
// temporary network issues or destination downtime.
//
// DESIGN CHOICE: The queue is strictly FIFO. Only the oldest batch is retried,
// and while anything is queued new batches wait behind it. Each source has a
// single destination, so if the oldest batch can't be delivered newer ones
// almost certainly can't either; this keeps delivery in order and avoids
// hammering a down destination. The caller applies back-pressure (stops
// reading) when the queue is full instead of dropping data.

use crate::destinations::LogEntry;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{Duration, Instant};

/// Configuration for retry behavior
#[derive(Clone, Debug)]
pub struct RetryConfig {
    /// Maximum number of retry attempts before dropping a batch
    pub max_retries: u32,
    /// Initial delay before first retry (doubles with each retry)
    pub initial_delay_ms: u64,
    /// Maximum delay between retries (caps exponential growth)
    pub max_delay_ms: u64,
    /// Queue length at which the source stops reading new lines
    pub max_queue_size: usize,
    /// Optional JSONL file that batches dropped after max retries are appended to
    pub dead_letter_path: Option<String>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            initial_delay_ms: 1000, // 1 second
            max_delay_ms: 60000,    // 60 seconds
            max_queue_size: 100,    // 100 batches
            dead_letter_path: None,
        }
    }
}

/// A batch waiting to be (re)sent
#[derive(Debug)]
pub struct RetryBatch {
    pub seq: u64,
    pub entries: Vec<LogEntry>,
    /// Failed send attempts so far
    pub failures: u32,
}

/// FIFO queue of undelivered batches with exponential backoff on the head
pub struct RetryQueue {
    queue: VecDeque<RetryBatch>,
    config: RetryConfig,
    next_attempt: Instant,
}

impl RetryQueue {
    /// Create a new retry queue with the given configuration
    pub fn new(config: RetryConfig) -> Self {
        Self {
            queue: VecDeque::new(),
            config,
            next_attempt: Instant::now(),
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// True when the source should stop reading until the queue drains
    pub fn is_full(&self) -> bool {
        self.queue.len() >= self.config.max_queue_size
    }

    /// Queue a batch that failed its first send attempt
    pub fn add_failed_batch(&mut self, seq: u64, entries: Vec<LogEntry>) {
        if self.queue.is_empty() {
            self.next_attempt = Instant::now() + self.calculate_backoff_delay(0);
        }
        self.queue.push_back(RetryBatch {
            seq,
            entries,
            failures: 1,
        });
        println!(
            "[Retry] Added batch to retry queue (queue size: {})",
            self.queue.len()
        );
    }

    /// Queue a batch behind older undelivered ones without trying to send it
    pub fn enqueue(&mut self, seq: u64, entries: Vec<LogEntry>) {
        self.queue.push_back(RetryBatch {
            seq,
            entries,
            failures: 0,
        });
    }

    /// The oldest batch, if its backoff has elapsed (or `force` is set)
    pub fn ready(&self, force: bool) -> Option<&RetryBatch> {
        if force || Instant::now() >= self.next_attempt {
            self.queue.front()
        } else {
            None
        }
    }

    /// The oldest batch was delivered; the next one may be sent immediately
    pub fn ack(&mut self) -> Option<RetryBatch> {
        self.next_attempt = Instant::now();
        self.queue.pop_front()
    }

    /// The oldest batch failed again. Returns it if it exceeded max retries and
    /// was dropped from the queue.
    pub fn fail(&mut self) -> Option<RetryBatch> {
        let head = self.queue.front_mut()?;
        head.failures += 1;
        let retries = head.failures - 1; // first failure was the initial send
        if retries > self.config.max_retries {
            eprintln!(
                "[Retry] Max retries ({}) exceeded, dropping batch of {} entries",
                self.config.max_retries,
                head.entries.len()
            );
            // The next batch hasn't been tried against this outage yet; give it a short wait
            self.next_attempt = Instant::now() + self.calculate_backoff_delay(0);
            return self.queue.pop_front();
        }
        let delay = self.calculate_backoff_delay(retries);
        self.next_attempt = Instant::now() + delay;
        println!(
            "[Retry] Retry {} failed, next attempt in {:?} (queue size: {})",
            retries,
            delay,
            self.queue.len()
        );
        None
    }

    /// Append a dropped batch to the dead-letter file, if one is configured
    pub fn dead_letter(&self, batch: &RetryBatch) -> std::io::Result<bool> {
        let Some(path) = &self.config.dead_letter_path else {
            return Ok(false);
        };
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let mut out = Vec::new();
        for entry in &batch.entries {
            serde_json::to_writer(&mut out, entry)?;
            out.push(b'\n');
        }
        file.write_all(&out)?;
        Ok(true)
    }

    /// Calculate exponential backoff delay: initial_delay * 2^retry, capped
    fn calculate_backoff_delay(&self, retry: u32) -> Duration {
        // DESIGN CHOICE: Saturating math, so large retry counts cap at max_delay_ms
        // instead of overflowing (panic in debug, wrap to ~0ms in release).
        let delay_ms = self
            .config
            .initial_delay_ms
            .saturating_mul(2u64.saturating_pow(retry));
        Duration::from_millis(delay_ms.min(self.config.max_delay_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_entry(line: &str) -> LogEntry {
        LogEntry::new("/test.log", line)
    }

    fn test_config(max_retries: u32, max_queue_size: usize) -> RetryConfig {
        RetryConfig {
            max_retries,
            initial_delay_ms: 50,
            max_delay_ms: 10000,
            max_queue_size,
            dead_letter_path: None,
        }
    }

    #[test]
    fn test_add_and_retrieve_batch() {
        let mut queue = RetryQueue::new(test_config(5, 10));
        queue.add_failed_batch(1, vec![create_test_entry("test line")]);

        // Not ready until the backoff elapses
        assert!(queue.ready(false).is_none());
        // Forced (shutdown) ignores backoff
        assert_eq!(queue.ready(true).unwrap().seq, 1);

        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(queue.ready(false).unwrap().seq, 1);
        assert_eq!(queue.ack().unwrap().seq, 1);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_exponential_backoff() {
        let config = RetryConfig {
            initial_delay_ms: 100,
            ..test_config(5, 10)
        };
        let queue = RetryQueue::new(config);

        assert_eq!(queue.calculate_backoff_delay(0), Duration::from_millis(100));
        assert_eq!(queue.calculate_backoff_delay(1), Duration::from_millis(200));
        assert_eq!(queue.calculate_backoff_delay(2), Duration::from_millis(400));
        assert_eq!(queue.calculate_backoff_delay(3), Duration::from_millis(800));

        // Test cap
        assert_eq!(
            queue.calculate_backoff_delay(10),
            Duration::from_millis(10000)
        );
    }

    #[test]
    fn test_backoff_does_not_overflow() {
        let config = RetryConfig {
            initial_delay_ms: u64::MAX / 2,
            max_delay_ms: 60_000,
            ..test_config(100, 10)
        };
        let queue = RetryQueue::new(config);
        for retry in [0, 1, 63, 64, 100, u32::MAX] {
            assert_eq!(
                queue.calculate_backoff_delay(retry),
                Duration::from_millis(60_000)
            );
        }
    }

    #[test]
    fn test_max_retries() {
        let mut queue = RetryQueue::new(test_config(3, 10));
        queue.add_failed_batch(7, vec![create_test_entry("test line")]);

        // Retries 1..=3 fail but are kept
        for _ in 0..3 {
            assert!(queue.fail().is_none());
        }
        // The 4th retry exceeds max_retries and the batch is dropped
        let dropped = queue.fail().unwrap();
        assert_eq!(dropped.seq, 7);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_fifo_order_and_full() {
        let mut queue = RetryQueue::new(test_config(5, 3));
        queue.add_failed_batch(1, vec![create_test_entry("a")]);
        queue.enqueue(2, vec![create_test_entry("b")]);
        assert!(!queue.is_full());
        queue.enqueue(3, vec![create_test_entry("c")]);
        assert!(queue.is_full());

        // Only the head is ever offered, in order
        let mut order = Vec::new();
        while let Some(batch) = queue.ready(true) {
            order.push(batch.seq);
            queue.ack();
        }
        assert_eq!(order, vec![1, 2, 3]);
    }

    #[test]
    fn test_dead_letter_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dead.jsonl");
        let config = RetryConfig {
            dead_letter_path: Some(path.to_str().unwrap().to_string()),
            ..test_config(0, 10)
        };
        let mut queue = RetryQueue::new(config);
        queue.add_failed_batch(1, vec![create_test_entry("a"), create_test_entry("b")]);
        let dropped = queue.fail().unwrap();
        assert!(queue.dead_letter(&dropped).unwrap());

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 2);
        assert!(contents.contains("\"line\":\"a\""));
    }
}
