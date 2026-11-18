//! Thin wrapper around a blocking reqwest client (rustls, no system OpenSSL).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, LINK};
use serde::de::DeserializeOwned;

const API_TIMEOUT: Duration = Duration::from_secs(30);
const GITHUB_API: &str = "https://api.github.com/";

pub struct Http {
    client: Client,
    github_token: Option<String>,
}

impl Http {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("jdkm/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            // No overall timeout: a JDK download can take minutes.
            // API calls set their own per-request timeout.
            .timeout(None)
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
        let resp = req
            .send()
            .with_context(|| format!("request to {url} failed"))?;
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
        self.client
            .get(url)
            .timeout(API_TIMEOUT)
            .send()
            .and_then(Response::error_for_status)
            .and_then(Response::text)
            .with_context(|| format!("failed to download {url}"))
    }

    /// Start a (possibly large) download; the caller streams the body.
    pub fn get_stream(&self, url: &str) -> Result<Response> {
        self.client
            .get(url)
            .send()
            .and_then(Response::error_for_status)
            .with_context(|| format!("failed to download {url}"))
    }
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
