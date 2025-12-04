use std::collections::HashMap;
use std::fs::{File, metadata};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;

/// Tracks state for a single file being tailed
struct FileState {
    reader: BufReader<File>,
    position: u64, // Current byte offset in file
    inode: u64,    // To detect file rotation
}

/// Manages tailing multiple log files
pub struct LogTailer {
    files: HashMap<PathBuf, FileState>,
}

impl LogTailer {
    pub fn new() -> Self {
        Self {
            files: HashMap::new(),
        }
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

        #[cfg(unix)]
        let current_inode = {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        };

        #[cfg(not(unix))]
        let current_inode = 0; // Windows doesn't have inodes

        let current_size = metadata.len();

        // Check if we're already tracking this file
        if let Some(state) = self.files.get_mut(&path_buf) {
            // DESIGN CHOICE: Detect file truncation
            // If current file size < our position, the file was truncated
            // This happens during log rotation when files are cleared
            if current_size < state.position {
                eprintln!("File {} truncated, resetting position", path);
                state.position = 0;
                state.reader.seek(SeekFrom::Start(0))?;
            }

            // DESIGN CHOICE: Detect file rotation
            // If inode changed, file was rotated (renamed and new file created)
            // We need to reopen the file
            #[cfg(unix)]
            if current_inode != state.inode {
                eprintln!("File {} rotated, reopening", path);
                self.files.remove(&path_buf);
                return self.poll(path); // Recursive call to reopen
            }

            // Seek to last position (in case file handle was disturbed)
            state.reader.seek(SeekFrom::Start(state.position))?;

            // DESIGN CHOICE: Read line-by-line instead of bulk read
            // This ensures we always send complete lines, never partial
            let mut line = String::new();
            while state.reader.read_line(&mut line)? > 0 {
                // Remove trailing newline
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }

                lines.push(line.clone());
                line.clear();
            }

            // Update position after reading
            state.position = state.reader.stream_position()?;
        } else {
            // First time seeing this file, open it
            let file = File::open(&path_buf)?;
            let mut reader = BufReader::new(file);

            // DESIGN CHOICE: Start at end of file for new files
            // We don't want to ship the entire existing log on startup
            // Only ship new lines that arrive after we start
            let position = reader.seek(SeekFrom::End(0))?;

            self.files.insert(
                path_buf.clone(),
                FileState {
                    reader,
                    position,
                    inode: current_inode,
                },
            );

            eprintln!("Now tailing {} from position {}", path, position);
        }

        Ok(lines)
    }
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
