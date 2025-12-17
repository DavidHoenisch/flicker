mod config;
mod destinations;
mod docker_tailer;
mod filter;
mod tailer;

use crate::config::Config;
use crate::destinations::{LogEntry, create_destination};
use crate::docker_tailer::DockerTailer;
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
        "Starting Flicker with {} log file(s) and {} Docker container(s)...",
        cfg.log_files.len(),
        cfg.docker_containers.len()
    );

    let mut handles = vec![];

    for log_file in cfg.log_files {
        let path = log_file.path.clone();
        let freq = log_file.polling_frequency_ms;
        let buffer_size = log_file.buffer_size;
        let flush_interval = Duration::from_millis(log_file.flush_interval_ms);
        let dest_type = log_file.destination.dest_type.clone();

        let dest = match create_destination(&log_file.destination) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to create destination for {}: {}", path, e);
                continue; // Skip this file and continue with others
            }
        };

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
                        for line in lines {
                            if filter.should_ship(&line) {
                                buffer.push(LogEntry {
                                    path: path.clone(),
                                    line,
                                });
                            }
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

                            if let Err(e) = dest.send_batch(buffer.clone()).await {
                                eprintln!("Failed to ship batch from {}: {}", path, e);
                            }

                            buffer.clear();
                            last_flush = Instant::now();
                        }
                    }
                    Err(e) => {
                        eprintln!("Error polling {}: {}", path, e);
                    }
                }
            }
            #[allow(unreachable_code)]
            Ok(())
        });

        handles.push(handle);
    }

    // Spawn tasks for Docker containers
    for docker_container in cfg.docker_containers {
        let container = docker_container.container.clone();
        let freq = docker_container.polling_frequency_ms;
        let buffer_size = docker_container.buffer_size;
        let flush_interval = Duration::from_millis(docker_container.flush_interval_ms);
        let dest_type = docker_container.destination.dest_type.clone();

        let dest = match create_destination(&docker_container.destination) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to create destination for container {}: {}", container, e);
                continue; // Skip this container and continue with others
            }
        };

        let filter = match LogFilter::new(docker_container.match_on.clone(), docker_container.exclude_on.clone()) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to create filter for container {}: {}", container, e);
                continue; // Skip this container and continue with others
            }
        };

        let handle: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let mut tailer = match DockerTailer::new() {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Failed to create Docker tailer for {}: {}", container, e);
                    return Err(e);
                }
            };
            let mut interval = time::interval(Duration::from_millis(freq));

            let mut buffer: Vec<LogEntry> = Vec::with_capacity(buffer_size);
            let mut last_flush = Instant::now();

            let filter_info = if filter.is_passthrough() {
                "no filters".to_string()
            } else {
                "with filters".to_string()
            };

            println!(
                "Tailing Docker container {} every {}ms (buffer: {} lines, flush: {}ms, {}) -> {} destination",
                container, freq, buffer_size, docker_container.flush_interval_ms, filter_info, dest_type
            );

            loop {
                interval.tick().await;

                // Poll this container for new lines
                match tailer.poll(&container).await {
                    Ok(lines) => {
                        for line in lines {
                            if filter.should_ship(&line) {
                                buffer.push(LogEntry {
                                    path: format!("docker://{}", container),
                                    line,
                                });
                            }
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
                                "Flushing {} entries from Docker container {} ({})",
                                buffer.len(),
                                container,
                                reason
                            );

                            if let Err(e) = dest.send_batch(buffer.clone()).await {
                                eprintln!("Failed to ship batch from Docker container {}: {}", container, e);
                            }

                            buffer.clear();
                            last_flush = Instant::now();
                        }
                    }
                    Err(e) => {
                        eprintln!("Error polling Docker container {}: {}", container, e);
                    }
                }
            }
            #[allow(unreachable_code)]
            Ok(())
        });

        handles.push(handle);
    }

    for handle in handles {
        match handle.await {
            Ok(_) => {} // Task completed (unreachable)
            Err(e) => eprintln!("Task panicked: {}", e),
        }
    }

    Ok(())
}
