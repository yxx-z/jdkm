//! Fetching one archive from several equivalent sources (DECISIONS.md #41).
//!
//! Every source serves the same official file, whose SHA-256 was settled
//! from official metadata before any source is contacted. That hash is the
//! only thing trusted, so the bytes may come from anywhere:
//!
//! 1. A short race picks the source that answers first.
//! 2. While downloading, a dropped connection resumes on the same source
//!    with an HTTP range request, and a source that stays slow is compared
//!    against the others and replaced mid-way if one is clearly faster.
//! 3. If the finished file does not match the hash, nothing from that
//!    attempt is reused: the file is fetched again from one source at a
//!    time, so a bad source cannot hide behind a good one.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::StatusCode;
use reqwest::header::CONTENT_RANGE;
use sha2::{Digest, Sha256};

use crate::remote::Artifact;
use crate::remote::http::{self, Http};

/// Knobs for the download policy; tests shrink the durations.
#[derive(Debug, Clone)]
pub struct Tuning {
    /// Size of the initial race read.
    pub probe_bytes: u64,
    pub probe_timeout: Duration,
    /// Resume attempts on one source in a row without any new bytes.
    pub retries: u32,
    pub retry_delay: Duration,
    /// Let a connection settle before judging its speed.
    pub warmup: Duration,
    pub check_every: Duration,
    /// Don't look elsewhere while the current source is at least this fast.
    pub fast_enough: f64,
    /// Don't switch when the download is about to finish anyway.
    pub min_remaining: Duration,
    /// How long / how much to sample from another source.
    pub measure_window: Duration,
    pub measure_bytes: u64,
    /// Switch when the other source measured this many times faster.
    pub switch_ratio: f64,
    /// Before sampling the same other source again.
    pub retest_after: Duration,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            probe_bytes: 256 * 1024,
            probe_timeout: Duration::from_secs(3),
            retries: 3,
            retry_delay: Duration::from_secs(1),
            warmup: Duration::from_secs(5),
            check_every: Duration::from_secs(5),
            fast_enough: 8.0 * 1024.0 * 1024.0,
            min_remaining: Duration::from_secs(15),
            measure_window: Duration::from_secs(4),
            measure_bytes: 8 * 1024 * 1024,
            switch_ratio: 1.5,
            retest_after: Duration::from_secs(60),
        }
    }
}

/// Download `artifact` to `dest` and check it against the official SHA-256.
pub fn download_verified(
    http: &Http,
    artifact: &Artifact,
    dest: &Path,
    tuning: &Tuning,
) -> Result<()> {
    let order = rank_sources(http, artifact, tuning);
    if order.is_empty() {
        bail!(
            "{} is not available from any download source",
            artifact.file_name
        );
    }
    let names = |sources: &[usize]| -> String {
        let names: Vec<_> = sources.iter().map(|&i| artifact.sources[i].name).collect();
        names.join("+")
    };

    let file =
        File::create(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    let mut sink = Sink::new(file);
    let bar = progress_bar(artifact.size)?;
    let mut failures = Vec::new();

    // Fast path: resume and switch freely between sources.
    let fetched = Fetch {
        http,
        artifact,
        tuning,
        bar: &bar,
    }
    .run(&order, &mut sink, &mut failures)?;
    if fetched {
        if sink.sha256() == artifact.sha256 {
            return sink.finish(&bar);
        }
        failures.push(format!("{}: SHA-256 mismatch", names(&sink.contributors)));
        let suspects = sink.contributors.clone();
        say(
            &bar,
            format!(
                "warning: {} failed verification; downloading again from one source at a time",
                artifact.file_name
            ),
        );

        // Careful path: one source per attempt, so a mismatch has one culprit.
        for &source in &order {
            if suspects == [source] {
                continue; // that source alone produced the bad file
            }
            sink.restart()?;
            bar.set_position(0);
            let fetched = Fetch {
                http,
                artifact,
                tuning,
                bar: &bar,
            }
            .run(&[source], &mut sink, &mut failures)?;
            if !fetched {
                continue;
            }
            if sink.sha256() == artifact.sha256 {
                return sink.finish(&bar);
            }
            failures.push(format!("{}: SHA-256 mismatch", names(&[source])));
        }
    }
    bar.finish_and_clear();
    bail!(
        "could not download a verified {}: {}",
        artifact.file_name,
        failures.join("; ")
    )
}

/// Race a small read from every source. The first to finish goes first;
/// the rest keep their listed order (official first), sources that failed
/// the race go last, and sources that answered 404 are dropped.
fn rank_sources(http: &Http, artifact: &Artifact, tuning: &Tuning) -> Vec<usize> {
    let count = artifact.sources.len();
    if count < 2 {
        return (0..count).collect();
    }
    let bytes = match artifact.size {
        0 => tuning.probe_bytes,
        size => tuning.probe_bytes.min(size),
    };
    let (sender, receiver) = mpsc::channel();
    for (index, source) in artifact.sources.iter().enumerate() {
        let (http, sender, url) = (http.clone(), sender.clone(), source.url.clone());
        let timeout = tuning.probe_timeout;
        thread::spawn(move || {
            let result = http.probe(&url, bytes, timeout);
            let _ = sender.send((index, result.map_err(|e| http::is_not_found(&e))));
        });
    }
    drop(sender);

    let deadline = Instant::now() + tuning.probe_timeout;
    let mut winner = None;
    let mut missing = Vec::new();
    let mut failed = Vec::new();
    while winner.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok((index, Ok(()))) => winner = Some(index),
            Ok((index, Err(true))) => missing.push(index),
            Ok((index, Err(false))) => failed.push(index),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    if winner.is_none() {
        for &index in &missing {
            if let Some(note) = artifact.sources[index].missing_note {
                eprintln!("note: {note}");
            }
        }
    }
    let mut order: Vec<usize> = winner.into_iter().collect();
    order.extend(
        (0..count).filter(|i| Some(*i) != winner && !missing.contains(i) && !failed.contains(i)),
    );
    order.extend(failed);
    order
}

/// The temporary archive plus a running hash of what has been written.
struct Sink {
    file: File,
    hasher: Sha256,
    written: u64,
    /// Indexes of the sources whose bytes are in the file.
    contributors: Vec<usize>,
}

impl Sink {
    fn new(file: File) -> Self {
        Self {
            file,
            hasher: Sha256::new(),
            written: 0,
            contributors: Vec::new(),
        }
    }

    fn restart(&mut self) -> Result<()> {
        self.file.set_len(0).map_err(LocalWriteError)?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(LocalWriteError)?;
        self.hasher = Sha256::new();
        self.written = 0;
        self.contributors.clear();
        Ok(())
    }

    fn write(&mut self, source: usize, bytes: &[u8]) -> Result<()> {
        self.file.write_all(bytes).map_err(LocalWriteError)?;
        self.hasher.update(bytes);
        self.written += bytes.len() as u64;
        if !self.contributors.contains(&source) {
            self.contributors.push(source);
        }
        Ok(())
    }

    fn sha256(&self) -> String {
        to_hex(&self.hasher.clone().finalize())
    }

    fn finish(self, bar: &ProgressBar) -> Result<()> {
        bar.finish_and_clear();
        self.file.sync_all().map_err(LocalWriteError)?;
        Ok(())
    }
}

/// Disk trouble: switching sources would not help, so it aborts at once.
#[derive(Debug, thiserror::Error)]
#[error("failed to write temporary archive")]
struct LocalWriteError(#[source] std::io::Error);

enum Outcome {
    Done,
    SwitchTo(usize),
}

struct Fetch<'a> {
    http: &'a Http,
    artifact: &'a Artifact,
    tuning: &'a Tuning,
    bar: &'a ProgressBar,
}

impl Fetch<'_> {
    /// Fill `sink` to the end of the file from `order` (most preferred
    /// first). Returns false when every source gave up; their errors are
    /// appended to `failures`.
    fn run(&self, order: &[usize], sink: &mut Sink, failures: &mut Vec<String>) -> Result<bool> {
        let mut usable = order.to_vec();
        let mut current = order[0];
        let mut stuck = 0; // attempts in a row on `current` without new bytes
        let mut checker = SpeedCheck::new(self.http, self.tuning);
        self.announce(current, sink.written);
        loop {
            let before = sink.written;
            let others: Vec<usize> = usable.iter().copied().filter(|&i| i != current).collect();
            let error = match self.stream(current, sink, &others, &mut checker) {
                Ok(Outcome::Done) => return Ok(true),
                Ok(Outcome::SwitchTo(next)) => {
                    current = next;
                    stuck = 0;
                    continue;
                }
                Err(err) if err.is::<LocalWriteError>() => return Err(err),
                Err(err) => err,
            };
            let name = self.artifact.sources[current].name;
            if sink.written > before {
                stuck = 0;
            }
            stuck += 1;
            if !http::is_not_found(&error) && stuck <= self.tuning.retries {
                say(
                    self.bar,
                    format!(
                        "warning: {name}: {error:#}; resuming at {:.1} MB",
                        sink.written as f64 / 1e6
                    ),
                );
                thread::sleep(self.tuning.retry_delay * stuck);
                continue;
            }
            failures.push(format!("{name}: {error:#}"));
            usable.retain(|&i| i != current);
            let Some(&next) = usable.first() else {
                return Ok(false);
            };
            say(self.bar, format!("warning: {name}: {error:#}"));
            current = next;
            stuck = 0;
            self.announce(current, sink.written);
        }
    }

    fn announce(&self, source: usize, offset: u64) {
        let name = self.artifact.sources[source].name;
        self.bar.set_message(format!("Downloading from {name}"));
        if offset == 0 {
            say(
                self.bar,
                format!("Downloading {} from {name}...", self.artifact.file_name),
            );
        } else {
            say(
                self.bar,
                format!("Continuing from {name} at {:.1} MB...", offset as f64 / 1e6),
            );
        }
    }

    /// One HTTP request: append to `sink` from where it stands.
    fn stream(
        &self,
        source: usize,
        sink: &mut Sink,
        others: &[usize],
        checker: &mut SpeedCheck,
    ) -> Result<Outcome> {
        let size = self.artifact.size;
        if size > 0 && sink.written >= size {
            return Ok(Outcome::Done);
        }
        let url = &self.artifact.sources[source].url;
        let offset = sink.written;
        let response = self.http.get_from(url, offset)?;
        if offset > 0 {
            match response.status() {
                StatusCode::PARTIAL_CONTENT => {
                    let start = range_start(response.headers().get(CONTENT_RANGE));
                    if start != Some(offset) {
                        bail!("server resumed at the wrong position");
                    }
                }
                // The server ignored the range and is sending everything.
                _ => {
                    sink.restart()?;
                    self.bar.set_position(0);
                }
            }
        }
        let mut body: Box<dyn Read> = if size > 0 {
            Box::new(response.take(size - sink.written))
        } else {
            Box::new(response)
        };
        checker.connected(sink.written);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = body.read(&mut buf).context("download interrupted")?;
            if n == 0 {
                break;
            }
            sink.write(source, &buf[..n])?;
            self.bar.set_position(sink.written);
            if let Some((next, current_rate, next_rate)) =
                checker.poll(self.artifact, others, sink.written)
            {
                say(
                    self.bar,
                    format!(
                        "{} is slow ({}/s); {} measured {}/s, switching",
                        self.artifact.sources[source].name,
                        human(current_rate),
                        self.artifact.sources[next].name,
                        human(next_rate),
                    ),
                );
                self.announce(next, sink.written);
                return Ok(Outcome::SwitchTo(next));
            }
        }
        if size > 0 && sink.written != size {
            bail!("download incomplete: got {} of {size} bytes", sink.written);
        }
        Ok(Outcome::Done)
    }
}

/// Watches the current connection's speed and, when it is poor, samples
/// another source in the background to see whether switching would help.
struct SpeedCheck<'a> {
    http: &'a Http,
    tuning: &'a Tuning,
    /// Start of the current connection.
    connected_at: Instant,
    /// Last speed reading: when, and how much had been written then.
    mark: (Instant, u64),
    sampling: Option<Sample>,
    last_sampled: HashMap<usize, Instant>,
}

struct Sample {
    source: usize,
    since: (Instant, u64),
    result: Receiver<Result<f64>>,
}

impl<'a> SpeedCheck<'a> {
    fn new(http: &'a Http, tuning: &'a Tuning) -> Self {
        let now = Instant::now();
        Self {
            http,
            tuning,
            connected_at: now,
            mark: (now, 0),
            sampling: None,
            last_sampled: HashMap::new(),
        }
    }

    fn connected(&mut self, written: u64) {
        let now = Instant::now();
        self.connected_at = now;
        self.mark = (now, written);
        self.sampling = None; // a sample against the old connection is stale
    }

    /// Returns `(source, current rate, its rate)` when it is time to switch.
    fn poll(
        &mut self,
        artifact: &Artifact,
        others: &[usize],
        written: u64,
    ) -> Option<(usize, f64, f64)> {
        let now = Instant::now();
        if let Some(sample) = &self.sampling {
            let measured = match sample.result.try_recv() {
                Err(TryRecvError::Empty) => return None,
                Ok(Ok(rate)) => Some(rate),
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => None,
            };
            let (since, base) = sample.since;
            let source = sample.source;
            self.sampling = None;
            self.mark = (now, written);
            let current = rate(written - base, now - since);
            return measured
                .filter(|&other| other > current * self.tuning.switch_ratio)
                .map(|other| (source, current, other));
        }

        let (at, base) = self.mark;
        if now - self.connected_at < self.tuning.warmup || now - at < self.tuning.check_every {
            return None;
        }
        self.mark = (now, written);
        let current = rate(written - base, now - at);
        if current >= self.tuning.fast_enough {
            return None;
        }
        if artifact.size > 0 && current > 0.0 {
            let left = (artifact.size - written) as f64 / current;
            if left < self.tuning.min_remaining.as_secs_f64() {
                return None;
            }
        }
        let source = others.iter().copied().find(|i| {
            self.last_sampled
                .get(i)
                .is_none_or(|t| now - *t >= self.tuning.retest_after)
        })?;
        self.last_sampled.insert(source, now);

        let (sender, receiver) = mpsc::channel();
        let http = self.http.clone();
        let url = artifact.sources[source].url.clone();
        let (max, window) = (self.tuning.measure_bytes, self.tuning.measure_window);
        thread::spawn(move || {
            let _ = sender.send(http.measure(&url, written, max, window));
        });
        self.sampling = Some(Sample {
            source,
            since: (now, written),
            result: receiver,
        });
        None
    }
}

/// Print a line above the progress bar. A hidden bar (stderr is not a
/// terminal) drops `println`, so fall back to plain stderr there.
fn say(bar: &ProgressBar, line: String) {
    if bar.is_hidden() {
        eprintln!("{line}");
    } else {
        bar.println(line);
    }
}

fn rate(bytes: u64, elapsed: Duration) -> f64 {
    bytes as f64 / elapsed.as_secs_f64().max(1e-3)
}

fn human(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1e6 {
        format!("{:.1} MB", bytes_per_sec / 1e6)
    } else {
        format!("{:.0} KB", bytes_per_sec / 1e3)
    }
}

/// `bytes 100-199/200` -> 100
fn range_start(header: Option<&reqwest::header::HeaderValue>) -> Option<u64> {
    let value = header?.to_str().ok()?;
    let range = value.strip_prefix("bytes ")?;
    range.split('-').next()?.trim().parse().ok()
}

fn progress_bar(size: u64) -> Result<ProgressBar> {
    let bar = ProgressBar::new(size);
    bar.set_style(
        ProgressStyle::with_template(
            "{msg} [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec} ETA {eta}",
        )?
        .progress_chars("=> "),
    );
    Ok(bar)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::DownloadSource;
    use crate::remote::test_server::{Reply, Request, Server};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn artifact(server: &Server, data: &[u8]) -> Artifact {
        Artifact {
            file_name: "jdk.tar.gz".into(),
            sources: ["GitHub", "TUNA"]
                .into_iter()
                .map(|name| DownloadSource {
                    name,
                    url: format!("{}/{name}", server.url),
                    missing_note: None,
                })
                .collect(),
            size: data.len() as u64,
            sha256: to_hex(&Sha256::digest(data)),
        }
    }

    fn tuning() -> Tuning {
        Tuning {
            probe_bytes: 1024,
            probe_timeout: Duration::from_secs(2),
            retries: 2,
            retry_delay: Duration::from_millis(10),
            warmup: Duration::from_millis(150),
            check_every: Duration::from_millis(150),
            fast_enough: f64::INFINITY,
            min_remaining: Duration::ZERO,
            measure_window: Duration::from_millis(150),
            measure_bytes: u64::MAX,
            switch_ratio: 1.5,
            retest_after: Duration::from_secs(60),
        }
    }

    fn fetch(server: &Server, data: &[u8]) -> Result<Vec<u8>> {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("jdk");
        download_verified(&Http::new()?, &artifact(server, data), &dest, &tuning())?;
        Ok(std::fs::read(dest)?)
    }

    /// Delay a source's race read so the other one wins it.
    fn lose_race(req: &Request) {
        if req.is_probe() {
            thread::sleep(Duration::from_millis(300));
        }
    }

    #[test]
    fn race_winner_is_used_and_a_bad_mirror_is_redone_from_official() {
        let good = data(300_000);
        let mut evil = good.clone();
        evil[123_456] ^= 1;
        let server = Server::new(move |req| match req.path.as_str() {
            "/GitHub" => {
                lose_race(req);
                Reply::file(req, &good)
            }
            _ if req.is_probe() => Reply::file(req, &good),
            _ => Reply::file(req, &evil),
        });
        assert_eq!(fetch(&server, &data(300_000)).unwrap(), data(300_000));
        assert_eq!(
            server.downloads(),
            [("/TUNA".into(), 0), ("/GitHub".into(), 0)]
        );
    }

    #[test]
    fn a_dropped_connection_resumes_on_the_same_source() {
        let good = data(1_000_000);
        let calls = AtomicUsize::new(0);
        let server = Server::new(move |req| match req.path.as_str() {
            "/TUNA" => {
                lose_race(req);
                Reply::file(req, &good)
            }
            _ if req.is_probe() => Reply::file(req, &good),
            _ if calls.fetch_add(1, Ordering::SeqCst) == 0 => {
                Reply::file(req, &good).cut_after(300_000)
            }
            _ => Reply::file(req, &good),
        });
        assert_eq!(fetch(&server, &data(1_000_000)).unwrap(), data(1_000_000));
        assert_eq!(
            server.downloads(),
            [("/GitHub".into(), 0), ("/GitHub".into(), 300_000)]
        );
    }

    #[test]
    fn a_server_that_ignores_the_range_restarts_cleanly() {
        let good = data(1_000_000);
        let calls = AtomicUsize::new(0);
        let server = Server::new(move |req| match req.path.as_str() {
            "/TUNA" => {
                lose_race(req);
                Reply::file(req, &good)
            }
            _ if req.is_probe() => Reply::file(req, &good),
            _ => {
                let whole = Request {
                    range: None,
                    ..req.clone()
                };
                match calls.fetch_add(1, Ordering::SeqCst) {
                    0 => Reply::file(&whole, &good).cut_after(400_000),
                    _ => Reply::file(&whole, &good),
                }
            }
        });
        assert_eq!(fetch(&server, &data(1_000_000)).unwrap(), data(1_000_000));
    }

    #[test]
    fn a_slow_source_is_replaced_mid_download_without_starting_over() {
        let good = data(2_000_000);
        let server = Server::new(move |req| match req.path.as_str() {
            "/TUNA" => {
                lose_race(req);
                Reply::file(req, &good)
            }
            _ if req.is_probe() => Reply::file(req, &good),
            // ~800 KB/s: the whole file would take 2.5 s.
            _ => Reply::file(req, &good).slow(16_000, Duration::from_millis(20)),
        });
        let started = Instant::now();
        assert_eq!(fetch(&server, &data(2_000_000)).unwrap(), data(2_000_000));
        assert!(started.elapsed() < Duration::from_millis(2000));
        let downloads = server.downloads();
        assert_eq!(downloads[0], ("/GitHub".into(), 0));
        let (last, offset) = downloads.last().unwrap();
        assert_eq!(last, "/TUNA");
        assert!(*offset > 0, "{downloads:?}");
    }

    #[test]
    fn a_fast_source_is_kept() {
        let good = data(2_000_000);
        let server = Server::new(move |req| match req.path.as_str() {
            "/TUNA" => {
                lose_race(req);
                Reply::file(req, &good).slow(16_000, Duration::from_millis(20))
            }
            _ => Reply::file(req, &good).slow(64_000, Duration::from_millis(5)),
        });
        assert_eq!(fetch(&server, &data(2_000_000)).unwrap(), data(2_000_000));
        // TUNA may be sampled once, but never switched to (which would
        // take a second request).
        let downloads = server.downloads();
        let tuna = downloads.iter().filter(|(path, _)| path == "/TUNA").count();
        assert!(tuna <= 1, "{downloads:?}");
        assert_eq!(downloads[0], ("/GitHub".into(), 0));
    }

    #[test]
    fn a_mirror_without_the_build_is_never_downloaded_from() {
        let good = data(100_000);
        let server = Server::new(move |req| match req.path.as_str() {
            "/TUNA" => Reply::status(404),
            _ => {
                lose_race(req);
                Reply::file(req, &good)
            }
        });
        assert_eq!(fetch(&server, &data(100_000)).unwrap(), data(100_000));
        assert_eq!(server.downloads(), [("/GitHub".into(), 0)]);
    }

    #[test]
    fn gives_up_with_every_source_reason() {
        let server = Server::new(|req| match req.path.as_str() {
            "/GitHub" => Reply::status(500),
            _ => Reply::status(503),
        });
        let err = fetch(&server, &data(1000)).unwrap_err().to_string();
        assert!(err.contains("GitHub") && err.contains("500"), "{err}");
        assert!(err.contains("TUNA") && err.contains("503"), "{err}");
    }

    #[test]
    fn sha256_hex() {
        assert_eq!(
            to_hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parses_content_range() {
        let value = reqwest::header::HeaderValue::from_static("bytes 100-199/200");
        assert_eq!(range_start(Some(&value)), Some(100));
        assert_eq!(range_start(None), None);
    }
}
