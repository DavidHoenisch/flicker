use crate::config::ApiSourceConfig;
use crate::registry::RegistryUpdate;
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use std::collections::HashMap;

/// Tracks state for a single API source being tailed
struct ApiSourceState {
    last_timestamp: Option<DateTime<Utc>>,
    cursor: Option<String>, // For cursor-based pagination
}

/// Manages tailing multiple API sources
pub struct ApiTailer {
    client: Client,
    sources: HashMap<String, ApiSourceState>,
    initial_positions: HashMap<String, (DateTime<Utc>, Option<String>)>, // name -> (timestamp, cursor)
    registry_tx: Option<tokio::sync::mpsc::UnboundedSender<RegistryUpdate>>,
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
            registry_tx: None,
        })
    }

    /// Set the registry channel for sending position updates
    pub fn set_registry_sender(&mut self, tx: tokio::sync::mpsc::UnboundedSender<RegistryUpdate>) {
        self.registry_tx = Some(tx);
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
                },
            );
            return Ok(all_lines); // Return empty on first poll
        }

        // Extract current state (to avoid holding mutable borrow)
        let (initial_timestamp, initial_cursor) = {
            let state = self.sources.get(&config.name).unwrap();
            (state.last_timestamp, state.cursor.clone())
        };

        // Build initial request with time filtering if configured
        let url = config.endpoint.clone();
        let mut query_params: Vec<(String, String)> = Vec::new();

        // Add time filter if configured
        if let Some(time_param) = &config.time_filter_param {
            if let Some(since_time) = initial_timestamp {
                let time_value = format_timestamp(
                    since_time,
                    config.time_filter_format.as_deref().unwrap_or("rfc3339"),
                );
                query_params.push((time_param.clone(), time_value));
            }
        }

        // Handle pagination
        let mut current_cursor = initial_cursor.clone();
        let mut page_offset = 0;
        let mut current_page = 1;
        let mut has_more = true;

        let mut latest_timestamp = initial_timestamp;
        let mut latest_cursor = initial_cursor;

        while has_more {
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
                        if let Some(cursor_param) = &pagination.cursor_param {
                            if let Some(cursor_value) = &current_cursor {
                                request_params.push((cursor_param.clone(), cursor_value.clone()));
                            }
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

            eprintln!("[API Debug] Received response for '{}': {}", config.name,
                serde_json::to_string(&body).unwrap_or_else(|_| "invalid json".to_string()));

            // Extract log entries from response
            let entries = self.extract_entries(&body, config)?;
            eprintln!("[API Debug] Extracted {} entries from '{}'", entries.len(), config.name);

            // Process each entry
            for entry in entries {
                // Extract timestamp
                if let Some(timestamp) = self.extract_timestamp(&entry, config) {
                    eprintln!("[API Debug] Entry timestamp: {}, last_timestamp: {:?}",
                        timestamp, latest_timestamp);

                    // Only process entries after our last timestamp
                    if let Some(last_ts) = latest_timestamp {
                        if timestamp <= last_ts {
                            eprintln!("[API Debug] Skipping entry - timestamp {} <= last_ts {}",
                                timestamp, last_ts);
                            continue; // Skip this entry, we've already seen it
                        }
                    }

                    // Extract log message
                    let log_line = if let Some(message_field) = &config.message_field {
                        self.extract_field_as_string(&entry, message_field)
                            .unwrap_or_else(|| entry.to_string())
                    } else {
                        // Serialize entire entry
                        serde_json::to_string(&entry).unwrap_or_else(|_| entry.to_string())
                    };

                    eprintln!("[API Debug] Adding log line to buffer (length: {})", log_line.len());
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
            has_more = false;
            if let Some(pagination) = &config.pagination {
                match pagination.pagination_type.as_str() {
                    "offset" => {
                        // If we got a full page, there might be more
                        if let Some(results_array) =
                            body.pointer(&format!("/{}", config.results_field))
                        {
                            if let Some(array) = results_array.as_array() {
                                if array.len() == pagination.page_size as usize {
                                    has_more = true;
                                    page_offset += pagination.page_size;
                                }
                            }
                        }
                    }
                    "cursor" => {
                        if let Some(next_cursor_field) = &pagination.next_cursor_field {
                            if let Some(next_cursor) =
                                self.extract_field_as_string(&body, next_cursor_field)
                            {
                                has_more = true;
                                current_cursor = Some(next_cursor.clone());
                                latest_cursor = Some(next_cursor);
                            }
                        }
                    }
                    "page" => {
                        if let Some(has_more_field) = &pagination.has_more_field {
                            if let Some(has_more_value) =
                                body.pointer(&format!("/{}", has_more_field))
                            {
                                has_more = has_more_value.as_bool().unwrap_or(false);
                                if has_more {
                                    current_page += 1;
                                }
                            }
                        } else if let Some(next_page_field) = &pagination.next_page_field {
                            if let Some(next_page) =
                                self.extract_field_as_string(&body, next_page_field)
                            {
                                if let Ok(next_page_num) = next_page.parse::<u32>() {
                                    has_more = true;
                                    current_page = next_page_num;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Safety: Limit to reasonable number of pages to avoid infinite loops
            if page_offset > 10000 || current_page > 100 {
                eprintln!(
                    "Warning: API source {} reached pagination limit, stopping",
                    config.name
                );
                break;
            }
        }

        // Update state (after we're done with all borrows)
        {
            let state = self.sources.get_mut(&config.name).unwrap();
            state.last_timestamp = latest_timestamp;
            state.cursor = latest_cursor.clone();
        }

        // Send registry update
        if !all_lines.is_empty() {
            if let Some(tx) = &self.registry_tx {
                let _ = tx.send(RegistryUpdate::UpdateApiSource {
                    name: config.name.clone(),
                    last_timestamp: latest_timestamp.unwrap_or_else(Utc::now),
                    cursor: latest_cursor,
                });
            }
        }

        eprintln!("[API Debug] Returning {} log lines for '{}'", all_lines.len(), config.name);
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
            if ts_num > 1_000_000_000 {
                if let Some(dt) = DateTime::from_timestamp_millis(ts_num) {
                    return Some(dt);
                }
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

/// Format a timestamp according to the specified format
fn format_timestamp(dt: DateTime<Utc>, format: &str) -> String {
    match format {
        "unix" => dt.timestamp().to_string(),
        "unix_ms" => dt.timestamp_millis().to_string(),
        "rfc3339" | _ => dt.to_rfc3339(),
    }
}
