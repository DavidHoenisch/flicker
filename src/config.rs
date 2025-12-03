use serde::Deserialize;
use std::fs;

// #[derive(Deserialize)] generates the code to parse YAML into this struct.
// It is equivalent to Go's struct tags `yaml:"..."`.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub polling_frequency_ms: u64,
    pub log_paths: Vec<String>,
    pub destination: DestinationConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DestinationConfig {
    pub endpoint: String,
    pub api_key: Option<String>, // Option is Rust's nil-safety.
}

impl Config {
    // A constructor-like function.
    // Result<T> is how Rust handles errors. No more (val, err).
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path)?; // The '?' operator is "if err != nil { return err }"
        let config = serde_yaml::from_str(&content)?;
        Ok(config)
    }
}
