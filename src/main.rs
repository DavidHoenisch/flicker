mod config;
mod destinations;
mod filter;
mod tailer;

use crate::config::Config;
use crate::destinations::{LogEntry, create_destination};
use crate::filter::LogFilter;
use crate::tailer::LogTailer;
use clap::Parser;
use std::time::{Duration, Instant};
use tokio::time;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "flicker.yaml")]
    config: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cfg = Config::load(&args.config)?;

    println!(
        "Starting Flicker with {} log file(s)...",
        cfg.log_files.len()
    );

    let mut handles = vec![];

    for log_file in cfg.log_files {
        let path = log_file.path.clone();
        let freq = log_file.polling_frequency_ms;
        let buffer_size = log_file.buffer_size;
        let flush_interval = Duration::from_millis(log_file.flush_interval_ms);
        let dest_type = log_file.destination.dest_type.clone();

        // Create destination from config
        let dest = match create_destination(&log_file.destination) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to create destination for {}: {}", path, e);
                continue; // Skip this file and continue with others
            }
        };

        // Create filter from config
        let filter = match LogFilter::new(log_file.match_on.clone(), log_file.exclude_on.clone()) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to create filter for {}: {}", path, e);
                continue; // Skip this file and continue with others
            }
        };

        let handle: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let mut tailer = LogTailer::new();
            let mut interval = time::interval(Duration::from_millis(freq));

            let mut buffer: Vec<LogEntry> = Vec::with_capacity(buffer_size);
            let mut last_flush = Instant::now();

            let filter_info = if filter.is_passthrough() {
                "no filters".to_string()
            } else {
                "with filters".to_string()
            };

            println!(
                "Tailing {} every {}ms (buffer: {} lines, flush: {}ms, {}) -> {} destination",
                path, freq, buffer_size, log_file.flush_interval_ms, filter_info, dest_type
            );

            loop {
                interval.tick().await;

                // Poll this file for new lines
                match tailer.poll(&path) {
                    Ok(lines) => {
                        // Apply filter and add matching lines to buffer
                        // DESIGN CHOICE: Filter before buffering
                        // This keeps buffer size accurate and avoids buffering
                        // lines that will never be shipped
                        for line in lines {
                            // Check if line passes filters
                            if filter.should_ship(&line) {
                                buffer.push(LogEntry {
                                    path: path.clone(),
                                    line,
                                });
                            }
                            // If line doesn't pass filter, it's silently dropped
                        }

                        let buffer_full = buffer.len() >= buffer_size;
                        let time_elapsed = last_flush.elapsed() >= flush_interval;

                        if buffer_full || (time_elapsed && !buffer.is_empty()) {
                            let reason = if buffer_full {
                                "buffer full"
                            } else {
                                "time elapsed"
                            };
                            println!(
                                "Flushing {} entries from {} ({})",
                                buffer.len(),
                                path,
                                reason
                            );

                            // Send batch to destination
                            if let Err(e) = dest.send_batch(buffer.clone()).await {
                                eprintln!("Failed to ship batch from {}: {}", path, e);
                            }

                            // Clear buffer and reset timer
                            buffer.clear();
                            last_flush = Instant::now();
                        }
                    }
                    Err(e) => {
                        eprintln!("Error polling {}: {}", path, e);
                        // Continue polling, don't crash
                    }
                }
            }
            #[allow(unreachable_code)]
            Ok(())
        });

        handles.push(handle);
    }

    for handle in handles {
        // Tasks run infinite loops and never return naturally
        match handle.await {
            Ok(_) => {} // Task completed (unreachable)
            Err(e) => eprintln!("Task panicked: {}", e),
        }
    }

    Ok(())
}
