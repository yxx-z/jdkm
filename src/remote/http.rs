//! Thin wrapper around a blocking reqwest client (rustls, no system OpenSSL).

use std::io::{self, Read};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, LINK, RANGE};
use serde::de::DeserializeOwned;

const API_TIMEOUT: Duration = Duration::from_secs(30);
const GITHUB_API: &str = "https://api.github.com/";

#[derive(Clone)]
pub struct Http {
    client: Client,
    github_token: Option<String>,
}

impl Http {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("jdkm/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            // In reqwest's blocking client this bounds the wait for the
            // response headers and for *each* read of the body, not the
            // whole transfer: a download that stalls for 30 s fails (and
            // can resume or switch source), a slow one that keeps moving
            // does not. Requests that need a total deadline set their own.
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to create HTTP client")?;
        let github_token = std::env::var("GITHUB_TOKEN")
            .ok()
            .filter(|t| !t.trim().is_empty());
        Ok(Self {
            client,
            github_token,
        })
    }

    /// GET a GitHub API URL and decode the JSON body. Also returns the
    /// `rel="next"` page URL from the `Link` header, if any.
    pub fn github_json<T: DeserializeOwned>(&self, url: &str) -> Result<(T, Option<String>)> {
        let mut req = self
            .client
            .get(url)
            .timeout(API_TIMEOUT)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        // Only ever send the token to the API host itself.
        if let Some(token) = &self.github_token
            && url.starts_with(GITHUB_API)
        {
            req = req.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let resp = send(req, url)?;
        check_rate_limit(&resp)?;
        let resp = resp
            .error_for_status()
            .with_context(|| format!("GitHub API request failed: {url}"))?;
        let next = next_page(resp.headers());
        let body = resp
            .text()
            .with_context(|| format!("failed to read {url}"))?;
        let value = serde_json::from_str(&body)
            .with_context(|| format!("unexpected response from {url}"))?;
        Ok((value, next))
    }

    /// Like [`github_json`], but a 404 becomes `Ok(None)`.
    pub fn github_json_opt<T: DeserializeOwned>(&self, url: &str) -> Result<Option<T>> {
        match self.github_json(url) {
            Ok((value, _)) => Ok(Some(value)),
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// GET a small text resource (e.g. a checksum file).
    pub fn get_text(&self, url: &str) -> Result<String> {
        self.get_text_within(url, API_TIMEOUT)
    }

    fn get_text_within(&self, url: &str, timeout: Duration) -> Result<String> {
        send(self.client.get(url).timeout(timeout), url)?
            .error_for_status()
            .and_then(Response::text)
            .with_context(|| format!("failed to download {url}"))
    }

    /// GET and decode a JSON document, giving up after `timeout`.
    pub fn json<T: DeserializeOwned>(&self, url: &str, timeout: Duration) -> Result<T> {
        serde_json::from_str(&self.get_text_within(url, timeout)?)
            .with_context(|| format!("unexpected response from {url}"))
    }

    /// Read the first `bytes` of `url` within `timeout`.
    pub fn probe(&self, url: &str, bytes: u64, timeout: Duration) -> Result<()> {
        let request = self
            .client
            .get(url)
            .header(RANGE, format!("bytes=0-{}", bytes - 1))
            .timeout(timeout);
        let response = send(request, url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        let copied = io::copy(&mut response.take(bytes), &mut io::sink())?;
        ensure!(copied == bytes, "short probe response");
        Ok(())
    }

    /// Download from `offset` for up to `window` (or `max` bytes) and
    /// return the observed throughput in bytes per second. The data is
    /// discarded; this only answers "would this source be faster?".
    pub fn measure(&self, url: &str, offset: u64, max: u64, window: Duration) -> Result<f64> {
        let start = Instant::now();
        let request = self
            .client
            .get(url)
            .header(RANGE, format!("bytes={offset}-"))
            .timeout(window * 2);
        let mut response = send(request, url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = 0u64;
        while got < max && start.elapsed() < window {
            let n = response.read(&mut buf)?;
            if n == 0 {
                break;
            }
            got += n as u64;
        }
        ensure!(got > 0, "no data received");
        Ok(got as f64 / start.elapsed().as_secs_f64())
    }

    /// Start a (possibly large) download at byte `offset`; the caller
    /// streams the body and must check for `206 Partial Content` when
    /// `offset > 0`, since a server may ignore the range and send it all.
    pub fn get_from(&self, url: &str, offset: u64) -> Result<Response> {
        let mut request = self.client.get(url);
        if offset > 0 {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        send(request, url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)
            .with_context(|| format!("failed to download {url}"))
    }
}

/// Send a request; connection problems get a hint about network/proxy.
fn send(req: RequestBuilder, url: &str) -> Result<Response> {
    req.send().map_err(|err| {
        let hint = if err.is_connect() || err.is_timeout() {
            let host = err
                .url()
                .and_then(|u| u.host_str())
                .unwrap_or(url)
                .to_string();
            format!(
                "cannot reach {host}. Check your network connection or proxy settings \
                 (HTTPS_PROXY / ALL_PROXY)"
            )
        } else {
            format!("request to {url} failed")
        };
        anyhow::Error::new(err.without_url()).context(hint)
    })
}

/// Did this request fail with HTTP 404?
pub fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain()
        .filter_map(|e| e.downcast_ref::<reqwest::Error>())
        .any(|e| e.status() == Some(StatusCode::NOT_FOUND))
}

/// Turn GitHub's rate-limit response into an actionable message.
fn check_rate_limit(resp: &Response) -> Result<()> {
    let status = resp.status();
    if status != StatusCode::FORBIDDEN && status != StatusCode::TOO_MANY_REQUESTS {
        return Ok(());
    }
    let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok());
    if header("x-ratelimit-remaining") != Some("0") {
        return Ok(());
    }
    let wait = header("x-ratelimit-reset")
        .and_then(|v| v.parse::<u64>().ok())
        .and_then(|reset| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
            Some(reset.saturating_sub(now) / 60 + 1)
        });
    match wait {
        Some(min) => bail!(
            "GitHub API rate limit exceeded (resets in about {min} min). \
             Set GITHUB_TOKEN to raise the limit."
        ),
        None => bail!("GitHub API rate limit exceeded. Set GITHUB_TOKEN to raise the limit."),
    }
}

/// Extract the `rel="next"` URL from a `Link` header:
/// `<https://...&page=2>; rel="next", <https://...&page=3>; rel="last"`
fn next_page(headers: &HeaderMap) -> Option<String> {
    let link = headers.get(LINK)?.to_str().ok()?;
    parse_next_link(link)
}

fn parse_next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        params
            .split(';')
            .any(|p| p.trim() == r#"rel="next""#)
            .then(|| {
                url.trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string()
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_link_header() {
        let link = r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=3>; rel="last""#;
        assert_eq!(
            parse_next_link(link).as_deref(),
            Some("https://api.github.com/x?page=2")
        );

        let last_page = r#"<https://api.github.com/x?page=1>; rel="prev", <https://api.github.com/x?page=1>; rel="first""#;
        assert_eq!(parse_next_link(last_page), None);
    }
}
