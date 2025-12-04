// Syslog destination - sends logs via syslog protocol (UDP/TCP)
//
// DESIGN: Supports both UDP and TCP syslog. Uses RFC 3164 format
// (traditional syslog). This is compatible with rsyslog, syslog-ng,
// and most syslog servers.
//
// Format: <PRIORITY>TIMESTAMP HOSTNAME TAG: MESSAGE
// Example: <134>Dec  3 14:23:45 myhost flicker: [/var/log/app.log] Log message

use super::{Destination, LogEntry};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Local;
use std::io::Write;
use std::net::TcpStream;
use std::net::{ToSocketAddrs, UdpSocket};

pub struct SyslogDestination {
    host: String,
    port: u16,
    protocol: SyslogProtocol,
    // DESIGN CHOICE: Use blocking I/O for syslog
    // Syslog is fire-and-forget (UDP) or simple (TCP), so blocking is fine
    // and simpler than async sockets for this use case
}

#[derive(Clone, Copy)]
enum SyslogProtocol {
    Udp,
    Tcp,
}

impl SyslogDestination {
    pub fn new(host: String, port: u16, protocol: &str) -> Result<Self> {
        let protocol = match protocol.to_lowercase().as_str() {
            "udp" => SyslogProtocol::Udp,
            "tcp" => SyslogProtocol::Tcp,
            _ => anyhow::bail!("Invalid syslog protocol: {} (use 'udp' or 'tcp')", protocol),
        };

        Ok(Self {
            host,
            port,
            protocol,
        })
    }

    /// Format log entry as RFC 3164 syslog message
    /// Priority: <FACILITY * 8 + SEVERITY>
    /// Facility 16 = local0, Severity 6 = info
    fn format_syslog_message(&self, entry: &LogEntry) -> String {
        let priority = 134; // local0.info (16 * 8 + 6)
        let timestamp = Local::now().format("%b %d %H:%M:%S");
        let hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "unknown".to_string());

        // Format: <PRIORITY>TIMESTAMP HOSTNAME TAG: MESSAGE
        format!(
            "<{}>{} {} flicker: [{}] {}",
            priority, timestamp, hostname, entry.path, entry.line
        )
    }

    #[allow(dead_code)]
    fn send_udp(&self, messages: &[String]) -> Result<()> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        let addr = format!("{}:{}", self.host, self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow::anyhow!("Failed to resolve syslog address"))?;

        for message in messages {
            socket.send_to(message.as_bytes(), addr)?;
        }

        Ok(())
    }

    #[allow(dead_code)]
    fn send_tcp(&self, messages: &[String]) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        let mut stream = TcpStream::connect(&addr)?;

        // DESIGN CHOICE: Send all messages in one connection
        // More efficient than reconnecting for each message
        for message in messages {
            // RFC 6587: Newline-delimited messages
            stream.write_all(message.as_bytes())?;
            stream.write_all(b"\n")?;
        }

        stream.flush()?;
        Ok(())
    }
}

#[async_trait]
impl Destination for SyslogDestination {
    async fn send(&self, entry: LogEntry) -> Result<()> {
        self.send_batch(vec![entry]).await
    }

    async fn send_batch(&self, entries: Vec<LogEntry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        println!(
            "[SYSLOG] Sending batch of {} entries to {}:{} ({})",
            entries.len(),
            self.host,
            self.port,
            match self.protocol {
                SyslogProtocol::Udp => "UDP",
                SyslogProtocol::Tcp => "TCP",
            }
        );

        // Format all entries as syslog messages
        let messages: Vec<String> = entries
            .iter()
            .map(|entry| self.format_syslog_message(entry))
            .collect();

        // DESIGN CHOICE: Use spawn_blocking for sync I/O in async context
        // Syslog uses blocking sockets, so we move it to a thread pool
        // to avoid blocking the async runtime
        let host = self.host.clone();
        let port = self.port;
        let protocol = self.protocol;

        tokio::task::spawn_blocking(move || {
            match protocol {
                SyslogProtocol::Udp => {
                    let socket = UdpSocket::bind("0.0.0.0:0")?;
                    let addr = format!("{}:{}", host, port)
                        .to_socket_addrs()?
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("Failed to resolve syslog address"))?;

                    for message in &messages {
                        socket.send_to(message.as_bytes(), addr)?;
                    }
                }
                SyslogProtocol::Tcp => {
                    let addr = format!("{}:{}", host, port);
                    let mut stream = TcpStream::connect(&addr)?;

                    for message in &messages {
                        stream.write_all(message.as_bytes())?;
                        stream.write_all(b"\n")?;
                    }

                    stream.flush()?;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .await??;

        println!("[SYSLOG] Batch sent successfully");

        Ok(())
    }
}
