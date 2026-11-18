use crate::masking::MaskingConfig;
use serde::Deserialize;
use std::fs;
use std::time::Duration;

#[derive(Debug, Deserialize, Clone)]
pub struct RetryConfigOptions {
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,

    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,

    #[serde(default = "default_max_queue_size")]
    pub max_queue_size: usize,

    /// Optional JSONL file for batches dropped after max_retries
    #[serde(default)]
    pub dead_letter_path: Option<String>,
}

impl Default for RetryConfigOptions {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            initial_delay_ms: default_initial_delay_ms(),
            max_delay_ms: default_max_delay_ms(),
            max_queue_size: default_max_queue_size(),
            dead_letter_path: None,
        }
    }
}

impl RetryConfigOptions {
    pub fn to_retry_config(&self) -> crate::retry_queue::RetryConfig {
        crate::retry_queue::RetryConfig {
            max_retries: self.max_retries,
            initial_delay_ms: self.initial_delay_ms,
            max_delay_ms: self.max_delay_ms,
            max_queue_size: self.max_queue_size,
            dead_letter_path: self.dead_letter_path.clone(),
        }
    }
}

fn default_max_retries() -> u32 {
    5
}

fn default_initial_delay_ms() -> u64 {
    1000 // 1 second
}

fn default_max_delay_ms() -> u64 {
    60000 // 60 seconds
}

fn default_max_queue_size() -> usize {
    100
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub log_files: Vec<LogFileConfig>,

    #[serde(default)]
    pub docker_containers: Vec<DockerContainerConfig>,

    #[serde(default)]
    pub api_sources: Vec<ApiSourceConfig>,

    #[serde(default)]
    pub retry: RetryConfigOptions,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LogFileConfig {
    pub path: String,
    pub polling_frequency_ms: u64,
    pub destination: DestinationConfig,

    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    #[serde(default = "default_flush_interval_ms")]
    pub flush_interval_ms: u64,

    #[serde(default)]
    pub match_on: Vec<String>, // List of regex patterns to match (empty = match all)

    #[serde(default)]
    pub exclude_on: Vec<String>, // List of regex patterns to exclude (empty = exclude none)

    #[serde(default)]
    pub masking: MaskingConfig, // Data masking and PII redaction configuration
}

#[derive(Debug, Deserialize, Clone)]
pub struct DockerContainerConfig {
    pub container: String, // Container name or ID
    pub polling_frequency_ms: u64,
    pub destination: DestinationConfig,

    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    #[serde(default = "default_flush_interval_ms")]
    pub flush_interval_ms: u64,

    #[serde(default)]
    pub match_on: Vec<String>, // List of regex patterns to match (empty = match all)

    #[serde(default)]
    pub exclude_on: Vec<String>, // List of regex patterns to exclude (empty = exclude none)

    #[serde(default)]
    pub masking: MaskingConfig, // Data masking and PII redaction configuration
}

#[derive(Debug, Deserialize, Clone)]
pub struct ApiSourceConfig {
    pub name: String,     // Unique identifier for this API source (used in registry)
    pub endpoint: String, // API endpoint URL
    pub polling_frequency_ms: u64,
    pub destination: DestinationConfig,

    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    #[serde(default = "default_flush_interval_ms")]
    pub flush_interval_ms: u64,

    #[serde(default)]
    pub match_on: Vec<String>, // List of regex patterns to match (empty = match all)

    #[serde(default)]
    pub exclude_on: Vec<String>, // List of regex patterns to exclude (empty = exclude none)

    #[serde(default)]
    pub masking: MaskingConfig, // Data masking and PII redaction configuration

    // Authentication
    pub api_key: Option<String>,  // Bearer token or API key
    pub basic: Option<BasicAuth>, // Basic auth credentials
    pub headers: Option<std::collections::HashMap<String, String>>, // Custom headers

    // Response parsing
    pub results_field: String, // JSON path to array of log entries (e.g., "data", "logs", "events")
    pub timestamp_field: String, // Field in each log entry containing timestamp
    pub message_field: Option<String>, // Field containing the log message (if None, serialize entire object)
    pub id_field: Option<String>, // Unique entry id, used to de-duplicate entries sharing a timestamp (default: hash of whole entry)
    #[serde(default)]
    pub on_missing_timestamp: MissingTimestampAction, // What to do with entries lacking a parseable timestamp

    // Pagination
    pub pagination: Option<PaginationConfig>,

    // Time filtering
    #[serde(default)]
    pub time_filter_param: Option<String>, // Query param for filtering by time (e.g., "since", "start_time")
    #[serde(default)]
    pub time_filter_format: Option<String>, // Time format: "unix", "unix_ms", "rfc3339" (default: "rfc3339")

    /// Largest response body accepted, in bytes (default: 64 MiB)
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
}

fn default_max_response_bytes() -> usize {
    64 * 1024 * 1024
}

/// Handling for API entries whose timestamp is missing or unparseable
#[derive(Debug, Deserialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MissingTimestampAction {
    /// Skip the entry (logged as a warning)
    #[default]
    Drop,
    /// Ship the entry anyway; it may be re-shipped on later polls
    Ship,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PaginationConfig {
    #[serde(default = "default_pagination_type")]
    pub pagination_type: String, // "offset", "cursor", or "page"

    // For offset-based pagination
    pub limit_param: Option<String>, // Query param for page size (e.g., "limit", "per_page")
    pub offset_param: Option<String>, // Query param for offset (e.g., "offset")

    // For cursor-based pagination
    pub cursor_param: Option<String>, // Query param for cursor (e.g., "cursor", "next_token")
    pub next_cursor_field: Option<String>, // Response field containing next cursor

    // For page-based pagination
    pub page_param: Option<String>, // Query param for page number (e.g., "page")
    pub next_page_field: Option<String>, // Response field containing next page number
    pub has_more_field: Option<String>, // Response field indicating if more pages exist

    #[serde(default = "default_page_size")]
    pub page_size: u32, // Number of items per page
}

fn default_pagination_type() -> String {
    "offset".to_string()
}

fn default_page_size() -> u32 {
    100
}

// Default: Flush every 100 lines
fn default_buffer_size() -> usize {
    100
}

// Default: Flush every 30 seconds
fn default_flush_interval_ms() -> u64 {
    30_000
}

#[derive(Debug, Deserialize, Clone)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TlsConfig {
    pub cert_path: String, // Path to client certificate file (PEM format)
    pub key_path: String,  // Path to client private key file (PEM format)
    pub ca_cert_path: Option<String>, // Optional path to CA certificate for server verification
    pub accept_invalid_certs: Option<bool>, // Accept invalid/self-signed server certs (default: false)
}

#[derive(Debug, Deserialize, Clone)]
pub struct DestinationConfig {
    #[serde(rename = "type")]
    pub dest_type: String, // "http", "syslog", "elasticsearch", "file"

    // HTTP destination fields
    pub endpoint: Option<String>, // HTTP endpoint URL
    pub require_auth: Option<bool>,
    pub api_key: Option<String>, // Optional API key for auth
    pub basic: Option<BasicAuth>,
    pub compression: Option<bool>, // Enable gzip compression (default: false)
    pub tls: Option<TlsConfig>,    // TLS/mTLS configuration

    // Syslog destination fields
    pub host: Option<String>,     // Syslog server hostname
    pub port: Option<u16>,        // Syslog server port (default: 514)
    pub protocol: Option<String>, // "udp" or "tcp" (default: "udp")

    // Elasticsearch destination fields
    pub url: Option<String>,   // Elasticsearch URL
    pub index: Option<String>, // Index name

    // File destination fields
    pub path: Option<String>, // Output file path

    // S3 destination fields
    pub bucket: Option<String>, // S3 bucket name
    pub prefix: Option<String>, // Optional key prefix (e.g., "logs/myapp/")
    pub region: Option<String>, // Optional AWS region

    // Network timeouts (http, elasticsearch, syslog tcp, s3)
    pub timeout_ms: Option<u64>, // Whole-request timeout (default: 30000)
    pub connect_timeout_ms: Option<u64>, // Connection timeout (default: 10000)
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config {}: {}", path, e))?;
        warn_if_world_readable(path);

        // DESIGN CHOICE: Substitute ${...} references on the parsed YAML tree (string
        // values only), so comments and YAML quoting don't interact with expansion.
        let raw: serde_yaml::Value = serde_yaml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config {}: {}", path, e))?;
        let expanded = expand_value(raw, &|name| std::env::var(name).ok())
            .map_err(|e| anyhow::anyhow!("Config {}: {}", path, e))?;
        let config: Self = serde_yaml::from_value(expanded)
            .map_err(|e| anyhow::anyhow!("Failed to parse config {}: {}", path, e))?;
        config.validate()?;
        Ok(config)
    }

    /// Check values that would otherwise panic or misbehave at runtime.
    /// Reports every problem at once rather than the first.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut errors = Vec::new();

        if self.log_files.is_empty()
            && self.docker_containers.is_empty()
            && self.api_sources.is_empty()
        {
            errors.push("no log_files, docker_containers or api_sources configured".to_string());
        }

        let sources = self
            .log_files
            .iter()
            .map(|f| {
                (
                    format!("log file '{}'", f.path),
                    f.path.is_empty(),
                    f.polling_frequency_ms,
                    f.buffer_size,
                )
            })
            .chain(self.docker_containers.iter().map(|c| {
                (
                    format!("docker container '{}'", c.container),
                    c.container.is_empty(),
                    c.polling_frequency_ms,
                    c.buffer_size,
                )
            }))
            .chain(self.api_sources.iter().map(|a| {
                (
                    format!("api source '{}'", a.name),
                    a.name.is_empty(),
                    a.polling_frequency_ms,
                    a.buffer_size,
                )
            }));
        for (label, name_empty, polling_frequency_ms, buffer_size) in sources {
            if name_empty {
                errors.push(format!("{}: name/path must not be empty", label));
            }
            if polling_frequency_ms == 0 {
                errors.push(format!("{}: polling_frequency_ms must be > 0", label));
            }
            if buffer_size == 0 {
                errors.push(format!("{}: buffer_size must be > 0", label));
            }
        }

        // API source names key the registry, so they must be unique
        let mut seen = std::collections::HashSet::new();
        for api in &self.api_sources {
            if !seen.insert(&api.name) {
                errors.push(format!("api source '{}': duplicate name", api.name));
            }
        }

        if self.retry.max_queue_size == 0 {
            errors.push(
                "retry.max_queue_size must be > 0 (reading pauses while the queue is full)"
                    .to_string(),
            );
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("Invalid config:\n  - {}", errors.join("\n  - "))
        }
    }

    #[cfg(test)]
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Self> {
        let config = serde_yaml::from_str(yaml)?;
        Ok(config)
    }
}

/// Warn when the config file can be read by any user, since it may hold secrets
fn warn_if_world_readable(path: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path)
            && meta.permissions().mode() & 0o004 != 0
        {
            eprintln!(
                "Warning: config {} is world-readable; if it contains secrets, restrict it (chmod 600) or use ${{ENV_VAR}} / ${{file:/path}} references",
                path
            );
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Expand `${...}` references in every string value of a YAML tree
fn expand_value(
    value: serde_yaml::Value,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> anyhow::Result<serde_yaml::Value> {
    use serde_yaml::Value;
    Ok(match value {
        Value::String(s) => Value::String(expand_string(&s, lookup)?),
        Value::Sequence(items) => Value::Sequence(
            items
                .into_iter()
                .map(|v| expand_value(v, lookup))
                .collect::<anyhow::Result<_>>()?,
        ),
        Value::Mapping(map) => Value::Mapping(
            map.into_iter()
                .map(|(k, v)| Ok((k, expand_value(v, lookup)?)))
                .collect::<anyhow::Result<_>>()?,
        ),
        Value::Tagged(tagged) => {
            let tagged = *tagged;
            Value::Tagged(Box::new(serde_yaml::value::TaggedValue {
                tag: tagged.tag,
                value: expand_value(tagged.value, lookup)?,
            }))
        }
        other => other,
    })
}

/// Expand references in a config string, so secrets needn't be stored in the file:
/// - `${VAR}` environment variable (error if unset)
/// - `${VAR:-default}` environment variable with a default
/// - `${file:/path}` contents of a file, trailing newline removed (e.g. Docker/k8s secrets)
/// - `$${` a literal `${`
fn expand_string(input: &str, lookup: &dyn Fn(&str) -> Option<String>) -> anyhow::Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(after) = rest.strip_prefix("$${") {
            out.push_str("${");
            rest = after;
        } else if let Some(after) = rest.strip_prefix("${") {
            let end = after
                .find('}')
                .ok_or_else(|| anyhow::anyhow!("unterminated ${{ in \"{}\"", input))?;
            let reference = &after[..end];
            if let Some(file) = reference.strip_prefix("file:") {
                let contents = fs::read_to_string(file)
                    .map_err(|e| anyhow::anyhow!("failed to read secret file {}: {}", file, e))?;
                out.push_str(contents.trim_end_matches(['\n', '\r']));
            } else {
                let (name, default) = match reference.split_once(":-") {
                    Some((name, default)) => (name, Some(default)),
                    None => (reference, None),
                };
                match (lookup(name), default) {
                    (Some(v), _) => out.push_str(&v),
                    (None, Some(d)) => out.push_str(d),
                    (None, None) => anyhow::bail!("environment variable {} is not set", name),
                }
            }
            rest = &after[end + 1..];
        } else {
            out.push('$');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Default whole-request timeout for network destinations
const DEFAULT_DEST_TIMEOUT_MS: u64 = 30_000;
/// Default connect timeout for network destinations
const DEFAULT_DEST_CONNECT_TIMEOUT_MS: u64 = 10_000;

impl DestinationConfig {
    /// Whole-request timeout. DESIGN CHOICE: always bounded, so a hung endpoint
    /// can't stall a source's poll loop forever.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.unwrap_or(DEFAULT_DEST_TIMEOUT_MS))
    }

    pub fn connect_timeout(&self) -> Duration {
        Duration::from_millis(
            self.connect_timeout_ms
                .unwrap_or(DEFAULT_DEST_CONNECT_TIMEOUT_MS),
        )
    }

    pub fn has_auth(&self) -> bool {
        let has_api = self.api_key.as_ref().is_some_and(|k| !k.is_empty());
        let has_basic = self.basic.is_some();

        has_api || has_basic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_config() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "http"
      endpoint: "http://localhost:8000/ingest"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files.len(), 1);
        assert_eq!(config.log_files[0].path, "/var/log/test.log");
        assert_eq!(config.log_files[0].polling_frequency_ms, 500);
        assert_eq!(config.log_files[0].buffer_size, 100); // Default
        assert_eq!(config.log_files[0].flush_interval_ms, 30000); // Default
        assert_eq!(config.log_files[0].destination.dest_type, "http");
    }

    #[test]
    fn test_config_with_custom_buffering() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 250
    buffer_size: 50
    flush_interval_ms: 15000
    destination:
      type: "http"
      endpoint: "http://localhost:8000/ingest"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files[0].buffer_size, 50);
        assert_eq!(config.log_files[0].flush_interval_ms, 15000);
    }

    #[test]
    fn test_config_with_filters() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    match_on:
      - "ERROR"
      - "WARN"
    exclude_on:
      - "DEBUG"
    destination:
      type: "http"
      endpoint: "http://localhost:8000/ingest"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files[0].match_on.len(), 2);
        assert_eq!(config.log_files[0].match_on[0], "ERROR");
        assert_eq!(config.log_files[0].match_on[1], "WARN");
        assert_eq!(config.log_files[0].exclude_on.len(), 1);
        assert_eq!(config.log_files[0].exclude_on[0], "DEBUG");
    }

    #[test]
    fn test_http_destination() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "http"
      endpoint: "http://example.com/logs"
      require_auth: true
      api_key: "secret123"
      basic:
        username: "user"
        password: "password"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "http");
        assert_eq!(dest.endpoint.as_ref().unwrap(), "http://example.com/logs");
        assert_eq!(dest.require_auth, Some(true));
        assert_eq!(dest.api_key.as_ref().unwrap(), "secret123");
        assert_eq!(dest.basic.as_ref().unwrap().username, "user");
        assert_eq!(dest.basic.as_ref().unwrap().password, "password");
    }

    #[test]
    fn test_destination_timeouts() {
        let yaml = r#"
log_files:
  - path: "/var/log/a.log"
    polling_frequency_ms: 500
    destination:
      type: "http"
      endpoint: "http://localhost:8000/ingest"
  - path: "/var/log/b.log"
    polling_frequency_ms: 500
    destination:
      type: "http"
      endpoint: "http://localhost:8000/ingest"
      timeout_ms: 5000
      connect_timeout_ms: 1000
        "#;
        let config = Config::from_yaml(yaml).unwrap();
        let defaults = &config.log_files[0].destination;
        assert_eq!(defaults.request_timeout(), Duration::from_secs(30));
        assert_eq!(defaults.connect_timeout(), Duration::from_secs(10));
        let custom = &config.log_files[1].destination;
        assert_eq!(custom.request_timeout(), Duration::from_secs(5));
        assert_eq!(custom.connect_timeout(), Duration::from_secs(1));
    }

    #[test]
    fn test_syslog_destination() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "syslog"
      host: "syslog.example.com"
      port: 514
      protocol: "tcp"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "syslog");
        assert_eq!(dest.host.as_ref().unwrap(), "syslog.example.com");
        assert_eq!(dest.port.unwrap(), 514);
        assert_eq!(dest.protocol.as_ref().unwrap(), "tcp");
    }

    #[test]
    fn test_elasticsearch_destination() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "elasticsearch"
      url: "http://elasticsearch:9200"
      index: "logs-test"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "elasticsearch");
        assert_eq!(dest.url.as_ref().unwrap(), "http://elasticsearch:9200");
        assert_eq!(dest.index.as_ref().unwrap(), "logs-test");
    }

    #[test]
    fn test_file_destination() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "file"
      path: "/backup/logs.jsonl"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "file");
        assert_eq!(dest.path.as_ref().unwrap(), "/backup/logs.jsonl");
    }

    #[test]
    fn test_multiple_log_files() {
        let yaml = r#"
log_files:
  - path: "/var/log/app1.log"
    polling_frequency_ms: 250
    destination:
      type: "http"
      endpoint: "http://dest1:8000/ingest"

  - path: "/var/log/app2.log"
    polling_frequency_ms: 500
    destination:
      type: "syslog"
      host: "syslog.local"

  - path: "/var/log/app3.log"
    polling_frequency_ms: 1000
    destination:
      type: "elasticsearch"
      url: "http://es:9200"
      index: "logs"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files.len(), 3);
        assert_eq!(config.log_files[0].path, "/var/log/app1.log");
        assert_eq!(config.log_files[1].path, "/var/log/app2.log");
        assert_eq!(config.log_files[2].path, "/var/log/app3.log");
    }

    #[test]
    fn test_invalid_yaml() {
        let yaml = "invalid: yaml: syntax: [[[";
        assert!(Config::from_yaml(yaml).is_err());
    }

    #[test]
    fn test_missing_required_fields() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    # Missing polling_frequency_ms
    destination:
      type: "http"
      endpoint: "http://localhost:8000"
        "#;

        assert!(Config::from_yaml(yaml).is_err());
    }

    #[test]
    fn test_empty_log_files() {
        let yaml = r#"
log_files: []
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files.len(), 0);
    }

    #[test]
    fn test_http_destination_with_tls() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "http"
      endpoint: "https://example.com/logs"
      tls:
        cert_path: "/etc/flicker/client.crt"
        key_path: "/etc/flicker/client.key"
        ca_cert_path: "/etc/flicker/ca.crt"
        accept_invalid_certs: false
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "http");
        assert_eq!(dest.endpoint.as_ref().unwrap(), "https://example.com/logs");
        assert!(dest.tls.is_some());

        let tls = dest.tls.as_ref().unwrap();
        assert_eq!(tls.cert_path, "/etc/flicker/client.crt");
        assert_eq!(tls.key_path, "/etc/flicker/client.key");
        assert_eq!(tls.ca_cert_path.as_ref().unwrap(), "/etc/flicker/ca.crt");
        assert_eq!(tls.accept_invalid_certs, Some(false));
    }

    #[test]
    fn test_http_destination_with_minimal_tls() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "http"
      endpoint: "https://example.com/logs"
      tls:
        cert_path: "/etc/flicker/client.crt"
        key_path: "/etc/flicker/client.key"
        "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert!(dest.tls.is_some());

        let tls = dest.tls.as_ref().unwrap();
        assert_eq!(tls.cert_path, "/etc/flicker/client.crt");
        assert_eq!(tls.key_path, "/etc/flicker/client.key");
        assert!(tls.ca_cert_path.is_none());
        assert_eq!(tls.accept_invalid_certs, None);
    }

    #[test]
    fn test_s3_destination_config() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "s3"
      bucket: "my-log-bucket"
      prefix: "logs/app/"
      region: "us-west-2"
    "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "s3");
        assert_eq!(dest.bucket.as_ref().unwrap(), "my-log-bucket");
        assert_eq!(dest.prefix.as_ref().unwrap(), "logs/app/");
        assert_eq!(dest.region.as_ref().unwrap(), "us-west-2");
    }

    #[test]
    fn test_s3_destination_config_minimal() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "s3"
      bucket: "minimal-bucket"
    "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "s3");
        assert_eq!(dest.bucket.as_ref().unwrap(), "minimal-bucket");
        assert!(dest.prefix.is_none());
        assert!(dest.region.is_none());
    }

    #[test]
    fn test_s3_destination_config_with_prefix_only() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "s3"
      bucket: "test-bucket"
      prefix: "myapp/logs/"
    "#;

        let config = Config::from_yaml(yaml).unwrap();
        let dest = &config.log_files[0].destination;
        assert_eq!(dest.dest_type, "s3");
        assert_eq!(dest.bucket.as_ref().unwrap(), "test-bucket");
        assert_eq!(dest.prefix.as_ref().unwrap(), "myapp/logs/");
        assert!(dest.region.is_none());
    }

    #[test]
    fn test_masking_config_parsing() {
        let yaml = r#"
log_files:
  - path: "./test.log"
    polling_frequency_ms: 250
    buffer_size: 5
    flush_interval_ms: 5000
    masking:
      enabled: true
      rules:
        email:
          enabled: true
          action: "redact"
          replacement: "[EMAIL]"
    destination:
      endpoint: "http://localhost:8000/ingest"
      type: "http"
"#;

        let config = Config::from_yaml(yaml).unwrap();
        assert_eq!(config.log_files.len(), 1);
        assert!(config.log_files[0].masking.enabled);
        assert!(config.log_files[0].masking.rules.email.is_some());

        let email_rule = config.log_files[0].masking.rules.email.as_ref().unwrap();
        assert!(email_rule.enabled);
        assert_eq!(email_rule.action, "redact");
        assert_eq!(email_rule.replacement.as_ref().unwrap(), "[EMAIL]");
    }

    #[test]
    fn test_masking_config_disabled_by_default() {
        let yaml = r#"
log_files:
  - path: "./test.log"
    polling_frequency_ms: 250
    destination:
      endpoint: "http://localhost:8000/ingest"
      type: "http"
"#;

        let config = Config::from_yaml(yaml).unwrap();
        assert!(!config.log_files[0].masking.enabled);
    }

    #[test]
    fn test_validate_accepts_minimal_config() {
        let yaml = r#"
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "file"
      path: "/tmp/out.log"
        "#;
        assert!(Config::from_yaml(yaml).unwrap().validate().is_ok());
    }

    #[test]
    fn test_validate_rejects_bad_values() {
        let yaml = r#"
retry:
  max_queue_size: 0
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 0
    buffer_size: 0
    destination:
      type: "file"
      path: "/tmp/out.log"
api_sources:
  - name: dup
    endpoint: "http://localhost"
    polling_frequency_ms: 1000
    results_field: data
    timestamp_field: ts
    destination: { type: file, path: /tmp/a }
  - name: dup
    endpoint: "http://localhost"
    polling_frequency_ms: 1000
    results_field: data
    timestamp_field: ts
    destination: { type: file, path: /tmp/a }
        "#;
        let err = Config::from_yaml(yaml)
            .unwrap()
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("polling_frequency_ms must be > 0"), "{}", err);
        assert!(err.contains("buffer_size must be > 0"), "{}", err);
        assert!(err.contains("duplicate name"), "{}", err);
        assert!(err.contains("max_queue_size"), "{}", err);
    }

    #[test]
    fn test_validate_rejects_empty_config() {
        let err = Config::from_yaml("{}")
            .unwrap()
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("no log_files"), "{}", err);
    }

    fn lookup(name: &str) -> Option<String> {
        match name {
            "API_KEY" => Some("s3cret".to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        }
    }

    #[test]
    fn test_expand_string() {
        assert_eq!(expand_string("${API_KEY}", &lookup).unwrap(), "s3cret");
        assert_eq!(
            expand_string("Bearer ${API_KEY}!", &lookup).unwrap(),
            "Bearer s3cret!"
        );
        assert_eq!(expand_string("${MISSING:-dflt}", &lookup).unwrap(), "dflt");
        assert_eq!(expand_string("${EMPTY:-dflt}", &lookup).unwrap(), "");
        assert_eq!(
            expand_string("cost $5 $${X}", &lookup).unwrap(),
            "cost $5 ${X}"
        );
        assert!(expand_string("${MISSING}", &lookup).is_err());
        assert!(expand_string("${API_KEY", &lookup).is_err());
    }

    #[test]
    fn test_expand_secret_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        writeln!(file, "from-file").unwrap();
        let input = format!("${{file:{}}}", file.path().display());
        assert_eq!(expand_string(&input, &lookup).unwrap(), "from-file");
    }

    #[test]
    fn test_expand_config_tree() {
        let yaml = r#"
# comment with ${NOT_SET} is ignored
log_files:
  - path: "/var/log/test.log"
    polling_frequency_ms: 500
    destination:
      type: "http"
      endpoint: "http://localhost:8000/ingest"
      api_key: "${API_KEY}"
"#;
        let raw: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let config: Config = serde_yaml::from_value(expand_value(raw, &lookup).unwrap()).unwrap();
        assert_eq!(
            config.log_files[0].destination.api_key.as_deref(),
            Some("s3cret")
        );
    }
}
