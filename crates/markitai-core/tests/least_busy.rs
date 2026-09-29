use markitai_core::{
    ConversionOutput, ConvertContext, ConvertOptions, LlmRuntime, convert_with_context,
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

// Each public conversion gets a private configuration environment. HOME stays
// inherited, while no ambient provider credentials enter the child process.
fn isolated(name: &str) -> bool {
    if std::env::var("MARKITAI_LEAST_BUSY_TEST").as_deref() == Ok(name) {
        return false;
    }
    let dir = tempfile::tempdir().unwrap();
    let stdout = dir.path().join("stdout");
    let stderr = dir.path().join("stderr");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env_clear()
        .env("MARKITAI_LEAST_BUSY_TEST", name)
        .env("MARKITAI_HOME", dir.path().join("state"))
        .current_dir(dir.path())
        .stdout(Stdio::from(std::fs::File::create(&stdout).unwrap()))
        .stderr(Stdio::from(std::fs::File::create(&stderr).unwrap()));
    for name in [
        "HOME",
        "PATH",
        "USERPROFILE",
        "SYSTEMROOT",
        "WINDIR",
        "LANG",
        "LC_ALL",
        "TZ",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("isolated routing test timed out: {name}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = std::fs::read_to_string(stdout).unwrap();
    let stderr = std::fs::read_to_string(stderr).unwrap();
    assert!(status.success(), "{name}: {status}\n{stdout}\n{stderr}");
    assert!(stdout.contains("1 passed"), "{stdout}");
    true
}

#[derive(Clone)]
struct Request {
    headers: String,
    body: Value,
}
#[derive(Default)]
struct State {
    requests: Vec<Request>,
    released: bool,
}
type Handler = dyn Fn(&Request, usize) -> (u16, Value) + Send + Sync;
struct Server {
    base: String,
    state: Arc<(Mutex<State>, Condvar)>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
fn read(stream: &mut TcpStream) -> Request {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request_reader =
        bounded_fixture_io::Reader::new(stream, std::time::Instant::now() + Duration::from_secs(5));
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    let end = loop {
        let count = request_reader.read(&mut buffer).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() < 2_000_000);
        if let Some(index) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        })
        .unwrap();
    assert!(length < 2_000_000);
    while bytes.len() < end + length {
        let count = request_reader.read(&mut buffer).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    Request {
        headers,
        body: serde_json::from_slice(&bytes[end..end + length]).unwrap(),
    }
}
impl Server {
    fn new(
        held: bool,
        handler: impl Fn(&Request, usize) -> (u16, Value) + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let state = Arc::new((
            Mutex::new(State {
                released: !held,
                ..Default::default()
            }),
            Condvar::new(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let captured = state.clone();
        let handler: Arc<Handler> = Arc::new(handler);
        let worker = thread::spawn(move || {
            let mut workers = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("mock accept: {error}"),
                };
                let captured = captured.clone();
                let handler = handler.clone();
                workers.push(thread::spawn(move || {
                    stream.set_nonblocking(false).unwrap();
                    let request = read(&mut stream);
                    let (lock, changed) = &*captured;
                    let mut state = lock.lock().unwrap();
                    let index = state.requests.len();
                    state.requests.push(request.clone()); changed.notify_all();
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while !state.released {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() { break; }
                        state = changed.wait_timeout(state, remaining).unwrap().0;
                    }
                    let released = state.released; drop(state);
                    let (status, body) = if released { handler(&request, index) } else { (500, json!({"error":"authored gate timeout"})) };
                    let bytes = serde_json::to_vec(&body).unwrap();
                    let _ = write!(stream, "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
                    let _ = stream.write_all(&bytes);
                }));
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Self {
            base,
            state,
            stop,
            worker: Some(worker),
        }
    }
    fn wait(&self, count: usize) -> Vec<Request> {
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while state.requests.len() < count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                drop(state);
                self.release();
                panic!("expected {count} real HTTP requests");
            }
            state = changed.wait_timeout(state, remaining).unwrap().0;
        }
        state.requests.clone()
    }
    fn requests(&self) -> Vec<Request> {
        self.state.0.lock().unwrap().requests.clone()
    }
    fn release(&self) {
        let mut state = self
            .state
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.released = true;
        self.state.1.notify_all();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.release();
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn success(request: &Request, _: usize) -> (u16, Value) {
    let content = &request.body["messages"][1]["content"];
    let user = content
        .as_str()
        .unwrap_or_else(|| content[0]["text"].as_str().unwrap());
    let text = if request.body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("MARKITAI_DOCUMENT_JSON_V1")
    {
        json!({"cleaned_markdown":user,"frontmatter":{"description":"Complete routing fixture","tags":["routing"]}}).to_string()
    } else {
        user.to_owned()
    };
    (
        200,
        json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":5}}),
    )
}
fn deployment(base: &str, key: &str, id: &str, weight: u64) -> Value {
    json!({"model_name":"default","litellm_params":{"model":"openai/routing-fixture","api_base":base,"api_key":key,"weight":weight},"model_info":{"id":id}})
}
fn config(root: &Path, models: Vec<Value>) -> Value {
    json!({"cache":{"enabled":false,"global_dir":root.join("cache")},"prompts":{"dir":root.join("prompts")},"llm":{"enabled":true,"pure":true,"on_failure":"fail","model_list":models,"router_settings":{"routing_strategy":"least-busy","num_retries":0,"timeout":15}}})
}
fn source(root: &Path, name: &str) -> PathBuf {
    let path = root.join(format!("{name}.md"));
    std::fs::write(
        &path,
        format!("# {name}\n\nComplete source text for {name}.\n"),
    )
    .unwrap();
    path
}
fn call(
    source: &Path,
    cfg: Value,
    runtime: &LlmRuntime,
) -> markitai_core::Result<ConversionOutput> {
    convert_with_context(
        source.to_str().unwrap(),
        ConvertOptions {
            config: Some(cfg),
            ..Default::default()
        },
        ConvertContext {
            llm_runtime: Some(runtime),
            ..Default::default()
        },
    )
}

#[test]
fn shared_runtime_routes_two_real_inflight_requests_to_distinct_endpoints() {
    if isolated("shared_runtime_routes_two_real_inflight_requests_to_distinct_endpoints") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let first = Server::new(true, success);
    let second = Server::new(true, success);
    let cfg = config(
        dir.path(),
        vec![
            deployment(&first.base, "key-a", "a", 1_000_000),
            deployment(&second.base, "key-b", "b", 1),
        ],
    );
    let runtime = LlmRuntime::new(2).unwrap();
    let a = source(dir.path(), "alpha");
    let b = source(dir.path(), "beta");
    thread::scope(|scope| {
        let one = scope.spawn(|| call(&a, cfg.clone(), &runtime));
        first.wait(1);
        let two = scope.spawn(|| call(&b, cfg.clone(), &runtime));
        second.wait(1); // Both HTTP requests have arrived before either response.
        first.release();
        second.release();
        let one = one.join().unwrap().unwrap();
        let two = two.join().unwrap().unwrap();
        assert!(
            one.llm_markdown
                .unwrap()
                .contains("Complete source text for alpha.")
        );
        assert!(
            two.llm_markdown
                .unwrap()
                .contains("Complete source text for beta.")
        );
        assert_eq!(one.usage.requests + two.usage.requests, 2);
    });
    assert_eq!(first.requests().len(), 1);
    assert_eq!(second.requests().len(), 1);
    // Released deployments return to zero; stable ties start with the first.
    call(&a, cfg, &runtime).unwrap();
    assert_eq!(first.requests().len(), 2);
}

#[test]
fn resolved_credentials_explicit_ids_and_independent_runtimes_isolate_occupancy() {
    if isolated("resolved_credentials_explicit_ids_and_independent_runtimes_isolate_occupancy") {
        return;
    }
    for explicit in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::new(true, success);
        let unused = Server::new(false, success);
        let key_b = if explicit { "key-a" } else { "key-b" };
        let id_b = if explicit {
            "deployment-b"
        } else {
            "deployment-a"
        };
        let a = deployment(&server.base, "key-a", "deployment-a", 1);
        let b = deployment(&server.base, key_b, id_b, 1);
        let cfg_a = config(dir.path(), vec![a]);
        let cfg_b = config(
            dir.path(),
            vec![b, deployment(&unused.base, "key-c", "deployment-c", 1)],
        );
        let path = source(dir.path(), "identity");
        let runtime = LlmRuntime::new(3).unwrap();
        let independent = LlmRuntime::new(1).unwrap();
        thread::scope(|scope| {
            let one = scope.spawn(|| call(&path, cfg_a.clone(), &runtime));
            server.wait(1);
            let two = scope.spawn(|| call(&path, cfg_b.clone(), &runtime));
            server.wait(2);
            let three = scope.spawn(|| call(&path, cfg_a.clone(), &independent));
            let requests = server.wait(3);
            server.release();
            assert!(
                requests[0]
                    .headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer key-a")
            );
            assert!(
                requests[1]
                    .headers
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {key_b}"))
            );
            assert!(
                requests[2]
                    .headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer key-a")
            );
            one.join().unwrap().unwrap();
            two.join().unwrap().unwrap();
            three.join().unwrap().unwrap();
        });
        assert_eq!(server.requests().len(), 3);
        assert!(unused.requests().is_empty());
    }
}

#[test]
fn failures_release_and_fallback_groups_keep_their_existing_order() {
    if isolated("failures_release_and_fallback_groups_keep_their_existing_order") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let once = Server::new(false, |request, index| {
        if index == 0 {
            (503, json!({"error":{"message":"first attempt fails"}}))
        } else {
            success(request, index)
        }
    });
    let unused = Server::new(false, success);
    let cfg = config(
        dir.path(),
        vec![
            deployment(&once.base, "key-a", "a", 1),
            deployment(&unused.base, "key-b", "b", 1),
        ],
    );
    let runtime = LlmRuntime::new(1).unwrap();
    let path = source(dir.path(), "release");
    assert!(call(&path, cfg.clone(), &runtime).is_err());
    call(&path, cfg, &runtime).unwrap();
    assert_eq!(once.requests().len(), 2);
    assert!(unused.requests().is_empty());
    let failed = Server::new(false, |_, _| {
        (
            503,
            json!({"error":{"message":"authored transient failure"}}),
        )
    });
    let backup = Server::new(false, success);
    let first = deployment(&failed.base, "key-a", "a", 1);
    let mut second = deployment(&backup.base, "key-b", "b", 1);
    second["model_name"] = json!("backup");
    let mut cfg = config(dir.path(), vec![first, second]);
    cfg["llm"]["router_settings"]["fallbacks"] = json!([{"default":["backup"]}]);
    let runtime = LlmRuntime::new(1).unwrap();
    let path = source(dir.path(), "fallback");
    for _ in 0..2 {
        let output = call(&path, cfg.clone(), &runtime).unwrap();
        assert!(
            output
                .llm_markdown
                .unwrap()
                .contains("Complete source text for fallback.")
        );
        assert_eq!(output.usage.requests, 1);
    }
    assert_eq!(failed.requests().len(), 2);
    assert_eq!(backup.requests().len(), 2);
}

#[test]
fn typed_cache_hits_do_not_reserve_a_deployment_or_add_usage() {
    if isolated("typed_cache_hits_do_not_reserve_a_deployment_or_add_usage") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let server = Server::new(false, success);
    let mut cfg = config(dir.path(), vec![deployment(&server.base, "key-a", "a", 1)]);
    cfg["llm"]["pure"] = json!(false);
    cfg["cache"]["enabled"] = json!(true);
    let path = source(dir.path(), "cached");
    let runtime = LlmRuntime::new(2).unwrap();
    let first = call(&path, cfg.clone(), &runtime).unwrap();
    let second = call(&path, cfg, &runtime).unwrap();
    assert_eq!(first.llm_markdown, second.llm_markdown);
    assert_eq!(first.usage.requests, 1);
    assert_eq!(second.usage.requests, 0);
    assert!(second.llm_cache_hit());
    assert_eq!(server.requests().len(), 1);
}

#[cfg(test)]
mod bounded_fixture_io {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/support/bounded_read.rs"
    ));
}
