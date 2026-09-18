//! A deliberately tiny HTTP/1.1 client for loopback calls (Ollama for
//! `ctx eval`, the daemon health check for `ctx doctor`). Plain HTTP to
//! localhost only, so there's no reason to pull in a TLS stack.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{Context, Result, bail};

struct Url<'a> {
    host: &'a str,
    port: u16,
    path: &'a str,
}

fn parse(url: &str) -> Result<Url<'_>> {
    let rest = url
        .strip_prefix("http://")
        .with_context(|| format!("only http:// URLs are supported: {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().context("bad port")?),
        None => (authority, 80),
    };
    Ok(Url { host, port, path })
}

/// Send a request and return `(status, body)`.
pub fn request(
    method: &str,
    url: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<(u16, String)> {
    let u = parse(url)?;
    let addr = (u.host, u.port)
        .to_socket_addrs()?
        .next()
        .with_context(|| format!("cannot resolve {}", u.host))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(5)))
        .with_context(|| format!("cannot connect to {}:{}", u.host, u.port))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let body = body.unwrap_or("");
    write!(
        stream,
        "{method} {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        u.path,
        u.host,
        u.port,
        body.len()
    )?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let Some((head, payload)) = text.split_once("\r\n\r\n") else {
        bail!("malformed HTTP response");
    };
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .context("malformed HTTP status line")?;
    let chunked = head.lines().any(|l| {
        l.to_ascii_lowercase().starts_with("transfer-encoding:")
            && l.to_ascii_lowercase().contains("chunked")
    });
    let body = if chunked {
        dechunk(payload)
    } else {
        payload.to_owned()
    };
    Ok((status, body))
}

fn dechunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((size_line, rest)) = s.split_once("\r\n") {
        let size = usize::from_str_radix(size_line.split(';').next().unwrap_or("0").trim(), 16)
            .unwrap_or(0);
        if size == 0 || rest.len() < size {
            break;
        }
        out.push_str(&rest[..size]);
        s = rest[size..].trim_start_matches("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn dechunks() {
        assert_eq!(
            super::dechunk("4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n"),
            "Wikipedia"
        );
    }
}
