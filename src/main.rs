mod api_tailer;
mod config;
mod destinations;
mod docker_tailer;
mod filter;
mod masking;
mod pipeline;
mod registry;
mod registry_storage;
mod retry_queue;
mod tailer;

use crate::api_tailer::ApiTailer;
use crate::config::{Config, DestinationConfig};
use crate::destinations::create_destination;
use crate::docker_tailer::DockerTailer;
use crate::filter::LogFilter;
use crate::masking::{MaskingConfig, MaskingEngine};
use crate::pipeline::{ApiSource, DockerSource, FileSource, PipelineConfig, run_source};
use crate::registry::{Registry, RegistryUpdate, registry_writer_task};
use crate::registry_storage::{FileSystemStorage, RegistryStorage, S3Storage};
use crate::retry_queue::RetryConfig;
use crate::tailer::LogTailer;
use clap::Parser;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

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
    let registry_sender = registry_tx.as_ref().map(|(tx, _)| tx.clone());
    let registry = registry_tx.as_ref().map(|(_, r)| r);

    for log_file in cfg.log_files {
        let path = log_file.path.clone();
        let pipeline = match build_pipeline(
            &path,
            &SourceSettings {
                polling_frequency_ms: log_file.polling_frequency_ms,
                buffer_size: log_file.buffer_size,
                flush_interval_ms: log_file.flush_interval_ms,
                destination: &log_file.destination,
                match_on: &log_file.match_on,
                exclude_on: &log_file.exclude_on,
                masking: &log_file.masking,
            },
            &retry_config,
            &registry_sender,
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to set up {}: {}", path, e);
                continue; // Skip this file and continue with others
            }
        };

        let mut tailer = LogTailer::new();
        if let Some(pos) = registry.and_then(|r| r.get_file_position(&path)) {
            tailer.set_initial_position(path.clone(), pos.position, pos.inode);
        }

        handles.push(tokio::spawn(run_source(
            FileSource { tailer, path },
            pipeline,
        )));
    }

    for docker_container in cfg.docker_containers {
        let container = docker_container.container.clone();
        let label = format!("Docker container {}", container);
        let pipeline = match build_pipeline(
            &label,
            &SourceSettings {
                polling_frequency_ms: docker_container.polling_frequency_ms,
                buffer_size: docker_container.buffer_size,
                flush_interval_ms: docker_container.flush_interval_ms,
                destination: &docker_container.destination,
                match_on: &docker_container.match_on,
                exclude_on: &docker_container.exclude_on,
                masking: &docker_container.masking,
            },
            &retry_config,
            &registry_sender,
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to set up {}: {}", label, e);
                continue; // Skip this container and continue with others
            }
        };

        let mut tailer = match DockerTailer::new() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Failed to create Docker tailer for {}: {}", container, e);
                continue;
            }
        };
        if let Some(pos) = registry.and_then(|r| r.get_container_position(&container)) {
            tailer.set_initial_timestamp(container.clone(), pos.last_timestamp);
        }

        handles.push(tokio::spawn(run_source(
            DockerSource { tailer, container },
            pipeline,
        )));
    }

    for api_source in cfg.api_sources {
        let name = api_source.name.clone();
        let label = format!("API source {}", name);
        let pipeline = match build_pipeline(
            &label,
            &SourceSettings {
                polling_frequency_ms: api_source.polling_frequency_ms,
                buffer_size: api_source.buffer_size,
                flush_interval_ms: api_source.flush_interval_ms,
                destination: &api_source.destination,
                match_on: &api_source.match_on,
                exclude_on: &api_source.exclude_on,
                masking: &api_source.masking,
            },
            &retry_config,
            &registry_sender,
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to set up {}: {}", label, e);
                continue; // Skip this API source and continue with others
            }
        };

        let mut tailer = match ApiTailer::new() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Failed to create API tailer for {}: {}", name, e);
                continue;
            }
        };
        if let Some(pos) = registry.and_then(|r| r.get_api_position(&name)) {
            tailer.set_initial_position(name.clone(), pos.last_timestamp, pos.cursor);
        }

        handles.push(tokio::spawn(run_source(
            ApiSource {
                tailer,
                config: api_source,
            },
            pipeline,
        )));
    }

    for handle in handles {
        match handle.await {
            Ok(_) => {} // Task completed (unreachable)
            Err(e) => eprintln!("Task panicked: {}", e),
        }
    }

    Ok(())
}

/// Per-source settings shared by all source types
struct SourceSettings<'a> {
    polling_frequency_ms: u64,
    buffer_size: usize,
    flush_interval_ms: u64,
    destination: &'a DestinationConfig,
    match_on: &'a [String],
    exclude_on: &'a [String],
    masking: &'a MaskingConfig,
}

/// Build destination, filter and masking for a source
fn build_pipeline(
    label: &str,
    settings: &SourceSettings,
    retry: &RetryConfig,
    registry_tx: &Option<mpsc::UnboundedSender<RegistryUpdate>>,
) -> anyhow::Result<PipelineConfig> {
    let destination = create_destination(settings.destination)
        .map_err(|e| anyhow::anyhow!("destination: {}", e))?;
    let filter = LogFilter::new(settings.match_on.to_vec(), settings.exclude_on.to_vec())
        .map_err(|e| anyhow::anyhow!("filter: {}", e))?;
    let masking =
        MaskingEngine::new(settings.masking).map_err(|e| anyhow::anyhow!("masking: {}", e))?;
    if masking.is_some() {
        println!("Masking enabled for {}", label);
    }

    println!(
        "Tailing {} every {}ms (buffer: {} lines, flush: {}ms, {}) -> {} destination",
        label,
        settings.polling_frequency_ms,
        settings.buffer_size,
        settings.flush_interval_ms,
        if filter.is_passthrough() {
            "no filters"
        } else {
            "with filters"
        },
        settings.destination.dest_type
    );

    Ok(PipelineConfig {
        polling_frequency: Duration::from_millis(settings.polling_frequency_ms),
        buffer_size: settings.buffer_size,
        flush_interval: Duration::from_millis(settings.flush_interval_ms),
        filter,
        masking,
        destination,
        retry: retry.clone(),
        registry_tx: registry_tx.clone(),
    })
}
