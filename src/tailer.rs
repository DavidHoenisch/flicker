use crate::registry::RegistryUpdate;
use std::collections::HashMap;
use std::fs::{File, metadata};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Longest line held in memory; longer lines are shipped in chunks of this size
const MAX_LINE_BYTES: usize = 1024 * 1024;
/// How long an unterminated last line may sit unchanged before it is shipped anyway
const PARTIAL_LINE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bytes before the read position compared each poll to detect truncate-and-refill
const FINGERPRINT_BYTES: u64 = 64;

/// Tracks state for a single file being tailed
struct FileState {
    reader: BufReader<File>,
    position: u64, // Current byte offset in file
    inode: u64,    // To detect file rotation
    /// Unterminated line at EOF: (its start offset, when first seen)
    partial: Option<(u64, Instant)>,
    /// Up to FINGERPRINT_BYTES bytes just before `position`, to detect rewrites
    fingerprint: Vec<u8>,
}

/// Manages tailing multiple log files
pub struct LogTailer {
    files: HashMap<PathBuf, FileState>,
    initial_positions: HashMap<String, (u64, u64)>, // path -> (position, inode)
    partial_line_timeout: Duration,
}

impl LogTailer {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
            initial_positions: HashMap::new(),
            partial_line_timeout: PARTIAL_LINE_TIMEOUT,
        }
    }

    /// Current read position for a file, for persisting to the registry.
    /// DESIGN CHOICE: The caller decides when to persist (after delivery), not the tailer.
    pub fn checkpoint(&self, path: &str) -> Option<RegistryUpdate> {
        self.files
            .get(&PathBuf::from(path))
            .map(|state| RegistryUpdate::UpdateFile {
                path: path.to_string(),
                position: state.position,
                inode: state.inode,
            })
    }

    /// Set initial position for a file (loaded from registry)
    pub fn set_initial_position(&mut self, path: String, position: u64, inode: u64) {
        self.initial_positions.insert(path, (position, inode));
    }

    /// Read new lines from a log file since last poll
    /// Returns a Vec of new lines found
    pub fn poll(&mut self, path: &str) -> anyhow::Result<Vec<String>> {
        let path_buf = PathBuf::from(path);
        let mut lines = Vec::new();

        // Try to get metadata - file might not exist yet
        let metadata = match metadata(&path_buf) {
            Ok(m) => m,
            Err(_) => {
                // File doesn't exist, skip for now
                // We'll try again next poll
                return Ok(lines);
            }
        };

        let current_inode = file_identity(&metadata);
        let current_size = metadata.len();
        let partial_line_timeout = self.partial_line_timeout;

        // DESIGN CHOICE: Detect file rotation
        // If the identity changed, the file was rotated (renamed and a new file created).
        // Drain whatever was written to the old file before rotation, then read the new
        // file from the start so nothing written since rotation is skipped.
        if let Some(state) = self.files.get_mut(&path_buf)
            && current_inode != state.inode
        {
            eprintln!("File {} rotated, draining old file and reopening", path);
            let drained = state
                .reader
                .seek(SeekFrom::Start(state.position))
                .and_then(|_| state.read_lines(&mut lines, Duration::ZERO));
            if let Err(e) = drained {
                eprintln!("Failed to drain rotated file {}: {}", path, e);
            }
            self.files.remove(&path_buf);
            self.open(path, 0, current_inode)?;
        }

        // Check if we're already tracking this file
        if let Some(state) = self.files.get_mut(&path_buf) {
            // DESIGN CHOICE: Detect file truncation
            // A file smaller than our position, or whose bytes just before our position
            // changed (truncated and refilled between polls), is read again from the start.
            if current_size < state.position || !state.seek_to_position()? {
                eprintln!("File {} truncated, resetting position", path);
                state.position = 0;
                state.partial = None;
                state.fingerprint.clear();
                state.reader.seek(SeekFrom::Start(0))?;
            }

            state.read_lines(&mut lines, partial_line_timeout)?;
        } else {
            // First time seeing this file
            // Check if we have an initial position from registry
            let position = if let Some((saved_pos, saved_inode)) = self.initial_positions.get(path)
            {
                // Use saved position if inode matches (file hasn't been rotated)
                if *saved_inode == current_inode && *saved_pos <= current_size {
                    eprintln!("Resuming {} from registry position {}", path, saved_pos);
                    Some(*saved_pos)
                } else {
                    // Rotated (or truncated) while we were stopped: everything in the
                    // current file is new to us.
                    eprintln!(
                        "File {} changed since last run (rotated or truncated), starting at beginning",
                        path
                    );
                    Some(0)
                }
            } else {
                // DESIGN CHOICE: Start at end of file for new files
                // We don't want to ship the entire existing log on startup
                // Only ship new lines that arrive after we start
                eprintln!("Now tailing {} from position end", path);
                None
            };
            let position = position.unwrap_or(current_size);
            self.open(path, position, current_inode)?;
        }

        Ok(lines)
    }

    /// Start tracking a file at the given offset
    fn open(&mut self, path: &str, position: u64, inode: u64) -> anyhow::Result<()> {
        let mut reader = BufReader::new(File::open(path)?);
        reader.seek(SeekFrom::Start(position))?;
        let mut state = FileState {
            reader,
            position,
            inode,
            partial: None,
            fingerprint: Vec::new(),
        };
        state.record_fingerprint()?;
        self.files.insert(PathBuf::from(path), state);
        Ok(())
    }
}

impl FileState {
    /// Seek to `position`, verifying the bytes just before it are unchanged.
    /// Returns false if they differ (the file was truncated and rewritten).
    fn seek_to_position(&mut self) -> std::io::Result<bool> {
        let len = self.fingerprint.len() as u64;
        self.reader.seek(SeekFrom::Start(self.position - len))?;
        let mut current = vec![0u8; self.fingerprint.len()];
        if self.reader.read_exact(&mut current).is_err() {
            return Ok(false);
        }
        Ok(current == self.fingerprint)
    }

    /// Remember the bytes just before `position` (reader must be at `position`)
    fn record_fingerprint(&mut self) -> std::io::Result<()> {
        let len = self.position.min(FINGERPRINT_BYTES);
        self.fingerprint = vec![0u8; len as usize];
        self.reader.seek(SeekFrom::Start(self.position - len))?;
        self.reader.read_exact(&mut self.fingerprint)
    }

    /// Read complete lines from the current reader position to EOF.
    /// `partial_line_timeout` of zero ships a trailing unterminated line immediately.
    fn read_lines(
        &mut self,
        lines: &mut Vec<String>,
        partial_line_timeout: Duration,
    ) -> std::io::Result<()> {
        // DESIGN CHOICE: Read raw bytes and decode lossily. read_line() errors on
        // invalid UTF-8, which would leave position stuck before the bad byte forever.
        let start = self.position;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            let n = (&mut self.reader)
                .take(MAX_LINE_BYTES as u64)
                .read_until(b'\n', &mut buf)?;
            if n == 0 {
                break;
            }

            // DESIGN CHOICE: Only ship complete lines. If the writer is mid-line,
            // leave the position at the line start and wait for the rest. Ship it
            // anyway if it stays unterminated past the timeout (e.g. a final line
            // with no newline) or reaches MAX_LINE_BYTES (bounds memory).
            if !buf.ends_with(b"\n") && n < MAX_LINE_BYTES && !partial_line_timeout.is_zero() {
                match self.partial {
                    Some((at, since))
                        if at == self.position && since.elapsed() >= partial_line_timeout => {}
                    Some((at, _)) if at == self.position => break,
                    _ => {
                        self.partial = Some((self.position, Instant::now()));
                        break;
                    }
                }
            }
            self.partial = None;
            self.position += n as u64;

            // Remove trailing newline
            if buf.ends_with(b"\n") {
                buf.pop();
                if buf.ends_with(b"\r") {
                    buf.pop();
                }
            }

            lines.push(String::from_utf8_lossy(&buf).into_owned());
        }

        if self.position != start {
            self.record_fingerprint()?;
        }
        Ok(())
    }
}

/// Stable identity for a file, used to detect rotation
#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.ino()
}

/// Stable identity for a file, used to detect rotation.
/// DESIGN CHOICE: No inodes off unix, so use creation time. The fingerprint check
/// in seek_to_position catches rotations this misses (e.g. Windows tunneling).
#[cfg(not(unix))]
fn file_identity(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_tailer_reads_new_lines() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        // Write initial content
        writeln!(file, "Line 1").unwrap();
        writeln!(file, "Line 2").unwrap();
        file.flush().unwrap();

        let mut tailer = LogTailer::new();

        // First poll: should start at end, read nothing
        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 0);

        // Add new lines
        writeln!(file, "Line 3").unwrap();
        writeln!(file, "Line 4").unwrap();
        file.flush().unwrap();

        // Second poll: should read new lines
        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "Line 3");
        assert_eq!(lines[1], "Line 4");

        // Third poll: no new lines
        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn test_tailer_handles_empty_file() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        let mut tailer = LogTailer::new();

        // Poll empty file
        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn test_tailer_handles_missing_file() {
        let mut tailer = LogTailer::new();

        // Poll non-existent file - should return empty, not error
        let lines = tailer.poll("/tmp/does_not_exist_12345.log").unwrap();
        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn test_tailer_detects_truncation() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        // Write initial content
        writeln!(file, "Line 1").unwrap();
        writeln!(file, "Line 2").unwrap();
        file.flush().unwrap();

        let mut tailer = LogTailer::new();

        // First poll
        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 0);

        // Add new lines
        writeln!(file, "Line 3").unwrap();
        file.flush().unwrap();

        // Read new line
        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "Line 3");

        // Truncate file (simulate log rotation)
        file.as_file_mut().set_len(0).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        writeln!(file, "New Line 1").unwrap();
        file.flush().unwrap();

        // Should detect truncation and reset
        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "New Line 1");
    }

    #[test]
    fn test_tailer_detects_truncate_and_refill() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        let mut tailer = LogTailer::new();
        tailer.poll(&path).unwrap();
        writeln!(file, "original line").unwrap();
        file.flush().unwrap();
        assert_eq!(tailer.poll(&path).unwrap(), vec!["original line"]);

        // Truncate and write more than before, all between polls
        file.as_file_mut().set_len(0).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        writeln!(file, "replacement content that is longer").unwrap();
        file.flush().unwrap();

        assert_eq!(
            tailer.poll(&path).unwrap(),
            vec!["replacement content that is longer"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_tailer_rotation_drains_old_and_reads_new_from_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        let path_str = path.to_str().unwrap().to_string();
        let mut file = File::create(&path).unwrap();

        let mut tailer = LogTailer::new();
        tailer.poll(&path_str).unwrap();
        writeln!(file, "before").unwrap();
        assert_eq!(tailer.poll(&path_str).unwrap(), vec!["before"]);

        // Written after the last poll, just before rotation
        writeln!(file, "late old").unwrap();
        write!(file, "unterminated old").unwrap();
        std::fs::rename(&path, dir.path().join("app.log.1")).unwrap();

        // New file gets lines before the next poll
        let mut new_file = File::create(&path).unwrap();
        writeln!(new_file, "first new").unwrap();

        assert_eq!(
            tailer.poll(&path_str).unwrap(),
            vec!["late old", "unterminated old", "first new"]
        );

        writeln!(new_file, "second new").unwrap();
        assert_eq!(tailer.poll(&path_str).unwrap(), vec!["second new"]);
    }

    #[test]
    fn test_tailer_resume_after_rotation_starts_at_beginning() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();
        writeln!(file, "new file line").unwrap();
        file.flush().unwrap();

        let mut tailer = LogTailer::new();
        // Registry says a different file, far past this one's end
        tailer.set_initial_position(path.clone(), 10_000, u64::MAX);
        tailer.poll(&path).unwrap();
        writeln!(file, "next").unwrap();
        file.flush().unwrap();
        assert_eq!(tailer.poll(&path).unwrap(), vec!["new file line", "next"]);
    }

    #[test]
    fn test_tailer_multiple_files() {
        let mut file1 = NamedTempFile::new().unwrap();
        let mut file2 = NamedTempFile::new().unwrap();
        let path1 = file1.path().to_str().unwrap().to_string();
        let path2 = file2.path().to_str().unwrap().to_string();

        let mut tailer = LogTailer::new();

        // Initialize both files
        tailer.poll(&path1).unwrap();
        tailer.poll(&path2).unwrap();

        // Write to first file
        writeln!(file1, "File1 Line1").unwrap();
        file1.flush().unwrap();

        // Write to second file
        writeln!(file2, "File2 Line1").unwrap();
        file2.flush().unwrap();

        // Poll both - should get independent results
        let lines1 = tailer.poll(&path1).unwrap();
        let lines2 = tailer.poll(&path2).unwrap();

        assert_eq!(lines1.len(), 1);
        assert_eq!(lines1[0], "File1 Line1");
        assert_eq!(lines2.len(), 1);
        assert_eq!(lines2[0], "File2 Line1");
    }

    #[test]
    fn test_tailer_waits_for_partial_line() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        let mut tailer = LogTailer::new();
        tailer.poll(&path).unwrap();

        write!(file, "full line\nhalf a li").unwrap();
        file.flush().unwrap();
        assert_eq!(tailer.poll(&path).unwrap(), vec!["full line"]);
        assert!(tailer.poll(&path).unwrap().is_empty());

        writeln!(file, "ne").unwrap();
        file.flush().unwrap();
        assert_eq!(tailer.poll(&path).unwrap(), vec!["half a line"]);
    }

    #[test]
    fn test_tailer_ships_stale_partial_line_after_timeout() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        let mut tailer = LogTailer::new();
        tailer.partial_line_timeout = Duration::from_millis(20);
        tailer.poll(&path).unwrap();

        write!(file, "no newline").unwrap();
        file.flush().unwrap();
        assert!(tailer.poll(&path).unwrap().is_empty());
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(tailer.poll(&path).unwrap(), vec!["no newline"]);

        // Position moved past it
        writeln!(file).unwrap();
        writeln!(file, "next").unwrap();
        file.flush().unwrap();
        assert_eq!(tailer.poll(&path).unwrap(), vec!["", "next"]);
    }

    #[test]
    fn test_tailer_splits_overlong_line() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        let mut tailer = LogTailer::new();
        tailer.poll(&path).unwrap();

        let long = "x".repeat(MAX_LINE_BYTES + 10);
        writeln!(file, "{}", long).unwrap();
        file.flush().unwrap();
        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
        assert_eq!(lines[1].len(), 10);
    }

    #[test]
    fn test_tailer_handles_invalid_utf8() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        let mut tailer = LogTailer::new();
        tailer.poll(&path).unwrap();

        file.write_all(b"good1\nbad\xff\ngood2\n").unwrap();
        file.flush().unwrap();

        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "good1");
        assert_eq!(lines[1], "bad\u{FFFD}");
        assert_eq!(lines[2], "good2");

        // Position advanced past the bad byte
        writeln!(file, "good3").unwrap();
        file.flush().unwrap();
        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines, vec!["good3"]);
    }

    #[test]
    fn test_tailer_preserves_line_content() {
        let mut file = NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();

        let mut tailer = LogTailer::new();
        tailer.poll(&path).unwrap();

        // Write lines with special characters
        writeln!(file, "Line with spaces  ").unwrap();
        writeln!(file, "Line with\ttabs").unwrap();
        writeln!(file, "Line with \"quotes\"").unwrap();
        file.flush().unwrap();

        let lines = tailer.poll(&path).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Line with spaces  ");
        assert_eq!(lines[1], "Line with\ttabs");
        assert_eq!(lines[2], "Line with \"quotes\"");
    }
}
