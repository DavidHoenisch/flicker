use anyhow::Result;
use async_trait::async_trait;

// We define the shape of a Log Entry
#[derive(Debug)]
pub struct LogEntry {
    pub path: String,
    pub line: String,
}

// The Trait.
// #[async_trait] is needed because async functions in traits
// are complicated in Rust (due to memory allocation rules).
#[async_trait]
pub trait Destination: Send + Sync {
    async fn send(&self, entry: LogEntry) -> Result<()>;
}

// --- Concrete Implementation: Vector ---

pub struct VectorDestination {
    client: reqwest::Client,
    endpoint: String,
}

impl VectorDestination {
    pub fn new(endpoint: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
        }
    }
}

#[async_trait]
impl Destination for VectorDestination {
    async fn send(&self, entry: LogEntry) -> Result<()> {
        // In a real app, you'd format this to Vector's JSON schema
        println!("Sending to Vector [{}]: {}", self.endpoint, entry.line);

        // Simulating network call
        // self.client.post(&self.endpoint).json(&entry).send().await?;

        Ok(())
    }
}

// A Factory function to build the destination based on config
pub fn create_destination(endpoint: &str) -> Box<dyn Destination> {
    // We return a Box<dyn Destination>.
    // This means "A pointer to some data on the heap that implements the Destination trait".
    // This matches the behavior of a Go Interface variable.
    Box::new(VectorDestination::new(endpoint.to_string()))
}
