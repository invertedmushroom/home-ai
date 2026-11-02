//! Self-contained, SSRF-hardened public-file downloader. Node never receives network access.
use anyhow::{Result, bail, ensure};
use reqwest::{StatusCode, Url, header};
use sha2::{Digest, Sha256};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

pub const MAX_BYTES: usize = 1_048_576;
pub const MAX_REDIRECTS: usize = 3;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub const USER_AGENT: &str = "deep-url-import/0.1";

#[derive(Debug)]
pub struct Download {
    pub url: String,
    pub final_url: String,
    pub media_type: String,
    pub content: String,
    pub sha256: String,
}

pub fn valid_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 160
        || name.contains('/')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        || name == "."
        || name == ".."
        || name.ends_with('.')
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let windows_device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    !windows_device
        && !matches!(
            name.to_ascii_lowercase().as_str(),
            "output" | "inputs" | "__compute.cjs"
        )
}

fn valid_public_domain_url(value: &str) -> bool {
    if value.len() > 2048 || value.bytes().any(|c| c <= 32 || c == 127) {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.as_str().len() > 2048
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    host.contains('.')
        && host.len() <= 253
        && !host.ends_with('.')
        && host.parse::<IpAddr>().is_err()
        && !host.starts_with('[')
        && ![
            "localhost",
            "local",
            "internal",
            "test",
            "invalid",
            "example",
        ]
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

pub fn parse_url(value: &str) -> Result<Url> {
    ensure!(
        valid_public_domain_url(value),
        "Import requires a public HTTP(S) domain URL on its default port, without credentials."
    );
    let url = Url::parse(value)?;
    let host = url.host_str().expect("validated domain");
    ensure!(
        !["arpa", "onion", "alt"]
            .iter()
            .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}"))),
        "Import cannot use special-use network names."
    );
    ensure!(
        url.fragment().is_none(),
        "Import URLs cannot contain fragments."
    );
    Ok(url)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192
                    && (b == 168 || (b == 0 && (c == 0 || c == 2)) || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Only ordinary global unicast. Exclude special-purpose 2001::/23,
            // documentation, 6to4 and newer documentation allocation 3fff::/20.
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

fn checked_addresses(addresses: Vec<SocketAddr>) -> Result<Vec<SocketAddr>> {
    ensure!(
        !addresses.is_empty()
            && addresses.len() <= 16
            && addresses.iter().all(|a| public_ip(a.ip())),
        "Import destination must resolve only to public Internet addresses."
    );
    Ok(addresses)
}

fn is_redirect_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

async fn request(url: &Url, addresses: &[SocketAddr]) -> Result<reqwest::Response> {
    // New client for every hop: no connection-pool, DNS, proxy, cookie or auth
    // state can bypass the exact addresses approved for this destination.
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .resolve_to_addrs(url.host_str().expect("validated domain"), addresses)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()?;
    client
        .get(url.clone())
        .header(header::USER_AGENT, USER_AGENT)
        .header(header::ACCEPT_ENCODING, "identity")
        .header(
            header::ACCEPT,
            "text/csv, text/tab-separated-values, application/json, text/plain, text/markdown",
        )
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("File download failed or timed out."))
}

fn media_type(value: &str) -> Result<String> {
    let mut parts = value.split(';');
    let raw_mime = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let mime = match raw_mime.as_str() {
        "text/csv" | "application/csv" => "text/csv",
        "text/tab-separated-values" => "text/tab-separated-values",
        "application/json" => "application/json",
        "text/plain" => "text/plain",
        "text/markdown" | "text/x-markdown" => "text/markdown",
        "text/html" | "application/xhtml+xml" => {
            bail!(
                "Import returned an unsupported content type (HTML webpage); import_url only downloads CSV, TSV, JSON, plain text or Markdown files."
            );
        }
        _ => {
            bail!(
                "Import supports CSV, TSV, JSON, plain text and Markdown; this response has an unsupported content type."
            );
        }
    };
    let mut seen_charset = false;
    for parameter in parts {
        if let Some((key, value)) = parameter.trim().split_once('=')
            && key.trim().eq_ignore_ascii_case("charset")
        {
            ensure!(!seen_charset, "Import response declared multiple charsets.");
            seen_charset = true;
            let charset = value.trim().trim_matches('"');
            ensure!(
                charset.eq_ignore_ascii_case("utf-8") || charset.eq_ignore_ascii_case("us-ascii"),
                "Import requires UTF-8 text."
            );
        }
    }
    Ok(mime.to_string())
}

fn decode(bytes: Vec<u8>, mime: &str) -> Result<String> {
    let text =
        String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("Import returned invalid UTF-8."))?;
    ensure!(!text.contains('\0'), "Import returned binary content.");
    let start = text
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    ensure!(
        !start.starts_with("<!doctype html") && !start.starts_with("<html"),
        "Import returned an HTML page instead of a data file."
    );
    if mime == "application/json" {
        serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
            .map_err(|_| anyhow::anyhow!("Import returned invalid JSON."))?;
    }
    Ok(text)
}

async fn resolve_public(url: Url) -> Result<Vec<SocketAddr>> {
    let addresses = tokio::net::lookup_host((
        url.host_str().expect("validated domain"),
        url.port_or_known_default().unwrap(),
    ))
    .await
    .map_err(|_| anyhow::anyhow!("Import destination DNS lookup failed."))?
    .take(17)
    .collect();
    checked_addresses(addresses)
}

async fn retrieve<F, Fut>(original: Url, resolve: F) -> Result<Download>
where
    F: Fn(Url) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<SocketAddr>>>,
{
    let work = async {
        let mut url = original.clone();
        for hop in 0..=MAX_REDIRECTS {
            let addresses = resolve(url.clone()).await?;
            let mut response = request(&url, &addresses).await?;
            if is_redirect_status(response.status()) {
                ensure!(hop < MAX_REDIRECTS, "Import exceeded three redirects.");
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .map(str::trim)
                    .filter(|loc| {
                        !loc.is_empty()
                            && loc.len() <= 2048
                            && !loc.bytes().any(|b| b < 32 || b == 127)
                    })
                    .ok_or_else(|| anyhow::anyhow!("Import redirect has no valid destination."))?;
                let next = url
                    .join(location)
                    .map_err(|_| anyhow::anyhow!("Invalid import redirect."))?;
                let next = parse_url(next.as_str())?;
                ensure!(
                    url.scheme() != "https" || next.scheme() == "https",
                    "Import cannot redirect HTTPS to HTTP."
                );
                drop(response);
                url = next;
                continue;
            }
            ensure!(
                response.status() == StatusCode::OK,
                "File download returned HTTP {}.",
                response.status().as_u16()
            );
            ensure!(
                response
                    .headers()
                    .get(header::CONTENT_ENCODING)
                    .is_none_or(|h| h == "identity"),
                "Compressed import responses are not supported."
            );
            let mime = media_type(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or_default(),
            )?;
            ensure!(
                response
                    .content_length()
                    .is_none_or(|n| n <= MAX_BYTES as u64),
                "Import exceeds the 1 MiB limit."
            );
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow::anyhow!("File download transport failed."))?
            {
                ensure!(
                    bytes.len() + chunk.len() <= MAX_BYTES,
                    "Import exceeds the 1 MiB limit."
                );
                bytes.extend_from_slice(&chunk);
            }
            let sha256 = format!("{:x}", Sha256::digest(&bytes));
            let content = decode(bytes, &mime)?;
            return Ok(Download {
                url: original.to_string(),
                final_url: url.to_string(),
                media_type: mime,
                content,
                sha256,
            });
        }
        unreachable!()
    };
    tokio::time::timeout(REQUEST_TIMEOUT, work)
        .await
        .map_err(|_| anyhow::anyhow!("File import exceeded its 15-second deadline."))?
}

pub async fn download(value: &str) -> Result<Download> {
    retrieve(parse_url(value)?, resolve_public).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_nonpublic_addresses_even_in_mixed_dns_answers() {
        for value in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "192.0.0.9",
            "192.0.2.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::ffff:8.8.8.8",
            "64:ff9b::a00:1",
            "64:ff9b:1::a00:1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2001::1",
            "2002:0808:0808::",
            "3fff::1",
        ] {
            let ip: IpAddr = value.parse().unwrap();
            assert!(!public_ip(ip), "{value}");
            assert!(
                checked_addresses(vec![
                    "8.8.8.8:443".parse().unwrap(),
                    SocketAddr::new(ip, 443)
                ])
                .is_err()
            );
        }
        for value in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(public_ip(value.parse().unwrap()), "{value}");
        }
        assert!(checked_addresses(vec![]).is_err());
        assert!(checked_addresses(vec!["8.8.8.8:443".parse().unwrap(); 17]).is_err());
    }
    #[test]
    fn validates_urls_names_and_text_without_changing_bytes() {
        for value in [
            "file:///tmp/a",
            "http://127.0.0.1/a",
            "https://user:pw@example.com/a",
            "https://example.com:8443/a",
            "https://example.local/a",
            "http://router.home.arpa/data.csv",
            "http://subdomain.onion/data.csv",
            "http://network.alt/data.csv",
            "https://example.com/a#part",
        ] {
            assert!(parse_url(value).is_err(), "{value}");
        }
        for name in ["../x.csv", "a/b.csv", "output", "__compute.cjs", "CON", ""] {
            assert!(!valid_name(name), "{name}");
        }
        assert!(valid_name("age.csv"));
        assert_eq!(media_type("text/csv; charset=UTF-8").unwrap(), "text/csv");
        assert_eq!(
            media_type("application/csv; charset=us-ascii").unwrap(),
            "text/csv"
        );
        assert_eq!(
            media_type("text/x-markdown; charset=\"utf-8\"").unwrap(),
            "text/markdown"
        );
        for mime in [
            "text/html",
            "application/xhtml+xml",
            "application/octet-stream",
            "text/plain; charset=windows-1252",
            "text/plain; charset=utf-8; charset=utf-8",
        ] {
            assert!(media_type(mime).is_err(), "{mime}");
        }
        for (text, mime) in [
            ("<!DOCTYPE html><html>Error</html>", "text/plain"),
            ("{bad", "application/json"),
            ("\0", "text/plain"),
        ] {
            assert!(decode(text.as_bytes().to_vec(), mime).is_err());
        }
        let bytes = "\u{feff}Age\r\n42\r\n";
        assert_eq!(
            decode(bytes.as_bytes().to_vec(), "text/csv").unwrap(),
            bytes
        );
    }
    #[tokio::test]
    async fn request_uses_pinned_addresses_without_following_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 2048];
            let n = stream.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]).to_ascii_lowercase();
            assert!(request.contains("host: download.example.com"));
            assert!(request.contains("user-agent: deep-url-import/0.1"));
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        // Test transport only: production checked_addresses rejects loopback.
        let response = request(
            &Url::parse(&format!(
                "http://download.example.com:{}/file",
                address.port()
            ))
            .unwrap(),
            &[address],
        )
        .await
        .unwrap();
        assert_eq!(response.status().as_u16(), 302);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn retrieval_checks_redirects_types_sizes_and_exact_source_bytes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let cases = [
            (
                "302 Found",
                "Location: http://127.0.0.1/private\r\n",
                "",
                false,
            ),
            ("302 Found", "Location: /again\r\n", "", false),
            (
                "304 Not Modified",
                "Location: https://download.example.com/other.csv\r\n",
                "",
                false,
            ),
            (
                "200 OK",
                "Content-Type: text/html\r\n",
                "<html>error</html>",
                false,
            ),
            (
                "200 OK",
                "Content-Type: text/csv\r\nContent-Encoding: gzip\r\n",
                "Age\n42\n",
                false,
            ),
            (
                "200 OK",
                "Content-Type: application/json\r\n",
                "{invalid",
                false,
            ),
            (
                "200 OK",
                "Content-Type: text/csv; charset=utf-8\r\n",
                "Age\r\n42\r\n",
                true,
            ),
        ];
        for (status, headers, body, success) in cases {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = [0; 2048];
                    assert!(stream.read(&mut request).await.unwrap() > 0);
                    let response = format!(
                        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                }
            });
            // Resolver substitution exercises the production transport/parser on
            // a fixture server; real DNS additionally enforces checked_addresses.
            let result = retrieve(
                Url::parse("http://download.example.com/data.csv").unwrap(),
                |_| async { Ok(vec![address]) },
            )
            .await;
            assert_eq!(result.is_ok(), success, "{status} {headers}");
            if !success {
                let expected = if headers.contains("127.0.0.1") {
                    "public HTTP(S)"
                } else if status.starts_with("302") {
                    "three redirects"
                } else if status.starts_with("304") {
                    "HTTP 304"
                } else if headers.contains("text/html") {
                    "unsupported content type"
                } else if headers.contains("gzip") {
                    "Compressed"
                } else {
                    "invalid JSON"
                };
                assert!(
                    result.as_ref().unwrap_err().to_string().contains(expected),
                    "{result:?}"
                );
            }
            if let Ok(file) = result {
                assert_eq!(file.content, body);
                assert_eq!(
                    file.sha256,
                    format!("{:x}", Sha256::digest(body.as_bytes()))
                );
                assert_eq!(file.url, "http://download.example.com/data.csv");
            }
            server.abort();
        }
        // Chunked bodies without Content-Length must obey the same byte limit.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            assert!(stream.read(&mut request).await.unwrap() > 0);
            let body = "x".repeat(MAX_BYTES + 1);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
        assert!(
            retrieve(
                Url::parse("http://download.example.com/data.csv").unwrap(),
                |_| async { Ok(vec![address]) }
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("1 MiB")
        );
        server.await.unwrap();
    }
}
