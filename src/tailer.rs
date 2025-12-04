use std::collections::HashMap;
use std::fs::{File, metadata};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;

/// Tracks state for a single file being tailed
struct FileState {
    reader: BufReader<File>,
    position: u64,       // Current byte offset in file
    inode: u64,          // To detect file rotation
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
