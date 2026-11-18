use crate::config::ApiSourceConfig;
use crate::registry::RegistryUpdate;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use std::collections::HashMap;

/// Maximum pages fetched in one poll, for every pagination mode
const MAX_PAGES_PER_POLL: u32 = 100;

/// Tracks state for a single API source being tailed
struct ApiSourceState {
    last_timestamp: Option<DateTime<Utc>>,
    cursor: Option<String>, // For cursor-based pagination
    /// Where to continue if the last poll stopped at MAX_PAGES_PER_POLL
    resume: Option<PaginationResume>,
}

/// Pagination position saved when a poll hits the page limit
struct PaginationResume {
    page_offset: u32,
    current_page: u32,
    cursor: Option<String>,
    max_timestamp: Option<DateTime<Utc>>,
}

/// Manages tailing multiple API sources
pub struct ApiTailer {
    client: Client,
    sources: HashMap<String, ApiSourceState>,
    initial_positions: HashMap<String, (DateTime<Utc>, Option<String>)>, // name -> (timestamp, cursor)
}

impl ApiTailer {
    /// Create a new ApiTailer
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            client,
            sources: HashMap::new(),
            initial_positions: HashMap::new(),
        })
    }

    /// Current position for an API source, for persisting to the registry
    pub fn checkpoint(&self, name: &str) -> Option<RegistryUpdate> {
        let state = self.sources.get(name)?;
        Some(RegistryUpdate::UpdateApiSource {
            name: name.to_string(),
            last_timestamp: state.last_timestamp?,
            cursor: state.cursor.clone(),
        })
    }

    /// Set initial position for an API source (loaded from registry)
    pub fn set_initial_position(
        &mut self,
        name: String,
        timestamp: DateTime<Utc>,
        cursor: Option<String>,
    ) {
        self.initial_positions.insert(name, (timestamp, cursor));
    }

    /// Poll an API source for new log entries
    /// Returns a Vec of log lines (as strings)
    pub async fn poll(&mut self, config: &ApiSourceConfig) -> Result<Vec<String>> {
        let mut all_lines = Vec::new();

        // Check if we're already tracking this source
        let is_new_source = !self.sources.contains_key(&config.name);

        if is_new_source {
            // First time seeing this source
            // Check if we have an initial position from registry
            let (initial_timestamp, initial_cursor) =
                if let Some((saved_ts, saved_cursor)) = self.initial_positions.get(&config.name) {
                    eprintln!(
                        "Resuming API source {} from registry timestamp {}",
                        config.name, saved_ts
                    );
                    (Some(*saved_ts), saved_cursor.clone())
                } else {
                    // Start from "now" to only capture new logs
                    eprintln!("Now tailing API source: {}", config.name);
                    (Some(Utc::now()), None)
                };

            self.sources.insert(
                config.name.clone(),
                ApiSourceState {
                    last_timestamp: initial_timestamp,
                    cursor: initial_cursor,
                    resume: None,
                },
            );
            return Ok(all_lines); // Return empty on first poll
        }

        // Extract current state (to avoid holding mutable borrow)
        let (initial_timestamp, initial_cursor, resume) = {
            let state = self.sources.get_mut(&config.name).unwrap();
            (
                state.last_timestamp,
                state.cursor.clone(),
                state.resume.take(),
            )
        };

        // Build initial request with time filtering if configured
        let url = config.endpoint.clone();
        let mut query_params: Vec<(String, String)> = Vec::new();

        // Add time filter if configured
        if let Some(time_param) = &config.time_filter_param
            && let Some(since_time) = initial_timestamp
        {
            let time_value = format_timestamp(
                since_time,
                config.time_filter_format.as_deref().unwrap_or("rfc3339"),
            );
            query_params.push((time_param.clone(), time_value));
        }

        // Handle pagination, continuing where a capped poll left off
        let mut current_cursor = initial_cursor.clone();
        let mut page_offset = 0;
        let mut current_page = 1;
        let mut latest_timestamp = initial_timestamp;
        if let Some(resume) = resume {
            current_cursor = resume.cursor;
            page_offset = resume.page_offset;
            current_page = resume.current_page;
            latest_timestamp = resume.max_timestamp;
        }
        let mut latest_cursor = current_cursor.clone();
        let mut pages_fetched = 0;
        let mut capped = false;

        loop {
            pages_fetched += 1;
            // Build request with pagination parameters
            let mut request_params = query_params.clone();

            if let Some(pagination) = &config.pagination {
                match pagination.pagination_type.as_str() {
                    "offset" => {
                        if let Some(limit_param) = &pagination.limit_param {
                            request_params
                                .push((limit_param.clone(), pagination.page_size.to_string()));
                        }
                        if let Some(offset_param) = &pagination.offset_param {
                            request_params.push((offset_param.clone(), page_offset.to_string()));
                        }
                    }
                    "cursor" => {
                        if let Some(cursor_param) = &pagination.cursor_param
                            && let Some(cursor_value) = &current_cursor
                        {
                            request_params.push((cursor_param.clone(), cursor_value.clone()));
                        }
                    }
                    "page" => {
                        if let Some(page_param) = &pagination.page_param {
                            request_params.push((page_param.clone(), current_page.to_string()));
                        }
                        if let Some(limit_param) = &pagination.limit_param {
                            request_params
                                .push((limit_param.clone(), pagination.page_size.to_string()));
                        }
                    }
                    _ => {}
                }
            }

            // Build and execute request
            let request = self.build_request(config, &url, &request_params)?;
            let response = request.send().await.context("Failed to send API request")?;

            if !response.status().is_success() {
                return Err(anyhow!(
                    "API request failed with status: {}",
                    response.status()
                ));
            }

            let body: Value = response
                .json()
                .await
                .context("Failed to parse JSON response")?;

            eprintln!(
                "[API Debug] Received response for '{}': {}",
                config.name,
                serde_json::to_string(&body).unwrap_or_else(|_| "invalid json".to_string())
            );

            // Extract log entries from response
            let entries = self.extract_entries(&body, config)?;
            eprintln!(
                "[API Debug] Extracted {} entries from '{}'",
                entries.len(),
                config.name
            );

            // Process each entry
            for entry in entries {
                // Extract timestamp
                if let Some(timestamp) = self.extract_timestamp(&entry, config) {
                    eprintln!(
                        "[API Debug] Entry timestamp: {}, last_timestamp: {:?}",
                        timestamp, latest_timestamp
                    );

                    // Only process entries after the timestamp this poll started from.
                    // DESIGN CHOICE: Compare against the starting point, not the running
                    // max, so newest-first APIs don't drop the rest of the page.
                    if let Some(last_ts) = initial_timestamp
                        && timestamp <= last_ts
                    {
                        eprintln!(
                            "[API Debug] Skipping entry - timestamp {} <= last_ts {}",
                            timestamp, last_ts
                        );
                        continue; // Skip this entry, we've already seen it
                    }

                    // Extract log message
                    let log_line = if let Some(message_field) = &config.message_field {
                        self.extract_field_as_string(&entry, message_field)
                            .unwrap_or_else(|| entry.to_string())
                    } else {
                        // Serialize entire entry
                        serde_json::to_string(&entry).unwrap_or_else(|_| entry.to_string())
                    };

                    eprintln!(
                        "[API Debug] Adding log line to buffer (length: {})",
                        log_line.len()
                    );
                    all_lines.push(log_line);

                    // Update latest timestamp
                    if latest_timestamp.is_none() || timestamp > latest_timestamp.unwrap() {
                        latest_timestamp = Some(timestamp);
                    }
                } else {
                    // Timestamp extraction failed - entry will be skipped
                    // Warning is already logged in extract_timestamp()
                    eprintln!("[API Debug] Skipping entry - timestamp extraction failed");
                }
            }

            // Check for more pages
            let mut has_more = false;
            if let Some(pagination) = &config.pagination {
                match pagination.pagination_type.as_str() {
                    "offset" => {
                        // If we got a full page, there might be more
                        if let Some(results_array) =
                            body.pointer(&format!("/{}", config.results_field))
                            && let Some(array) = results_array.as_array()
                            && array.len() == pagination.page_size as usize
                        {
                            has_more = true;
                            page_offset += pagination.page_size;
                        }
                    }
                    "cursor" => {
                        // null, empty or missing cursor means last page
                        if let Some(next_cursor_field) = &pagination.next_cursor_field
                            && let Some(next_cursor) = next_cursor_value(&body, next_cursor_field)
                        {
                            if current_cursor.as_deref() == Some(next_cursor.as_str()) {
                                eprintln!(
                                    "Warning: API source {} returned the same cursor again, stopping pagination",
                                    config.name
                                );
                            } else {
                                has_more = true;
                                current_cursor = Some(next_cursor.clone());
                                latest_cursor = Some(next_cursor);
                            }
                        }
                    }
                    "page" => {
                        if let Some(has_more_field) = &pagination.has_more_field
                            && let Some(has_more_value) =
                                body.pointer(&format!("/{}", has_more_field))
                        {
                            has_more = has_more_value.as_bool().unwrap_or(false);
                            if has_more {
                                current_page += 1;
                            }
                        } else if let Some(next_page_field) = &pagination.next_page_field
                            && let Some(next_page) =
                                self.extract_field_as_string(&body, next_page_field)
                            && let Ok(next_page_num) = next_page.parse::<u32>()
                        {
                            has_more = true;
                            current_page = next_page_num;
                        }
                    }
                    _ => {}
                }
            }

            if !has_more {
                break;
            }

            // Safety: Limit pages per poll to avoid runaway loops
            if pages_fetched >= MAX_PAGES_PER_POLL {
                eprintln!(
                    "Warning: API source {} reached {} pages this poll, continuing next poll",
                    config.name, MAX_PAGES_PER_POLL
                );
                capped = true;
                break;
            }
        }

        // Update state (after we're done with all borrows)
        {
            let state = self.sources.get_mut(&config.name).unwrap();
            if capped {
                // DESIGN CHOICE: Don't advance the timestamp until every page has been
                // fetched; otherwise unfetched pages would be filtered out next poll.
                state.resume = Some(PaginationResume {
                    page_offset,
                    current_page,
                    cursor: current_cursor,
                    max_timestamp: latest_timestamp,
                });
            } else {
                state.last_timestamp = latest_timestamp;
                state.cursor = latest_cursor;
            }
        }

        eprintln!(
            "[API Debug] Returning {} log lines for '{}'",
            all_lines.len(),
            config.name
        );
        Ok(all_lines)
    }

    /// Build an HTTP request with authentication
    fn build_request(
        &self,
        config: &ApiSourceConfig,
        url: &str,
        query_params: &[(String, String)],
    ) -> Result<RequestBuilder> {
        let mut request = self.client.get(url).query(query_params);

        // Add authentication
        if let Some(api_key) = &config.api_key {
            request = request.bearer_auth(api_key);
        }

        if let Some(basic) = &config.basic {
            request = request.basic_auth(&basic.username, Some(&basic.password));
        }

        if let Some(headers) = &config.headers {
            for (key, value) in headers {
                request = request.header(key, value);
            }
        }

        Ok(request)
    }

    /// Extract log entries from JSON response
    fn extract_entries(&self, body: &Value, config: &ApiSourceConfig) -> Result<Vec<Value>> {
        let results_pointer = format!("/{}", config.results_field.replace('.', "/"));

        let entries = body.pointer(&results_pointer).ok_or_else(|| {
            anyhow!(
                "Results field '{}' not found in response",
                config.results_field
            )
        })?;

        let entries_array = entries
            .as_array()
            .ok_or_else(|| anyhow!("Results field '{}' is not an array", config.results_field))?;

        Ok(entries_array.clone())
    }

    /// Extract timestamp from a log entry
    fn extract_timestamp(&self, entry: &Value, config: &ApiSourceConfig) -> Option<DateTime<Utc>> {
        let timestamp_pointer = format!("/{}", config.timestamp_field.replace('.', "/"));
        let timestamp_value = entry.pointer(&timestamp_pointer)?;

        // Try to parse as different timestamp formats
        if let Some(ts_str) = timestamp_value.as_str() {
            // Try RFC3339
            if let Ok(dt) = DateTime::parse_from_rfc3339(ts_str) {
                return Some(dt.with_timezone(&Utc));
            }
            // Try unix timestamp as string
            if let Ok(unix_ts) = ts_str.parse::<i64>() {
                return DateTime::from_timestamp(unix_ts, 0);
            }
        }

        // Try unix timestamp as number (i64)
        if let Some(ts_num) = timestamp_value.as_i64() {
            // Heuristic: determine if it's seconds, milliseconds, or microseconds
            // based on the magnitude of the number

            // Microseconds: > 1_000_000_000_000 (roughly > year 2001 in milliseconds)
            // Example: 1766098059650950 (microseconds since epoch)
            if ts_num > 1_000_000_000_000 {
                // Try as microseconds first
                let secs = ts_num / 1_000_000;
                let nanos = ((ts_num % 1_000_000) * 1_000) as u32;
                if let Some(dt) = DateTime::from_timestamp(secs, nanos) {
                    return Some(dt);
                }
            }

            // Milliseconds: > 1_000_000_000 (roughly > year 2001 in seconds)
            // Example: 1766098059650 (milliseconds since epoch)
            if ts_num > 1_000_000_000
                && let Some(dt) = DateTime::from_timestamp_millis(ts_num)
            {
                return Some(dt);
            }

            // Seconds: assume anything else is seconds
            // Example: 1766098059 (seconds since epoch)
            return DateTime::from_timestamp(ts_num, 0);
        }

        eprintln!(
            "Warning: Failed to parse timestamp field '{}' in API source '{}'. Value: {:?}",
            config.timestamp_field, config.name, timestamp_value
        );
        None
    }

    /// Extract a field value as a string
    fn extract_field_as_string(&self, value: &Value, field: &str) -> Option<String> {
        let pointer = format!("/{}", field.replace('.', "/"));
        let field_value = value.pointer(&pointer)?;

        if let Some(s) = field_value.as_str() {
            Some(s.to_string())
        } else {
            Some(field_value.to_string())
        }
    }
}

/// Read the next-page cursor; null, empty or missing means there are no more pages
fn next_cursor_value(body: &Value, field: &str) -> Option<String> {
    let pointer = format!("/{}", field.replace('.', "/"));
    match body.pointer(&pointer)? {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// Format a timestamp according to the specified format
fn format_timestamp(dt: DateTime<Utc>, format: &str) -> String {
    match format {
        "unix" => dt.timestamp().to_string(),
        "unix_ms" => dt.timestamp_millis().to_string(),
        _ => dt.to_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Minimal HTTP server: responds to each request with `respond(query)`.
    /// Returns the endpoint URL and a log of received query strings.
    async fn mock_api<F>(respond: F) -> (String, Arc<Mutex<Vec<String>>>)
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let queries = Arc::new(Mutex::new(Vec::new()));
        let log = queries.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let target = req.split_whitespace().nth(1).unwrap_or("/");
                let query = target.split_once('?').map_or("", |(_, q)| q).to_string();
                log.lock().unwrap().push(query.clone());
                let body = respond(&query);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        (format!("http://{}/logs", addr), queries)
    }

    fn api_config(endpoint: &str, pagination: &str) -> ApiSourceConfig {
        serde_yaml::from_str(&format!(
            r#"
name: test
endpoint: "{}"
polling_frequency_ms: 1000
destination:
  type: file
  path: /dev/null
results_field: data
timestamp_field: ts
message_field: msg
pagination:
{}
"#,
            endpoint, pagination
        ))
        .unwrap()
    }

    /// Create a tailer that has already done its initial (empty) poll from 2020
    async fn primed_tailer(config: &ApiSourceConfig) -> ApiTailer {
        let mut tailer = ApiTailer::new().unwrap();
        let start = "2020-01-01T00:00:00Z".parse().unwrap();
        tailer.set_initial_position(config.name.clone(), start, None);
        assert!(tailer.poll(config).await.unwrap().is_empty());
        tailer
    }

    const CURSOR_PAGINATION: &str = r#"
  pagination_type: cursor
  cursor_param: cursor
  next_cursor_field: next"#;

    #[tokio::test]
    async fn test_cursor_null_ends_pagination() {
        let (url, queries) = mock_api(|_| {
            r#"{"data":[{"ts":"2030-01-01T00:00:00Z","msg":"a"}],"next":null}"#.to_string()
        })
        .await;
        let config = api_config(&url, CURSOR_PAGINATION);
        let mut tailer = primed_tailer(&config).await;

        let lines = tailer.poll(&config).await.unwrap();
        assert_eq!(lines, vec!["a"]);
        assert_eq!(queries.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_cursor_empty_string_ends_pagination() {
        let (url, queries) = mock_api(|_| {
            r#"{"data":[{"ts":"2030-01-01T00:00:00Z","msg":"a"}],"next":""}"#.to_string()
        })
        .await;
        let config = api_config(&url, CURSOR_PAGINATION);
        let mut tailer = primed_tailer(&config).await;

        tailer.poll(&config).await.unwrap();
        assert_eq!(queries.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_cursor_repeat_ends_pagination() {
        let (url, queries) = mock_api(|_| r#"{"data":[],"next":"same"}"#.to_string()).await;
        let config = api_config(&url, CURSOR_PAGINATION);
        let mut tailer = primed_tailer(&config).await;

        tailer.poll(&config).await.unwrap();
        // First page yields cursor "same", second page repeats it -> stop
        assert_eq!(queries.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_newest_first_page_ships_all_entries() {
        let (url, _) = mock_api(|_| {
            r#"{"data":[{"ts":"2030-01-02T00:00:00Z","msg":"new"},{"ts":"2030-01-01T00:00:00Z","msg":"old"}],"next":null}"#
                .to_string()
        })
        .await;
        let config = api_config(&url, CURSOR_PAGINATION);
        let mut tailer = primed_tailer(&config).await;

        let lines = tailer.poll(&config).await.unwrap();
        assert_eq!(lines, vec!["new", "old"]);
    }

    #[tokio::test]
    async fn test_page_limit_resumes_without_advancing_timestamp() {
        // Every page claims there are more
        let (url, queries) = mock_api(|query| {
            let page = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("page="))
                .unwrap_or("0")
                .to_string();
            format!(
                r#"{{"data":[{{"ts":"2030-01-01T00:00:00Z","msg":"p{}"}}],"more":true}}"#,
                page
            )
        })
        .await;
        let config = api_config(
            &url,
            r#"
  pagination_type: page
  page_param: page
  has_more_field: more"#,
        );
        let mut tailer = primed_tailer(&config).await;
        let start_checkpoint = format!("{:?}", tailer.checkpoint("test"));

        let lines = tailer.poll(&config).await.unwrap();
        assert_eq!(lines.len(), MAX_PAGES_PER_POLL as usize);
        // Timestamp held until pagination completes
        assert_eq!(format!("{:?}", tailer.checkpoint("test")), start_checkpoint);

        // Next poll continues from the next page
        let lines = tailer.poll(&config).await.unwrap();
        assert_eq!(lines[0], format!("p{}", MAX_PAGES_PER_POLL + 1));
        let queries = queries.lock().unwrap();
        assert!(
            queries[MAX_PAGES_PER_POLL as usize]
                .contains(&format!("page={}", MAX_PAGES_PER_POLL + 1))
        );
    }
}
