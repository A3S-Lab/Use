use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub(crate) struct TestServer {
    base_url: String,
    routes: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    requests: Arc<Mutex<Vec<String>>>,
    range_requests: Arc<Mutex<Vec<(String, String)>>>,
    interruptions: Arc<Mutex<HashMap<String, InterruptedResponse>>>,
    content_range_overrides: Arc<Mutex<HashMap<String, String>>>,
    ignored_range_paths: Arc<Mutex<HashSet<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy)]
struct InterruptedResponse {
    remaining_requests: usize,
    bytes_per_request: usize,
}

impl TestServer {
    pub(crate) fn start(routes: HashMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base_url = format!("http://{}/", listener.local_addr().unwrap());
        let routes = Arc::new(Mutex::new(routes));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let range_requests = Arc::new(Mutex::new(Vec::new()));
        let interruptions = Arc::new(Mutex::new(HashMap::new()));
        let content_range_overrides = Arc::new(Mutex::new(HashMap::new()));
        let ignored_range_paths = Arc::new(Mutex::new(HashSet::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_routes = Arc::clone(&routes);
        let thread_requests = Arc::clone(&requests);
        let thread_range_requests = Arc::clone(&range_requests);
        let thread_interruptions = Arc::clone(&interruptions);
        let thread_content_range_overrides = Arc::clone(&content_range_overrides);
        let thread_ignored_range_paths = Arc::clone(&ignored_range_paths);
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let routes = Arc::clone(&thread_routes);
                        let requests = Arc::clone(&thread_requests);
                        let range_requests = Arc::clone(&thread_range_requests);
                        let interruptions = Arc::clone(&thread_interruptions);
                        let content_range_overrides = Arc::clone(&thread_content_range_overrides);
                        let ignored_range_paths = Arc::clone(&thread_ignored_range_paths);
                        std::thread::spawn(move || {
                            serve(
                                stream,
                                &routes,
                                &requests,
                                &range_requests,
                                &interruptions,
                                &content_range_overrides,
                                &ignored_range_paths,
                            )
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url,
            routes,
            requests,
            range_requests,
            interruptions,
            content_range_overrides,
            ignored_range_paths,
            stop,
            thread: Some(thread),
        }
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    pub(crate) fn clear_requests(&self) {
        self.requests.lock().unwrap().clear();
        self.range_requests.lock().unwrap().clear();
    }

    pub(crate) fn replace_routes(&self, routes: HashMap<String, Vec<u8>>) {
        *self.routes.lock().unwrap() = routes;
    }

    pub(crate) fn interrupt_requests(
        &self,
        path: impl Into<String>,
        remaining_requests: usize,
        bytes_per_request: usize,
    ) {
        assert!(remaining_requests > 0);
        assert!(bytes_per_request > 0);
        self.interruptions.lock().unwrap().insert(
            path.into(),
            InterruptedResponse {
                remaining_requests,
                bytes_per_request,
            },
        );
    }

    pub(crate) fn allow_complete_requests(&self, path: &str) {
        self.interruptions.lock().unwrap().remove(path);
    }

    pub(crate) fn ranges_for(&self, path: &str) -> Vec<String> {
        self.range_requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(request_path, _)| request_path == path)
            .map(|(_, range)| range.clone())
            .collect()
    }

    pub(crate) fn override_content_range(&self, path: impl Into<String>, value: impl Into<String>) {
        self.content_range_overrides
            .lock()
            .unwrap()
            .insert(path.into(), value.into());
    }

    pub(crate) fn ignore_ranges_for(&self, path: impl Into<String>) {
        self.ignored_range_paths.lock().unwrap().insert(path.into());
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    routes: &Mutex<HashMap<String, Vec<u8>>>,
    requests: &Mutex<Vec<String>>,
    range_requests: &Mutex<Vec<(String, String)>>,
    interruptions: &Mutex<HashMap<String, InterruptedResponse>>,
    content_range_overrides: &Mutex<HashMap<String, String>>,
    ignored_range_paths: &Mutex<HashSet<String>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buffer = [0_u8; 8192];
    let Ok(size) = stream.read(&mut buffer) else {
        return;
    };
    let request = String::from_utf8_lossy(&buffer[..size]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    requests.lock().unwrap().push(path.clone());
    let range = request.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("range")
            .then(|| value.trim().to_owned())
    });
    if let Some(range) = &range {
        range_requests
            .lock()
            .unwrap()
            .push((path.clone(), range.clone()));
    }
    let body = routes.lock().unwrap().get(&path).cloned();
    let effective_range = if ignored_range_paths.lock().unwrap().contains(&path) {
        None
    } else {
        range.as_deref()
    };
    let (status, body, content_range) = match body {
        Some(body) => match effective_range.and_then(range_start) {
            Some(start) if start < body.len() => (
                "206 Partial Content",
                body[start..].to_vec(),
                Some(format!("bytes {start}-{}/{}", body.len() - 1, body.len())),
            ),
            Some(_) => (
                "416 Range Not Satisfiable",
                Vec::new(),
                Some(format!("bytes */{}", body.len())),
            ),
            None => ("200 OK", body, None),
        },
        None => ("404 Not Found", b"not found".to_vec(), None),
    };
    let content_range = content_range
        .map(|value| {
            content_range_overrides
                .lock()
                .unwrap()
                .get(&path)
                .cloned()
                .unwrap_or(value)
        })
        .map(|value| format!("Content-Range: {value}\r\n"))
        .unwrap_or_default();
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n{content_range}Connection: close\r\n\r\n",
        body.len()
    );
    let interrupted_bytes = {
        let mut interruptions = interruptions.lock().unwrap();
        interruptions.get_mut(&path).and_then(|interruption| {
            if interruption.remaining_requests == 0 {
                return None;
            }
            interruption.remaining_requests -= 1;
            Some(interruption.bytes_per_request.min(body.len()))
        })
    };
    let response = interrupted_bytes.map_or(body.as_slice(), |length| &body[..length]);
    if stream.write_all(header.as_bytes()).is_ok() && stream.write_all(response).is_ok() {
        let _ = stream.flush();
        let _ = stream.shutdown(Shutdown::Write);
    }
}

fn range_start(value: &str) -> Option<usize> {
    value
        .strip_prefix("bytes=")?
        .strip_suffix('-')?
        .parse()
        .ok()
}
