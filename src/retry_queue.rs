// Retry queue for failed log batches with exponential backoff
//
// DESIGN: When log batches fail to send to destinations, they are queued
// for retry with exponential backoff. This prevents data loss during
// temporary network issues or destination downtime.

use crate::destinations::LogEntry;
use std::collections::VecDeque;
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
    /// Maximum number of batches to keep in retry queue
    pub max_queue_size: usize,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            initial_delay_ms: 1000, // 1 second
            max_delay_ms: 60000,    // 60 seconds
            max_queue_size: 100,    // 100 batches
        }
    }
}

/// A batch waiting to be retried
#[derive(Debug)]
struct RetryBatch {
    entries: Vec<LogEntry>,
    retry_count: u32,
    next_retry_time: Instant,
}

/// Queue for managing failed batches with exponential backoff
pub struct RetryQueue {
    queue: VecDeque<RetryBatch>,
    config: RetryConfig,
}

impl RetryQueue {
    /// Create a new retry queue with the given configuration
    pub fn new(config: RetryConfig) -> Self {
        Self {
            queue: VecDeque::new(),
            config,
        }
    }

    /// Add a failed batch to the retry queue
    /// Returns true if batch was added, false if queue is full
    pub fn add_failed_batch(&mut self, entries: Vec<LogEntry>, retry_count: u32) -> bool {
        if self.queue.len() >= self.config.max_queue_size {
            eprintln!(
                "[Retry] Queue full ({} batches), dropping oldest batch",
                self.config.max_queue_size
            );
            // Drop oldest batch to make room
            self.queue.pop_front();
        }

        // Calculate next retry time using exponential backoff
        let delay = self.calculate_backoff_delay(retry_count);
        let next_retry_time = Instant::now() + delay;

        self.queue.push_back(RetryBatch {
            entries,
            retry_count,
            next_retry_time,
        });

        println!(
            "[Retry] Added batch to retry queue (retry count: {}, delay: {:?}, queue size: {})",
            retry_count,
            delay,
            self.queue.len()
        );

        true
    }

    /// Get batches that are ready for retry
    /// Returns a vector of (entries, retry_count) tuples
    pub fn get_ready_batches(&mut self) -> Vec<(Vec<LogEntry>, u32)> {
        let now = Instant::now();
        let mut ready = Vec::new();

        // We need to check all batches and remove the ones that are ready
        // We'll iterate through the queue and keep batches that aren't ready yet
        let mut remaining = VecDeque::new();

        while let Some(batch) = self.queue.pop_front() {
            if batch.next_retry_time <= now {
                // This batch is ready for retry
                ready.push((batch.entries, batch.retry_count));
            } else {
                // This batch isn't ready yet, keep it in the queue
                remaining.push_back(batch);
            }
        }

        // Put the remaining batches back in the queue
        self.queue = remaining;

        if !ready.is_empty() {
            println!("[Retry] Found {} batch(es) ready for retry", ready.len());
        }

        ready
    }

    /// Re-add a batch that failed retry (increments retry count)
    /// Returns true if batch was re-added, false if max retries exceeded
    pub fn retry_failed(&mut self, entries: Vec<LogEntry>, retry_count: u32) -> bool {
        if retry_count >= self.config.max_retries {
            eprintln!(
                "[Retry] Max retries ({}) exceeded, dropping batch of {} entries",
                self.config.max_retries,
                entries.len()
            );
            return false;
        }

        self.add_failed_batch(entries, retry_count + 1)
    }

    /// Calculate exponential backoff delay
    fn calculate_backoff_delay(&self, retry_count: u32) -> Duration {
        // Exponential backoff: initial_delay * 2^retry_count
        let delay_ms = self.config.initial_delay_ms * 2u64.pow(retry_count);
        let capped_delay = delay_ms.min(self.config.max_delay_ms);
        Duration::from_millis(capped_delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_entry(line: &str) -> LogEntry {
        LogEntry {
            path: "/test.log".to_string(),
            line: line.to_string(),
        }
    }

    #[test]
    fn test_add_and_retrieve_batch() {
        let mut queue = RetryQueue::new(RetryConfig::default());
        let entries = vec![create_test_entry("test line")];

        // Add batch with retry_count = 0
        assert!(queue.add_failed_batch(entries.clone(), 0));

        // Batch should be ready immediately (for testing)
        std::thread::sleep(std::time::Duration::from_millis(10));
        let ready = queue.get_ready_batches();
        assert_eq!(ready.len(), 0); // Not ready yet because of 1s delay

        // After delay, should be ready
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let ready = queue.get_ready_batches();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].1, 0); // retry_count
    }

    #[test]
    fn test_exponential_backoff() {
        let config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 100,
            max_delay_ms: 10000,
            max_queue_size: 10,
        };
        let queue = RetryQueue::new(config);

        // Test backoff calculation
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
    fn test_max_retries() {
        let config = RetryConfig {
            max_retries: 3,
            initial_delay_ms: 100,
            max_delay_ms: 10000,
            max_queue_size: 10,
        };
        let mut queue = RetryQueue::new(config);
        let entries = vec![create_test_entry("test line")];

        // Retry count 0, 1, 2 should succeed
        assert!(queue.retry_failed(entries.clone(), 0));
        assert!(queue.retry_failed(entries.clone(), 1));
        assert!(queue.retry_failed(entries.clone(), 2));

        // Retry count 3 should fail (equals max_retries)
        assert!(!queue.retry_failed(entries.clone(), 3));
    }

    #[test]
    fn test_queue_size_limit() {
        let config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 100,
            max_delay_ms: 10000,
            max_queue_size: 3,
        };
        let mut queue = RetryQueue::new(config);

        // Add 4 batches (exceeds max_queue_size of 3)
        for i in 0..4 {
            let entries = vec![create_test_entry(&format!("line {}", i))];
            queue.add_failed_batch(entries, 0);
        }

        // Queue should only have 3 items (oldest was dropped)
        // Wait for all batches to be ready
        std::thread::sleep(std::time::Duration::from_millis(110));
        let ready = queue.get_ready_batches();
        assert_eq!(ready.len(), 3);
    }

    #[test]
    fn test_retry_ordering() {
        let config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 100,
            max_delay_ms: 10000,
            max_queue_size: 10,
        };
        let mut queue = RetryQueue::new(config);

        // Add batches with different retry counts (different delays)
        queue.add_failed_batch(vec![create_test_entry("batch_400ms")], 2); // 400ms delay
        queue.add_failed_batch(vec![create_test_entry("batch_100ms")], 0); // 100ms delay
        queue.add_failed_batch(vec![create_test_entry("batch_200ms")], 1); // 200ms delay

        // Collect all batches as they become ready
        // Windows timing can be imprecise, so we verify the order rather than exact timing
        let mut collected_order: Vec<String> = Vec::new();

        // Wait for first batch (100ms) to be ready - use generous margin for Windows
        std::thread::sleep(std::time::Duration::from_millis(250));
        let ready = queue.get_ready_batches();
        assert!(
            !ready.is_empty(),
            "Expected at least one batch ready after 250ms"
        );
        for batch in &ready {
            collected_order.push(batch.0[0].line.clone());
        }

        // Wait for second batch (200ms) - should be ready by now
        std::thread::sleep(std::time::Duration::from_millis(150));
        let ready = queue.get_ready_batches();
        for batch in &ready {
            collected_order.push(batch.0[0].line.clone());
        }

        // Wait for third batch (400ms) - should be ready by now
        std::thread::sleep(std::time::Duration::from_millis(200));
        let ready = queue.get_ready_batches();
        for batch in &ready {
            collected_order.push(batch.0[0].line.clone());
        }

        // Verify all batches were collected
        assert_eq!(
            collected_order.len(),
            3,
            "Expected all 3 batches to be collected"
        );

        // Verify ordering: lower delay batches should come before higher delay batches
        // batch_100ms (100ms) should come before batch_200ms (200ms) and batch_400ms (400ms)
        let idx_100ms = collected_order
            .iter()
            .position(|x| x == "batch_100ms")
            .unwrap();
        let idx_200ms = collected_order
            .iter()
            .position(|x| x == "batch_200ms")
            .unwrap();
        let idx_400ms = collected_order
            .iter()
            .position(|x| x == "batch_400ms")
            .unwrap();

        assert!(
            idx_100ms < idx_400ms,
            "batch_100ms should come before batch_400ms"
        );
        assert!(
            idx_200ms < idx_400ms,
            "batch_200ms should come before batch_400ms"
        );
    }
}
