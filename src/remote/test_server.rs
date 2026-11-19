//! A tiny HTTP/1.1 server for download tests: range requests, slow
//! bodies and connections that drop mid-way.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub path: String,
    /// `Range: bytes=<start>-[<end>]`
    pub range: Option<(u64, Option<u64>)>,
}

impl Request {
    /// The initial race reads a closed range from the start.
    pub fn is_probe(&self) -> bool {
        matches!(self.range, Some((0, Some(_))))
    }

    pub fn offset(&self) -> u64 {
        self.range.map_or(0, |(start, _)| start)
    }
}

pub struct Reply {
    status: u16,
    headers: Vec<String>,
    body: Vec<u8>,
    /// Advertised Content-Length; more than `body.len()` simulates a drop.
    declared: usize,
    chunk: usize,
    delay: Duration,
}

impl Reply {
    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            declared: 0,
            chunk: usize::MAX,
            delay: Duration::ZERO,
        }
    }

    /// Serve `data` like a static file server, honoring `Range`.
    pub fn file(req: &Request, data: &[u8]) -> Self {
        let len = data.len() as u64;
        let (status, headers, body) = match req.range {
            None => (200, Vec::new(), data),
            Some((start, end)) => {
                let end = end.unwrap_or(len - 1).min(len - 1);
                let header = format!("Content-Range: bytes {start}-{end}/{len}");
                (206, vec![header], &data[start as usize..=end as usize])
            }
        };
        Self {
            status,
            headers,
            body: body.to_vec(),
            declared: body.len(),
            ..Self::status(0)
        }
    }

    /// Send the body in `chunk`-byte pieces with a pause after each.
    pub fn slow(mut self, chunk: usize, delay: Duration) -> Self {
        self.chunk = chunk;
        self.delay = delay;
        self
    }

    /// Close the connection after `n` body bytes, mid-transfer.
    pub fn cut_after(mut self, n: usize) -> Self {
        self.body.truncate(n);
        self
    }
}

pub struct Server {
    pub url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    listener: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn new(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", socket.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let handler = Arc::new(handler);
        let listener = thread::spawn(move || {
            for stream in socket.incoming() {
                let stream = stream.unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    break; // the Drop impl's wake-up connection
                }
                let path = first.split_whitespace().nth(1).unwrap().to_owned();
                let (handler, seen) = (handler.clone(), seen.clone());
                thread::spawn(move || {
                    let range = (&mut reader)
                        .lines()
                        .map_while(Result::ok)
                        .take_while(|line| !line.is_empty())
                        .find_map(|line| parse_range(&line));
                    let req = Request { path, range };
                    seen.lock().unwrap().push(req.clone());
                    let reply = handler(&req);
                    let mut stream = reader.into_inner();
                    let mut head = format!(
                        "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
                        reply.status, reply.declared
                    );
                    for header in &reply.headers {
                        head.push_str(header);
                        head.push_str("\r\n");
                    }
                    head.push_str("\r\n");
                    if stream.write_all(head.as_bytes()).is_err() {
                        return;
                    }
                    for piece in reply.body.chunks(reply.chunk.max(1)) {
                        if stream.write_all(piece).is_err() {
                            return; // the client hung up (e.g. it switched source)
                        }
                        thread::sleep(reply.delay);
                    }
                });
            }
        });
        Self {
            url,
            requests,
            listener: Some(listener),
        }
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// Non-probe requests as `(path, offset)`.
    pub fn downloads(&self) -> Vec<(String, u64)> {
        self.requests()
            .into_iter()
            .filter(|r| !r.is_probe())
            .map(|r| (r.path.clone(), r.offset()))
            .collect()
    }
}

fn parse_range(line: &str) -> Option<(u64, Option<u64>)> {
    let (name, value) = line.split_once(':')?;
    if !name.eq_ignore_ascii_case("range") {
        return None;
    }
    let (start, end) = value.trim().strip_prefix("bytes=")?.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()))
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(TcpStream::connect(self.url.strip_prefix("http://").unwrap()).unwrap());
        self.listener.take().unwrap().join().unwrap();
    }
}
