use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) struct Directory(PathBuf);
impl Directory {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "cli-tools-scenarios-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) line: String,
    pub(crate) headers: String,
    pub(crate) body: Value,
}

pub(crate) struct Server {
    pub(crate) url: String,
    thread: thread::JoinHandle<Vec<Request>>,
}

impl Server {
    pub(crate) fn with_identity(mut responses: Vec<(u16, Value)>) -> Self {
        responses.insert(0, (200, auth_identity()));
        Self::new(responses)
    }

    pub(crate) fn new(responses: Vec<(u16, Value)>) -> Self {
        Self::with_account(responses, Some("281"))
    }

    pub(crate) fn with_account(responses: Vec<(u16, Value)>, account: Option<&str>) -> Self {
        let account_header = account
            .map(|account| format!("Gecko-Account: {account}\r\n"))
            .unwrap_or_default();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let thread = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, response) in responses {
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => panic!("mock server timed out or failed: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0; 8192];
                let (headers, boundary, length) = loop {
                    let count = stream.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (headers, end + 4, length);
                    }
                };
                while bytes.len() < boundary + length {
                    let count = stream.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                let body = if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap()
                };
                requests.push(Request {
                    line: headers.lines().next().unwrap().into(),
                    headers,
                    body,
                });
                let response = response.to_string();
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{account_header}Content-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            }
            requests
        });
        Self { url, thread }
    }
    pub(crate) fn finish(self) -> Vec<Request> {
        self.thread.join().unwrap()
    }
}

pub(crate) fn app_tokens(profile: &str) -> crate::auth::TokenSet {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let claims = serde_json::json!({"profile": profile, "account": "test-account-uuid"});
    crate::auth::TokenSet {
        access_token: format!(
            "e30.{}.test-signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        ),
        id_token: "test-id".into(),
        refresh_token: "test-refresh".into(),
        expires_in: None,
        token_type: None,
    }
}

pub(crate) fn auth_identity() -> Value {
    serde_json::json!({"token": {
        "account": {"uuid": "test-account-uuid", "routing_id": 281},
        "user": {"auth_id": "app-user-1", "id": 2260}
    }})
}
