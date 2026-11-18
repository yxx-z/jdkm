//! A tiny JSON file cache with a time-to-live, used for remote release
//! lists so we stay well under GitHub's anonymous rate limit.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Serialize, Deserialize)]
struct Entry<T> {
    /// Seconds since the Unix epoch.
    fetched_at: u64,
    data: T,
}

/// Return the cached value at `path` if it is younger than `ttl` (and
/// `refresh` is false); otherwise call `fetch` and cache its result.
/// If fetching fails but a stale copy exists, the stale copy is used.
pub fn cached<T, F>(path: &Path, ttl: Duration, refresh: bool, fetch: F) -> Result<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> Result<T>,
{
    let now = now_secs();
    let existing: Option<Entry<T>> = fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok()); // a corrupt cache is just a miss

    if let Some(entry) = &existing
        && !refresh
        && now.saturating_sub(entry.fetched_at) < ttl.as_secs()
    {
        return Ok(existing.unwrap().data);
    }

    match fetch() {
        Ok(data) => {
            let entry = Entry {
                fetched_at: now,
                data,
            };
            // Failing to write the cache must not fail the command.
            if let Err(err) = write(path, &entry) {
                eprintln!("warning: could not write cache {}: {err:#}", path.display());
            }
            Ok(entry.data)
        }
        Err(err) => match existing {
            Some(entry) => {
                eprintln!(
                    "warning: {err:#}\nwarning: using cached data from {}",
                    path.display()
                );
                Ok(entry.data)
            }
            None => Err(err),
        },
    }
}

fn write<T: Serialize>(path: &Path, entry: &Entry<T>) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(entry)?).context("write")?;
    fs::rename(&tmp, path).context("rename")?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::bail;
    use std::cell::Cell;

    #[test]
    fn caches_until_expired_or_refreshed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        let calls = Cell::new(0);
        let fetch = || {
            calls.set(calls.get() + 1);
            Ok(vec![calls.get()])
        };

        assert_eq!(cached(&path, DEFAULT_TTL, false, fetch).unwrap(), [1]);
        assert_eq!(cached(&path, DEFAULT_TTL, false, fetch).unwrap(), [1]); // hit
        assert_eq!(cached(&path, DEFAULT_TTL, true, fetch).unwrap(), [2]); // refresh
        assert_eq!(cached(&path, Duration::ZERO, false, fetch).unwrap(), [3]); // expired
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn falls_back_to_stale_data_when_fetch_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        cached(&path, DEFAULT_TTL, false, || Ok(vec![7])).unwrap();

        let failing = || -> Result<Vec<i32>> { bail!("network down") };
        assert_eq!(cached(&path, Duration::ZERO, false, failing).unwrap(), [7]);

        let missing = tmp.path().join("none.json");
        assert!(cached(&missing, DEFAULT_TTL, false, failing).is_err());
    }

    #[test]
    fn corrupt_cache_is_a_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        fs::write(&path, "not json").unwrap();
        assert_eq!(cached(&path, DEFAULT_TTL, false, || Ok(5)).unwrap(), 5);
    }
}
