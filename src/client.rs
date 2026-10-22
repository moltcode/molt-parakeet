//! A minimal blocking HTTP/1.1 client for the service's unix socket, used by
//! `molt-parakeet transcribe` and `health` so agents reuse the loaded model.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

pub fn request(
    socket: &Path,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<Reply> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("connecting to {}", socket.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;

    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    if let Some(ct) = content_type {
        head.push_str(&format!("Content-Type: {ct}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    parse(&raw)
}

fn parse(raw: &[u8]) -> Result<Reply> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("malformed response")?;
    let head = std::str::from_utf8(&raw[..split])?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .context("no status")?
        .parse()?;
    let chunked = head.lines().any(|l| {
        l.to_ascii_lowercase().starts_with("transfer-encoding:")
            && l.to_ascii_lowercase().contains("chunked")
    });
    let body = &raw[split + 4..];
    let body = if chunked {
        dechunk(body)?
    } else {
        body.to_vec()
    };
    Ok(Reply { status, body })
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .context("bad chunk")?;
        let size_str = std::str::from_utf8(&data[..line_end])?;
        let size = usize::from_str_radix(size_str.split(';').next().unwrap_or("").trim(), 16)?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size + 2 {
            bail!("truncated chunk");
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_chunked() {
        let r = parse(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nhi").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"hi"[..]));
        let r = parse(b"HTTP/1.1 404 Not Found\r\ntransfer-encoding: chunked\r\n\r\n2\r\nhi\r\n1\r\n!\r\n0\r\n\r\n").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (404, &b"hi!"[..]));
    }
}
