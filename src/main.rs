mod config;
mod destination;

use crate::config::Config;
use crate::destination::{LogEntry, create_destination};
use std::time::Duration;
use tokio::time;

// #[tokio::main] starts the async runtime (like Go's scheduler)
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Load Config
    // In a real app, use clap to parse CLI args for the config path
    let cfg = Config::load("flicker.yaml").unwrap_or_else(|_| {
        // Fallback for demo purposes if file missing
        Config {
            polling_frequency_ms: 1000,
            log_paths: vec!["/var/log/syslog".to_string()],
            destination: config::DestinationConfig {
                endpoint: "http://localhost:8080".to_string(),
                api_key: None,
            },
        }
    });

    println!("Starting Flicker...");

    // 2. Create the Channel
    // tx = transmitter (send-only channel end)
    // rx = receiver (receive-only channel end)
    let (tx, mut rx) = tokio::sync::mpsc::channel::<LogEntry>(100);

    // 3. Spawn the File Watcher (Producer)
    // This is like `go func() { ... }`
    let paths = cfg.log_paths.clone();
    let freq = cfg.polling_frequency_ms;

    tokio::spawn(async move {
        // "move" keyword forces the closure to take ownership of `paths` and `tx`
        // so they exist inside this new thread.
        let mut interval = time::interval(Duration::from_millis(freq));

        loop {
            interval.tick().await;

            // FAKE INGESTION: Simulating reading lines from files
            for path in &paths {
                let entry = LogEntry {
                    path: path.clone(),
                    line: format!("Log event at {:?}", std::time::SystemTime::now()),
                };

                // Send to channel.
                if let Err(e) = tx.send(entry).await {
                    eprintln!("Receiver dropped: {}", e);
                    return; // Exit task
                }
            }
        }
    });

    // 4. The Destination Handler (Consumer)
    // We create the concrete implementation but hold it as a Trait Object
    let dest = create_destination(&cfg.destination.endpoint);

    // Loop over messages as they arrive
    while let Some(entry) = rx.recv().await {
        // Processing Logic (Filter/Regex) would go here

        // Ship it
        if let Err(e) = dest.send(entry).await {
            eprintln!("Failed to ship log: {}", e);
        }
    }

    Ok(())
}
