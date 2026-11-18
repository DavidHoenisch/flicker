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
use std::io::{ErrorKind, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Largest UDP syslog message sent; longer messages are truncated.
/// (UDP payloads max out at 65,507 bytes over IPv4.)
const MAX_UDP_MESSAGE_BYTES: usize = 65_000;

pub struct SyslogDestination {
    host: String,
    port: u16,
    protocol: SyslogProtocol,
    framing: TcpFraming,
    timeout: Duration,
    connect_timeout: Duration,
    /// Looked up once; it doesn't change while we run
    hostname: String,
    // DESIGN CHOICE: Use blocking I/O for syslog, run via spawn_blocking.
    // Sockets are kept between batches instead of reconnecting every time.
    tcp: Arc<Mutex<Option<TcpStream>>>,
    udp: Arc<Mutex<Option<UdpSocket>>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SyslogProtocol {
    Udp,
    Tcp,
}

/// How messages are delimited on a TCP stream (RFC 6587)
#[derive(Clone, Copy, Debug, PartialEq)]
enum TcpFraming {
    /// Newline-terminated; embedded newlines are escaped as `\n` / `\r` so a
    /// multi-line entry (stack trace, pretty JSON) stays one message
    Newline,
    /// `<length> <message>`; embedded newlines are sent as-is
    OctetCounting,
}

impl SyslogDestination {
    pub fn new(
        host: String,
        port: u16,
        protocol: &str,
        framing: Option<&str>,
        timeout: Duration,
        connect_timeout: Duration,
    ) -> Result<Self> {
        let protocol = match protocol.to_lowercase().as_str() {
            "udp" => SyslogProtocol::Udp,
            "tcp" => SyslogProtocol::Tcp,
            _ => anyhow::bail!("Invalid syslog protocol: {} (use 'udp' or 'tcp')", protocol),
        };
        let framing = match framing.map(|f| f.to_lowercase()).as_deref() {
            None | Some("newline") => TcpFraming::Newline,
            Some("octet-counting") | Some("octet_counting") => TcpFraming::OctetCounting,
            Some(other) => anyhow::bail!(
                "Invalid syslog framing: {} (use 'newline' or 'octet-counting')",
                other
            ),
        };
        let hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "unknown".to_string());

        Ok(Self {
            host,
            port,
            protocol,
            framing,
            timeout,
            connect_timeout,
            hostname,
            tcp: Arc::new(Mutex::new(None)),
            udp: Arc::new(Mutex::new(None)),
        })
    }

    /// Format log entry as RFC 3164 syslog message
    /// Priority: <FACILITY * 8 + SEVERITY>
    /// Facility 16 = local0, Severity 6 = info
    fn format_syslog_message(&self, entry: &LogEntry) -> String {
        let priority = 134; // local0.info (16 * 8 + 6)
        let timestamp = entry
            .timestamp
            .with_timezone(&Local)
            .format("%b %d %H:%M:%S");

        // Format: <PRIORITY>TIMESTAMP HOSTNAME TAG: MESSAGE
        format!(
            "<{}>{} {} flicker: [{}] {}",
            priority, timestamp, self.hostname, entry.path, entry.line
        )
    }

    /// Frame one message for a TCP stream
    fn frame_tcp(&self, message: &str) -> Vec<u8> {
        match self.framing {
            TcpFraming::Newline => {
                let mut out = message.replace('\n', "\\n").replace('\r', "\\r");
                out.push('\n');
                out.into_bytes()
            }
            TcpFraming::OctetCounting => format!("{} {}", message.len(), message).into_bytes(),
        }
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

        let host = self.host.clone();
        let port = self.port;
        match self.protocol {
            SyslogProtocol::Udp => {
                let socket = self.udp.clone();
                tokio::task::spawn_blocking(move || send_udp(&socket, &host, port, &messages))
                    .await??;
            }
            SyslogProtocol::Tcp => {
                let payload: Vec<u8> = messages.iter().flat_map(|m| self.frame_tcp(m)).collect();
                let stream = self.tcp.clone();
                let (timeout, connect_timeout) = (self.timeout, self.connect_timeout);
                tokio::task::spawn_blocking(move || {
                    send_tcp(&stream, &host, port, timeout, connect_timeout, &payload)
                })
                .await??;
            }
        }

        println!("[SYSLOG] Batch sent successfully");

        Ok(())
    }
}

/// Send each message as one datagram, reusing the socket between batches
fn send_udp(
    socket: &Mutex<Option<UdpSocket>>,
    host: &str,
    port: u16,
    messages: &[String],
) -> Result<()> {
    let addr = (host, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve syslog address"))?;

    let mut guard = socket.lock().unwrap_or_else(|e| e.into_inner());
    if guard
        .as_ref()
        .and_then(|s| s.local_addr().ok())
        .is_none_or(|local| local.is_ipv4() != addr.is_ipv4())
    {
        let bind: SocketAddr = if addr.is_ipv4() {
            "0.0.0.0:0".parse()?
        } else {
            "[::]:0".parse()?
        };
        *guard = Some(UdpSocket::bind(bind)?);
    }
    let socket = guard.as_ref().unwrap();

    let mut truncated = 0;
    for message in messages {
        let bytes = truncate_to_char_boundary(message, MAX_UDP_MESSAGE_BYTES);
        if bytes.len() < message.len() {
            truncated += 1;
        }
        socket.send_to(bytes.as_bytes(), addr)?;
    }
    if truncated > 0 {
        eprintln!(
            "[SYSLOG] Truncated {} message(s) to {} bytes for UDP; use protocol: tcp for long lines",
            truncated, MAX_UDP_MESSAGE_BYTES
        );
    }
    Ok(())
}

/// Longest prefix of `s` that fits in `max` bytes without splitting a character
fn truncate_to_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Write a framed payload over a persistent TCP connection, reconnecting if needed.
/// DESIGN CHOICE: Timeouts keep a hung server from pinning a blocking thread
/// (and the source's poll loop) forever.
fn send_tcp(
    stream: &Mutex<Option<TcpStream>>,
    host: &str,
    port: u16,
    timeout: Duration,
    connect_timeout: Duration,
    payload: &[u8],
) -> Result<()> {
    let mut guard = stream.lock().unwrap_or_else(|e| e.into_inner());

    // Drop a connection the server has closed since the last batch; writes to it
    // could otherwise appear to succeed and be lost.
    if guard.as_ref().is_some_and(|s| !is_connection_alive(s)) {
        *guard = None;
    }

    let reused = guard.is_some();
    if guard.is_none() {
        *guard = Some(connect(host, port, timeout, connect_timeout)?);
    }
    let result = guard
        .as_mut()
        .unwrap()
        .write_all(payload)
        .and_then(|_| guard.as_mut().unwrap().flush());

    match result {
        Ok(()) => Ok(()),
        // A reused connection may have died; retry once on a fresh one
        Err(_) if reused => {
            let mut fresh = connect(host, port, timeout, connect_timeout)?;
            let retry = fresh.write_all(payload).and_then(|_| fresh.flush());
            match retry {
                Ok(()) => {
                    *guard = Some(fresh);
                    Ok(())
                }
                Err(e) => {
                    *guard = None;
                    Err(e.into())
                }
            }
        }
        Err(e) => {
            *guard = None;
            Err(e.into())
        }
    }
}

fn connect(
    host: &str,
    port: u16,
    timeout: Duration,
    connect_timeout: Duration,
) -> Result<TcpStream> {
    let mut last_err = None;
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, connect_timeout) {
            Ok(stream) => {
                stream.set_write_timeout(Some(timeout))?;
                stream.set_read_timeout(Some(timeout))?;
                return Ok(stream);
            }
            Err(e) => last_err = Some(e),
        }
    }
    match last_err {
        Some(e) => Err(e.into()),
        None => anyhow::bail!("Failed to resolve syslog address"),
    }
}

/// True unless the peer has closed the connection (syslog servers never send data)
fn is_connection_alive(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let mut buf = [0u8; 1];
    let alive = match stream.peek(&mut buf) {
        Ok(0) => false,                                      // orderly shutdown
        Ok(_) => true,                                       // unexpected data, still open
        Err(e) if e.kind() == ErrorKind::WouldBlock => true, // nothing to read: open
        Err(_) => false,
    };
    stream.set_nonblocking(false).is_ok() && alive
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    fn dest(protocol: &str, port: u16, framing: Option<&str>) -> SyslogDestination {
        SyslogDestination::new(
            "127.0.0.1".to_string(),
            port,
            protocol,
            framing,
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .unwrap()
    }

    #[test]
    fn test_newline_framing_escapes_embedded_newlines() {
        let d = dest("tcp", 514, None);
        assert_eq!(d.frame_tcp("a\nb\r\nc"), b"a\\nb\\r\\nc\n".to_vec());
    }

    #[test]
    fn test_octet_counting_framing() {
        let d = dest("tcp", 514, Some("octet-counting"));
        assert_eq!(d.frame_tcp("héllo\nx"), "8 héllo\nx".as_bytes().to_vec());
    }

    #[test]
    fn test_invalid_framing_rejected() {
        assert!(
            SyslogDestination::new(
                "h".into(),
                1,
                "tcp",
                Some("bogus"),
                Duration::ZERO,
                Duration::ZERO
            )
            .is_err()
        );
    }

    #[test]
    fn test_truncate_to_char_boundary() {
        assert_eq!(truncate_to_char_boundary("abc", 10), "abc");
        assert_eq!(truncate_to_char_boundary("aé", 2), "a");
        assert_eq!(truncate_to_char_boundary("aé", 3), "aé");
    }

    #[tokio::test]
    async fn test_tcp_reuses_connection_and_reconnects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let d = dest("tcp", port, None);

        d.send_batch(vec![LogEntry::new("/a.log", "one")])
            .await
            .unwrap();
        let (mut conn, _) = listener.accept().unwrap();
        d.send_batch(vec![LogEntry::new("/a.log", "two")])
            .await
            .unwrap();

        // Both batches arrived on the same connection
        conn.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut received = String::new();
        let mut buf = [0u8; 4096];
        while !received.contains("two") {
            let n = conn.read(&mut buf).unwrap();
            received.push_str(std::str::from_utf8(&buf[..n]).unwrap());
        }
        assert!(received.contains("one"));

        // Server closes the connection; the next batch reconnects
        drop(conn);
        std::thread::sleep(Duration::from_millis(50));
        d.send_batch(vec![LogEntry::new("/a.log", "three")])
            .await
            .unwrap();
        let (mut conn2, _) = listener.accept().unwrap();
        conn2
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let n = conn2.read(&mut buf).unwrap();
        assert!(std::str::from_utf8(&buf[..n]).unwrap().contains("three"));
    }

    #[tokio::test]
    async fn test_udp_sends_message() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let port = receiver.local_addr().unwrap().port();
        let d = dest("udp", port, None);

        d.send_batch(vec![LogEntry::new("/a.log", "hello")])
            .await
            .unwrap();
        let mut buf = [0u8; 2048];
        let n = receiver.recv(&mut buf).unwrap();
        let msg = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(msg.starts_with("<134>"), "{}", msg);
        assert!(msg.ends_with("flicker: [/a.log] hello"), "{}", msg);
    }
}
