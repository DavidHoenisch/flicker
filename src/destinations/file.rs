// File destination - writes logs to a file
//
// DESIGN: Simple file output destination. Useful for:
// - Debugging (see exactly what Flicker is shipping)
// - Backup (archive logs to secondary location)
// - Format conversion (consolidate multiple logs into one file)
//
// Output format: JSON Lines (JSONL) - one JSON object per line
// This format is simple, streaming-friendly, and easy to parse

use super::{Destination, LogEntry};
use anyhow::Result;
use async_trait::async_trait;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

pub struct FileDestination {
    path: PathBuf,
    // DESIGN CHOICE: Store path only, open file on each write
    // This is simpler than using Mutex and works fine for append-mode writes.
    // The OS handles concurrent writes to the same file correctly.
}

impl FileDestination {
    pub fn new(path: String) -> Result<Self> {
        let path_buf = PathBuf::from(&path);

        // Create parent directories if they don't exist
        if let Some(parent) = path_buf.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Test that we can write to the file
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path_buf)?;

        Ok(Self { path: path_buf })
    }
}

#[async_trait]
impl Destination for FileDestination {
    async fn send(&self, entry: LogEntry) -> Result<()> {
        self.send_batch(vec![entry]).await
    }

    async fn send_batch(&self, entries: Vec<LogEntry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        println!(
            "[FILE] Writing batch of {} entries to {:?}",
            entries.len(),
            self.path
        );

        // DESIGN CHOICE: Use spawn_blocking for file I/O
        // File writes are blocking operations, so we move them
        // to a thread pool to avoid blocking the async runtime
        let entries_json: Vec<String> = entries
            .iter()
            .map(|entry| serde_json::to_string(entry).unwrap())
            .collect();

        let path = self.path.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            // Open file in append mode
            let mut file = OpenOptions::new().create(true).append(true).open(&path)?;

            // Write all entries
            for json in entries_json {
                writeln!(file, "{}", json)?;
            }

            // Flush to ensure data is written
            file.flush()?;
            Ok(())
        })
        .await??;

        println!("[FILE] Batch written successfully");

        Ok(())
    }
}
