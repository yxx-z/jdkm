use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct Server {
    pub url: String,
    requests: Arc<Mutex<Vec<(String, bool)>>>,
    listener: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn new(handler: impl Fn(&str, bool) -> (u16, Vec<u8>) + Send + Sync + 'static) -> Self {
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
                if reader.read_line(&mut first).unwrap() == 0 {
                    break;
                }
                let path = first.split_whitespace().nth(1).unwrap().to_owned();
                let handler = handler.clone();
                let seen = seen.clone();
                thread::spawn(move || {
                    let probe = (&mut reader)
                        .lines()
                        .map(Result::unwrap)
                        .take_while(|line| !line.is_empty())
                        .any(|line| line.to_ascii_lowercase().starts_with("range: bytes=0-"));
                    seen.lock().unwrap().push((path.clone(), probe));
                    let (status, body) = handler(&path, probe);
                    let mut stream = reader.into_inner();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(&body);
                });
            }
        });
        Self {
            url,
            requests,
            listener: Some(listener),
        }
    }

    pub fn downloads(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, probe)| !probe)
            .map(|(path, _)| path.clone())
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(TcpStream::connect(self.url.strip_prefix("http://").unwrap()).unwrap());
        self.listener.take().unwrap().join().unwrap();
    }
}
