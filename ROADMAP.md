# Flicker Roadmap

This document outlines planned and potential features for Flicker, the lightweight log shipping agent. Ideas are categorized by theme and prioritized by potential impact and feasibility. Contributions and feedback are welcome!

## 1. Enhanced Input Sources (Expand Data Ingestion)
- **Kafka Consumer Support**: Allow Flicker to consume logs from Kafka topics as an input source, with support for consumer groups, offset management, and topic partitioning.
- **Database Change Data Capture (CDC)**: Add support for tailing database logs or using CDC tools (e.g., via Debezium) to ship database changes as structured logs.
- **Cloud Storage Buckets (e.g., S3, GCS)**: Poll and ship logs stored in object storage buckets, with options for prefix filtering and event-driven triggers (e.g., via SQS/SNS for S3).
- **Syslog UDP/TCP Receiver**: Act as a syslog server to receive logs directly over the network, in addition to just sending to syslog destinations.
- **Webhook/WebSocket Inputs**: Accept real-time log pushes via webhooks or WebSockets for applications that prefer push over poll.

## 2. Advanced Transformations and Processing (Improve Log Enrichment)
- **Field Extraction and Parsing**: Beyond regex filtering, add parsers for common formats like JSON, CSV, key-value pairs, or custom Grok patterns. Allow extracting fields (e.g., timestamp, level) and adding them as metadata.
- **Data Masking and Compliance**: Support redacting sensitive data (e.g., PII like emails or credit cards) using regex or built-in rules, with options for hashing or encryption.
- **Log Enrichment**: Automatically add contextual metadata like host info, Kubernetes pod labels, or geo-IP data based on IP addresses.
- **Conditional Routing**: Route logs to different destinations based on content (e.g., error logs to one endpoint, debug to another), beyond just per-source configs.
- **Sampling and Deduplication**: Add configurable sampling rates (e.g., ship only 10% of logs) or deduplication logic to avoid shipping repeated entries.

## 3. New Destinations (Expand Output Options)
- **Kafka Producer**: Ship logs directly to Kafka topics for downstream processing.
- **Cloud Logging Services**: Native support for AWS CloudWatch Logs, Google Cloud Logging, Azure Monitor, or Splunk HEC.
- **Database Destinations**: Write logs to databases like PostgreSQL, ClickHouse, or Elasticsearch (beyond the current basic ES support) with batch inserts.
- **Message Queues**: Support for RabbitMQ, Redis Pub/Sub, or NATS for async queuing.
- **File Rotation and Archiving**: Enhanced file destination with automatic rotation, compression (e.g., zstd), and archival to S3 after a retention period.

## 4. Observability and Management (Make It Easier to Operate)
- **Metrics and Monitoring**: Expose Prometheus-compatible metrics (e.g., lines processed, latency, error rates) and add health check endpoints for Kubernetes liveness/readiness probes.
- **Web UI/Dashboard**: A simple web interface (built-in or optional) for viewing config, real-time stats, and log previews without editing YAML files.
- **Configuration Hot-Reload**: Allow config changes to be applied without restarting, using file watching or API endpoints.
- **Alerting and Notifications**: Integrate with services like PagerDuty or Slack to alert on high error rates, failed shipments, or buffer overflows.
- **Log Levels and Structured Logging**: Add configurable logging levels for Flicker itself, with structured JSON output for easier debugging.

## 5. Scalability and Reliability (Handle Larger Deployments)
- **Clustering and High Availability**: Support multiple Flicker instances coordinating via Consul/Etcd or Kubernetes leader election to distribute load and avoid duplicates.
- **Rate Limiting and Backpressure**: Add per-destination rate limits to prevent overwhelming downstream systems, with backpressure to slow input polling if needed.
- **Plugin System**: Allow user-defined plugins (e.g., in Lua or WebAssembly) for custom inputs, outputs, or transforms, making Flicker extensible without core changes.
- **Cross-Platform Enhancements**: Full Windows support with better file rotation detection (e.g., via file change notifications), and ARM64 binaries for edge deployments.

## 6. Security and Compliance (Harden for Enterprise Use)
- **Advanced Authentication**: Support OAuth2, JWT, or certificate-based auth for API sources and destinations.
- **Audit Logging**: Log all configuration changes, failed auth attempts, and data flows for compliance (e.g., SOC 2).
- **Encryption at Rest/Transit**: Beyond TLS, add end-to-end encryption options or support for encrypted S3 buckets.
- **Schema Validation**: Validate incoming log formats against JSON schemas to ensure data quality.

## Implementation Notes
- **Prioritize Simplicity**: Keep features lightweight—avoid over-engineering by leveraging existing async Rust crates.
- **Backwards Compatibility**: Ensure new features don't break existing configs; use optional fields or feature flags.
- **Testing**: Expand the `test_tools/` with mocks/simulators for new sources/destinations.
- **Community Input**: Use GitHub discussions for feature requests to gauge interest.

## Status
- [ ] Review and prioritize items
- [ ] Create issues for top features
- [ ] Assign contributors
- [ ] Track progress in this file

Last updated: 2025-12-22
