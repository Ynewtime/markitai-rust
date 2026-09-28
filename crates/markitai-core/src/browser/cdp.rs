use super::options::Options;
use crate::{Error, Result};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tungstenite::{Message, WebSocket};
use url::Url;

const MAX_MESSAGE: usize = 140 * 1024 * 1024;

fn failure(message: &str) -> Error {
    Error::Fetch(message.into())
}

fn private_profile() -> Result<TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("markitai-browser-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    Ok(builder.tempdir()?)
}

struct Process {
    child: Child,
    _profile: TempDir,
}
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // The child owns a new process group; Chromium renderers belong to it.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_millis(500);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Process {
    fn launch(executable: &Path, options: &Options) -> Result<(Self, u16, String)> {
        let profile = private_profile()?;
        let mut command = Command::new(executable);
        command
            .env_clear()
            .current_dir(profile.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for name in [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "TMPDIR",
            "TEMP",
            "TMP",
            "LANG",
            "LC_ALL",
            "TZ",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-extensions",
            "--disable-sync",
            "--disable-client-side-phishing-detection",
            "--metrics-recording-only",
            "--disable-features=MediaRouter,OptimizationHints,AutofillServerCommunication",
            "--password-store=basic",
            "--use-mock-keychain",
        ]);
        command.arg(format!("--user-data-dir={}", profile.path().display()));
        command.arg(format!(
            "--disk-cache-dir={}",
            profile.path().join("cache").display()
        ));
        if let Some(proxy) = &options.proxy {
            command.arg(format!("--proxy-server={proxy}"));
            command.arg(format!("--proxy-bypass-list={}", options.bypass));
        } else {
            command.arg("--no-proxy-server");
        }
        command.arg("about:blank");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn().map_err(|_| failure("Cannot launch Chromium; check MARKITAI_BROWSER_EXECUTABLE or install Chrome/Chromium"))?;
        let mut process = Self {
            child,
            _profile: profile,
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if process.child.try_wait()?.is_some() {
                return Err(failure(
                    "Chromium exited before its debugging endpoint became ready",
                ));
            }
            let path = process._profile.path().join("DevToolsActivePort");
            if let Ok(bytes) = std::fs::read(&path)
                && bytes.len() < 4096
                && let Ok(text) = std::str::from_utf8(&bytes)
            {
                let mut lines = text.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next())
                    && let Ok(port) = port.parse::<u16>()
                    && port > 0
                    && path.starts_with("/devtools/browser/")
                    && !path.contains(['\r', '\n', '?', '#'])
                {
                    return Ok((process, port, path.to_owned()));
                }
            }
            if Instant::now() >= deadline {
                return Err(failure("Chromium startup timed out"));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

pub(super) struct Browser {
    socket: WebSocket<TcpStream>,
    _process: Option<Process>,
    session: String,
    next_id: u64,
    pub deadline: Instant,
    blocked: Vec<regex::Regex>,
    in_flight: HashSet<String>,
    pub last_network_change: Instant,
    pub main_frame: Option<String>,
    pub status: Option<u64>,
    pub navigation_failed: bool,
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self
            .socket
            .get_mut()
            .set_write_timeout(Some(Duration::from_millis(100)));
        let _ = self.socket.send(Message::Text(
            json!({"id":u64::MAX,"method":"Browser.close"})
                .to_string()
                .into(),
        ));
    }
}
impl Browser {
    pub fn launch(executable: &Path, options: &Options) -> Result<Self> {
        let (process, port, path) = Process::launch(executable, options)?;
        let stream = TcpStream::connect_timeout(
            &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            Duration::from_secs(3),
        )
        .map_err(|_| failure("Cannot connect to Chromium debugging endpoint"))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (socket, _) = tungstenite::client::client_with_config(
            format!("ws://127.0.0.1:{port}{path}"),
            stream,
            Some(config),
        )
        .map_err(|_| failure("Cannot establish Chromium protocol connection"))?;
        let mut browser = Self {
            socket,
            _process: Some(process),
            session: String::new(),
            next_id: 1,
            deadline: Instant::now() + Duration::from_millis(options.timeout),
            blocked: options.blocked.clone(),
            in_flight: HashSet::new(),
            last_network_change: Instant::now(),
            main_frame: None,
            status: None,
            navigation_failed: false,
        };
        let target = browser.call("Target.createTarget", json!({"url":"about:blank"}))?;
        let target_id = target
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or_else(|| failure("Chromium did not create a page"))?;
        let attached = browser.call(
            "Target.attachToTarget",
            json!({"targetId":target_id,"flatten":true}),
        )?;
        browser.session = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| failure("Chromium did not attach a page"))?
            .to_owned();
        for method in ["Page.enable", "Runtime.enable", "Network.enable"] {
            browser.call(method, json!({}))?;
        }
        browser.call("Network.setCacheDisabled", json!({"cacheDisabled":true}))?;
        browser.call("Network.setBypassServiceWorker", json!({"bypass":true}))?;
        browser.call("Browser.setDownloadBehavior", json!({"behavior":"deny"}))?;
        browser.call(
            "Fetch.enable",
            json!({"patterns":[{"urlPattern":"*","requestStage":"Request"}]}),
        )?;
        browser.call("Emulation.setDeviceMetricsOverride", json!({"width":options.width,"height":options.height,"deviceScaleFactor":1,"mobile":false}))?;
        if !options.headers.is_empty() {
            browser.call(
                "Network.setExtraHTTPHeaders",
                json!({"headers":options.headers}),
            )?;
        }
        if let Some(agent) = &options.user_agent {
            browser.call("Network.setUserAgentOverride", json!({"userAgent":agent}))?;
        }
        if !options.cookies.is_empty() {
            browser.call("Network.setCookies", json!({"cookies":options.cookies}))?;
        }
        Ok(browser)
    }

    fn send(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({"id":id,"method":method,"params":params});
        if !self.session.is_empty()
            && !method.starts_with("Browser.")
            && !method.starts_with("Target.")
        {
            message["sessionId"] = json!(self.session);
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .map_err(|_| failure("Chromium protocol write failed"))?;
        Ok(id)
    }
    fn event(&mut self, message: &Value) -> Result<()> {
        if message.get("sessionId").and_then(Value::as_str) != Some(self.session.as_str()) {
            return Ok(());
        }
        let params = &message["params"];
        match message.get("method").and_then(Value::as_str).unwrap_or("") {
            "Fetch.requestPaused" => {
                let id = params["requestId"]
                    .as_str()
                    .ok_or_else(|| failure("Malformed Chromium request event"))?;
                let url = params
                    .pointer("/request/url")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let safe = Url::parse(url).is_ok_and(|url| {
                    ["http", "https", "data", "blob", "about"].contains(&url.scheme())
                });
                if !safe || self.blocked.iter().any(|pattern| pattern.is_match(url)) {
                    self.send(
                        "Fetch.failRequest",
                        json!({"requestId":id,"errorReason":"BlockedByClient"}),
                    )?;
                } else {
                    self.send("Fetch.continueRequest", json!({"requestId":id}))?;
                }
            }
            "Network.requestWillBeSent" => {
                if let Some(id) = params["requestId"].as_str() {
                    self.in_flight.insert(id.to_owned());
                    self.last_network_change = Instant::now();
                }
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                if let Some(id) = params["requestId"].as_str() {
                    self.in_flight.remove(id);
                }
                self.last_network_change = Instant::now();
            }
            "Network.responseReceived" if params["type"].as_str() == Some("Document") => {
                if self
                    .main_frame
                    .as_deref()
                    .is_none_or(|frame| params["frameId"].as_str() == Some(frame))
                {
                    self.status = params.pointer("/response/status").and_then(Value::as_u64);
                }
            }
            "Page.javascriptDialogOpening" => {
                self.send("Page.handleJavaScriptDialog", json!({"accept":false}))?;
            }
            "Inspector.targetCrashed" => self.navigation_failed = true,
            _ => {}
        }
        Ok(())
    }
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| failure("Browser operation timed out"))?;
        self.socket.get_mut().set_write_timeout(Some(remaining))?;
        let id = self.send(method, params)?;
        loop {
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| failure("Browser operation timed out"))?;
            self.socket.get_mut().set_read_timeout(Some(remaining))?;
            let message = self
                .socket
                .read()
                .map_err(|_| failure("Browser operation timed out or connection closed"))?;
            match message {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text)
                        .map_err(|_| failure("Invalid Chromium protocol response"))?;
                    if value["id"].as_u64() == Some(id) {
                        if value.get("error").is_some() {
                            return Err(failure(&format!("Chromium protocol rejected {method}")));
                        }
                        return Ok(value.get("result").cloned().unwrap_or_else(|| json!({})));
                    }
                    self.event(&value)?;
                }
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Close(_) => return Err(failure("Chromium closed its page")),
                _ => return Err(failure("Unexpected Chromium protocol frame")),
            }
        }
    }
    pub fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let timeout = self
            .deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1);
        let result = self.call("Runtime.evaluate", json!({"expression":expression,"returnByValue":true,"awaitPromise":true,"timeout":timeout.min(u64::MAX as u128) as u64}))?;
        if result.get("exceptionDetails").is_some() {
            return Err(failure("Browser page evaluation failed"));
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }
    pub fn idle(&self) -> bool {
        self.in_flight.is_empty()
            && self.last_network_change.elapsed() >= Duration::from_millis(500)
    }
    pub fn pause(&mut self, duration: Duration) -> Result<()> {
        let millis = duration.as_millis().min(u64::MAX as u128) as u64;
        self.evaluate(&format!(
            "new Promise(resolve => setTimeout(() => resolve(true), {millis}))"
        ))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn mock(
        limit: usize,
        handler: impl FnOnce(WebSocket<TcpStream>) + Send + 'static,
    ) -> (Browser, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            handler(tungstenite::accept(stream).unwrap());
        });
        let stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(limit))
            .max_frame_size(Some(limit));
        let (socket, _) = tungstenite::client::client_with_config(
            format!("ws://{address}/mock"),
            stream,
            Some(config),
        )
        .unwrap();
        (
            Browser {
                socket,
                _process: None,
                session: "test-session".into(),
                next_id: 1,
                deadline: Instant::now() + Duration::from_secs(3),
                blocked: vec![regex::Regex::new(r"^https://example\.test/ads/").unwrap()],
                in_flight: HashSet::new(),
                last_network_change: Instant::now(),
                main_frame: Some("main".into()),
                status: None,
                navigation_failed: false,
            },
            worker,
        )
    }
    fn receive(socket: &mut WebSocket<TcpStream>) -> Value {
        let message = socket.read().unwrap().into_text().unwrap();
        serde_json::from_str(&message).unwrap()
    }
    fn send(socket: &mut WebSocket<TcpStream>, value: Value) {
        socket
            .send(Message::Text(value.to_string().into()))
            .unwrap();
    }

    #[test]
    fn paused_requests_are_filtered_while_waiting_for_the_matching_reply() {
        let (mut browser, worker) = mock(4096, |mut socket| {
            for (url, expected) in [
                ("https://example.test/content", "Fetch.continueRequest"),
                ("https://example.test/ads/banner", "Fetch.failRequest"),
                ("file:///private/secret", "Fetch.failRequest"),
            ] {
                let command = receive(&mut socket);
                send(
                    &mut socket,
                    json!({"sessionId":"test-session","method":"Fetch.requestPaused","params":{"requestId":"request-1","request":{"url":url}}}),
                );
                let action = receive(&mut socket);
                assert_eq!(action["method"], expected);
                assert_eq!(action["params"]["requestId"], "request-1");
                send(&mut socket, json!({"id":action["id"],"result":{}}));
                send(
                    &mut socket,
                    json!({"id":command["id"],"result":{"complete":true}}),
                );
            }
        });
        for _ in 0..3 {
            assert_eq!(
                browser.call("Runtime.evaluate", json!({})).unwrap(),
                json!({"complete":true})
            );
        }
        worker.join().unwrap();
    }

    #[test]
    fn unrelated_sessions_and_iframe_status_do_not_change_main_navigation() {
        let (mut browser, worker) = mock(4096, |mut socket| {
            let command = receive(&mut socket);
            for (session, frame, status) in [
                ("other", "main", 503),
                ("test-session", "iframe", 404),
                ("test-session", "main", 201),
            ] {
                send(
                    &mut socket,
                    json!({"sessionId":session,"method":"Network.responseReceived","params":{"type":"Document","frameId":frame,"response":{"status":status}}}),
                );
            }
            send(
                &mut socket,
                json!({"sessionId":"test-session","method":"Network.requestWillBeSent","params":{"requestId":"pending"}}),
            );
            send(&mut socket, json!({"id":command["id"],"result":{}}));
        });
        browser.call("Page.getFrameTree", json!({})).unwrap();
        assert_eq!(browser.status, Some(201));
        assert!(!browser.idle());
        worker.join().unwrap();
    }

    #[test]
    fn protocol_errors_do_not_echo_page_or_credential_payloads() {
        let (mut browser, worker) = mock(4096, |mut socket| {
            let command = receive(&mut socket);
            send(
                &mut socket,
                json!({"id":command["id"],"error":{"message":"private-token and cookie-secret","code":-1}}),
            );
        });
        let error = browser
            .call("Network.setCookies", json!({}))
            .unwrap_err()
            .to_string();
        assert_eq!(error, "Chromium protocol rejected Network.setCookies");
        worker.join().unwrap();
    }

    #[test]
    fn oversized_protocol_messages_fail_instead_of_returning_partial_content() {
        let (mut browser, worker) = mock(256, |mut socket| {
            let command = receive(&mut socket);
            send(
                &mut socket,
                json!({"id":command["id"],"result":{"html":"x".repeat(1024)}}),
            );
        });
        assert!(browser.call("Runtime.evaluate", json!({})).is_err());
        worker.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn browser_profile_is_private_and_removed_when_ownership_ends() {
        use std::os::unix::fs::PermissionsExt;
        let profile = private_profile().unwrap();
        let path = profile.path().to_owned();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::write(path.join("cookie-state"), b"isolated test state").unwrap();
        drop(profile);
        assert!(!path.exists());
    }
}
