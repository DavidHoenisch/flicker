mod api_tailer;
mod config;
mod destinations;
mod docker_tailer;
mod filter;
mod masking;
mod registry;
mod registry_storage;
mod retry_queue;
mod tailer;

use crate::api_tailer::ApiTailer;
use crate::config::Config;
use crate::destinations::{LogEntry, create_destination};
use crate::docker_tailer::DockerTailer;
use crate::filter::LogFilter;
use crate::masking::MaskingEngine;
use crate::registry::{Registry, registry_writer_task};
use crate::registry_storage::{FileSystemStorage, RegistryStorage, S3Storage};
use crate::retry_queue::RetryQueue;
use crate::tailer::LogTailer;
use clap::Parser;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "flicker.yaml")]
    config: String,

    /// Enable registry tracking to persist file positions across restarts
    #[arg(long)]
    track: bool,

    /// Path or S3 URL to the registry file (only used with --track)
    /// Examples: ".flicker-registry.json" or "s3://bucket/path/to/registry.json"
    #[arg(long, default_value = ".flicker-registry.json")]
    registry_file: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let cfg = Config::load(&args.config)?;

    println!(
        "Starting Flicker with {} log file(s), {} Docker container(s), and {} API source(s)...",
        cfg.log_files.len(),
        cfg.docker_containers.len(),
        cfg.api_sources.len()
    );

    // Initialize registry if tracking is enabled
    let registry_tx = if args.track {
        // Detect storage backend based on registry_file value
        let storage: Arc<dyn RegistryStorage> = if args.registry_file.starts_with("s3://") {
            println!(
                "Registry tracking enabled, using S3: {}",
                args.registry_file
            );
            let (bucket, key) = S3Storage::parse_s3_url(&args.registry_file)?;
            Arc::new(S3Storage::new(bucket, key).await?)
        } else {
            println!(
                "Registry tracking enabled, using file: {}",
                args.registry_file
            );
            Arc::new(FileSystemStorage::new(&args.registry_file))
        };

        // Load initial registry from storage
        let registry = match storage.load().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "Failed to load registry: {}. Starting with empty registry.",
                    e
                );
                Registry::new()
            }
        };

        let (tx, rx) = mpsc::unbounded_channel();

        // Spawn registry writer task
        let storage_clone = storage.clone();
        tokio::spawn(async move {
            registry_writer_task(storage_clone, rx).await;
        });

        Some((tx, registry))
    } else {
        None
    };

    let mut handles = vec![];

    let retry_config = cfg.retry.to_retry_config();

    for log_file in cfg.log_files {
        let path = log_file.path.clone();
        let freq = log_file.polling_frequency_ms;
        let buffer_size = log_file.buffer_size;
        let flush_interval = Duration::from_millis(log_file.flush_interval_ms);
        let dest_type = log_file.destination.dest_type.clone();
        let retry_cfg = retry_config.clone();

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

        let masking = match MaskingEngine::new(&log_file.masking) {
            Ok(m) => {
                if m.is_some() {
                    println!("Masking enabled for {}", path);
                } else {
                    println!("Masking config present but no active rules for {}", path);
                }
                m
            }
            Err(e) => {
                eprintln!("Failed to create masking engine for {}: {}", path, e);
                continue; // Skip this file and continue with others
            }
        };

        // Get registry data if tracking is enabled
        let (registry_sender, initial_position) = if let Some((ref tx, ref registry)) = registry_tx
        {
            let pos = registry.get_file_position(&path);
            (Some(tx.clone()), pos)
        } else {
            (None, None)
        };

        let handle: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let mut tailer = LogTailer::new();
            let mut retry_queue = RetryQueue::new(retry_cfg);

            // Set up registry tracking if enabled
            if let Some(tx) = registry_sender {
                tailer.set_registry_sender(tx);
            }
            if let Some(pos) = initial_position {
                tailer.set_initial_position(path.clone(), pos.position, pos.inode);
            }
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

                // First, try to send any batches from the retry queue
                let ready_batches = retry_queue.get_ready_batches();
                for (entries, retry_count) in ready_batches {
                    println!(
                        "[Retry] Attempting to resend batch of {} entries from {} (retry {})",
                        entries.len(),
                        path,
                        retry_count + 1
                    );

                    if let Err(e) = dest.send_batch(entries.clone()).await {
                        eprintln!("[Retry] Failed to resend batch from {}: {}", path, e);
                        retry_queue.retry_failed(entries, retry_count);
                    } else {
                        println!("[Retry] Successfully resent batch from {}", path);
                    }
                }

                // Poll this file for new lines
                match tailer.poll(&path) {
                    Ok(lines) => {
                        let mut lines_read = 0;
                        let mut lines_shipped = 0;
                        for line in lines {
                            lines_read += 1;
                            if filter.should_ship(&line) {
                                lines_shipped += 1;
                                let masked_line = masking
                                    .as_ref()
                                    .map_or_else(|| line.clone(), |m| m.apply(&line));
                                buffer.push(LogEntry {
                                    path: path.clone(),
                                    line: masked_line,
                                });
                            }
                        }

                        if lines_read > 0 {
                            println!(
                                "[{}] Read {} lines, shipped {} (buffer: {})",
                                path,
                                lines_read,
                                lines_shipped,
                                buffer.len()
                            );
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
                                // Add failed batch to retry queue
                                retry_queue.add_failed_batch(buffer.clone(), 0);
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
        let retry_cfg = retry_config.clone();

        let dest = match create_destination(&docker_container.destination) {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "Failed to create destination for container {}: {}",
                    container, e
                );
                continue; // Skip this container and continue with others
            }
        };

        let filter = match LogFilter::new(
            docker_container.match_on.clone(),
            docker_container.exclude_on.clone(),
        ) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to create filter for container {}: {}", container, e);
                continue; // Skip this container and continue with others
            }
        };

        let masking = match MaskingEngine::new(&docker_container.masking) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "Failed to create masking engine for container {}: {}",
                    container, e
                );
                continue; // Skip this container and continue with others
            }
        };

        // Get registry data if tracking is enabled
        let (registry_sender, initial_timestamp) = if let Some((ref tx, ref registry)) = registry_tx
        {
            let pos = registry.get_container_position(&container);
            (Some(tx.clone()), pos.map(|p| p.last_timestamp))
        } else {
            (None, None)
        };

        let handle: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let mut tailer = match DockerTailer::new() {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Failed to create Docker tailer for {}: {}", container, e);
                    return Err(e);
                }
            };
            let mut retry_queue = RetryQueue::new(retry_cfg);

            // Set up registry tracking if enabled
            if let Some(tx) = registry_sender {
                tailer.set_registry_sender(tx);
            }
            if let Some(ts) = initial_timestamp {
                tailer.set_initial_timestamp(container.clone(), ts);
            }
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
                container,
                freq,
                buffer_size,
                docker_container.flush_interval_ms,
                filter_info,
                dest_type
            );

            loop {
                interval.tick().await;

                // First, try to send any batches from the retry queue
                let ready_batches = retry_queue.get_ready_batches();
                for (entries, retry_count) in ready_batches {
                    println!(
                        "[Retry] Attempting to resend batch of {} entries from Docker container {} (retry {})",
                        entries.len(),
                        container,
                        retry_count + 1
                    );

                    if let Err(e) = dest.send_batch(entries.clone()).await {
                        eprintln!(
                            "[Retry] Failed to resend batch from Docker container {}: {}",
                            container, e
                        );
                        retry_queue.retry_failed(entries, retry_count);
                    } else {
                        println!(
                            "[Retry] Successfully resent batch from Docker container {}",
                            container
                        );
                    }
                }

                // Poll this container for new lines
                match tailer.poll(&container).await {
                    Ok(lines) => {
                        for line in lines {
                            if filter.should_ship(&line) {
                                let masked_line = masking
                                    .as_ref()
                                    .map_or_else(|| line.clone(), |m| m.apply(&line));
                                buffer.push(LogEntry {
                                    path: format!("docker://{}", container),
                                    line: masked_line,
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
                                eprintln!(
                                    "Failed to ship batch from Docker container {}: {}",
                                    container, e
                                );
                                // Add failed batch to retry queue
                                retry_queue.add_failed_batch(buffer.clone(), 0);
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

    // Spawn tasks for API sources
    for api_source in cfg.api_sources {
        let name = api_source.name.clone();
        let freq = api_source.polling_frequency_ms;
        let buffer_size = api_source.buffer_size;
        let flush_interval = Duration::from_millis(api_source.flush_interval_ms);
        let dest_type = api_source.destination.dest_type.clone();
        let retry_cfg = retry_config.clone();
        let api_config = api_source.clone();

        let dest = match create_destination(&api_source.destination) {
            Ok(d) => d,
            Err(e) => {
                eprintln!(
                    "Failed to create destination for API source {}: {}",
                    name, e
                );
                continue; // Skip this API source and continue with others
            }
        };

        let filter =
            match LogFilter::new(api_source.match_on.clone(), api_source.exclude_on.clone()) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Failed to create filter for API source {}: {}", name, e);
                    continue; // Skip this API source and continue with others
                }
            };

        let masking = match MaskingEngine::new(&api_source.masking) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "Failed to create masking engine for API source {}: {}",
                    name, e
                );
                continue; // Skip this API source and continue with others
            }
        };

        // Get registry data if tracking is enabled
        let (registry_sender, initial_position) = if let Some((ref tx, ref registry)) = registry_tx
        {
            let pos = registry.get_api_position(&name);
            (Some(tx.clone()), pos.map(|p| (p.last_timestamp, p.cursor)))
        } else {
            (None, None)
        };

        let handle: tokio::task::JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
            let mut tailer = match ApiTailer::new() {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("Failed to create API tailer for {}: {}", name, e);
                    return Err(e);
                }
            };

            let mut retry_queue = RetryQueue::new(retry_cfg);

            // Set up registry tracking if enabled
            if let Some(tx) = registry_sender {
                tailer.set_registry_sender(tx);
            }
            if let Some((ts, cursor)) = initial_position {
                tailer.set_initial_position(name.clone(), ts, cursor);
            }

            let mut interval = time::interval(Duration::from_millis(freq));

            let mut buffer: Vec<LogEntry> = Vec::with_capacity(buffer_size);
            let mut last_flush = Instant::now();

            let filter_info = if filter.is_passthrough() {
                "no filters".to_string()
            } else {
                "with filters".to_string()
            };

            println!(
                "Tailing API source {} every {}ms (buffer: {} lines, flush: {}ms, {}) -> {} destination",
                name, freq, buffer_size, api_config.flush_interval_ms, filter_info, dest_type
            );

            loop {
                interval.tick().await;

                // First, try to send any batches from the retry queue
                let ready_batches = retry_queue.get_ready_batches();
                for (entries, retry_count) in ready_batches {
                    println!(
                        "[Retry] Attempting to resend batch of {} entries from API source {} (retry {})",
                        entries.len(),
                        name,
                        retry_count + 1
                    );

                    if let Err(e) = dest.send_batch(entries.clone()).await {
                        eprintln!(
                            "[Retry] Failed to resend batch from API source {}: {}",
                            name, e
                        );
                        retry_queue.retry_failed(entries, retry_count);
                    } else {
                        println!("[Retry] Successfully resent batch from API source {}", name);
                    }
                }

                // Poll this API source for new entries
                match tailer.poll(&api_config).await {
                    Ok(lines) => {
                        for line in lines {
                            if filter.should_ship(&line) {
                                let masked_line = masking
                                    .as_ref()
                                    .map_or_else(|| line.clone(), |m| m.apply(&line));
                                buffer.push(LogEntry {
                                    path: format!("api://{}", name),
                                    line: masked_line,
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
                                "Flushing {} entries from API source {} ({})",
                                buffer.len(),
                                name,
                                reason
                            );

                            if let Err(e) = dest.send_batch(buffer.clone()).await {
                                eprintln!("Failed to ship batch from API source {}: {}", name, e);
                                // Add failed batch to retry queue
                                retry_queue.add_failed_batch(buffer.clone(), 0);
                            }

                            buffer.clear();
                            last_flush = Instant::now();
                        }
                    }
                    Err(e) => {
                        eprintln!("Error polling API source {}: {}", name, e);
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
