use super::vision_processing::{Server, cfg as base_cfg, typed};
use super::*;
use markitai_core::{ConversionOutput, ConvertContext, LlmRuntime, convert_with_context};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn isolated(name: &str) -> bool {
    let exact = format!("request_coalescing::{name}");
    if std::env::var("MARKITAI_COALESCING_TEST").as_deref() == Ok(&exact) {
        return false;
    }
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let out = dir.path().join("stdout");
    let err = dir.path().join("stderr");
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", &exact, "--nocapture"])
        .env_clear()
        .env("MARKITAI_COALESCING_TEST", &exact)
        .env("MARKITAI_HOME", state)
        .current_dir(dir.path())
        .stdout(Stdio::from(std::fs::File::create(&out).unwrap()))
        .stderr(Stdio::from(std::fs::File::create(&err).unwrap()));
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
            cmd.env(name, value);
        }
    }
    let mut child = cmd.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("isolated test timed out: {exact}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = std::fs::read_to_string(out).unwrap();
    let stderr = std::fs::read_to_string(err).unwrap();
    assert!(status.success(), "{exact}: {status}\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("1 passed"),
        "test selector did not run: {stdout}"
    );
    true
}

#[derive(Default)]
struct Gate {
    state: Mutex<(usize, bool)>,
    changed: Condvar,
}
impl Gate {
    fn entered(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 += 1;
        self.changed.notify_all();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !state.1 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                drop(state);
                panic!("authored model gate timed out");
            }
            state = self.changed.wait_timeout(state, remaining).unwrap().0;
        }
    }
    fn wait_for(&self, count: usize) {
        let mut state = self.state.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.0 < count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.1 = true;
                self.changed.notify_all();
                drop(state);
                panic!("model requests did not arrive");
            }
            state = self.changed.wait_timeout(state, remaining).unwrap().0;
        }
    }
    fn release(&self) {
        let mut state = self.state.lock().unwrap();
        state.1 = true;
        self.changed.notify_all();
    }
}
fn input(root: &Path) -> PathBuf {
    let path = root.join("document.md");
    std::fs::write(
        &path,
        "# Complete source

Every caller must retain this original paragraph.

```rust
let exact = \"literal\";
```

Final text.
",
    )
    .unwrap();
    path
}
fn user(request: &Value) -> &str {
    let content = &request["messages"][1]["content"];
    content
        .as_str()
        .unwrap_or_else(|| content[0]["text"].as_str().unwrap())
}
fn config(server: &Server, root: &Path) -> Value {
    let mut cfg = base_cfg(server, root);
    cfg["cache"]["enabled"] = json!(true);
    cfg["llm"]["on_failure"] = json!("fail");
    // Make persistent reads/writes fail: one POST then several complete results
    // must come from active sharing rather than a sequential durable cache hit.
    let blocked = root.join("blocked-cache");
    std::fs::write(&blocked, b"owned fixture").unwrap();
    cfg["cache"]["global_dir"] = json!(blocked);
    cfg
}
fn concurrent(
    source: &Path,
    configs: Vec<Value>,
    runtimes: Vec<LlmRuntime>,
    gate: &Gate,
    distinct: usize,
) -> Vec<markitai_core::Result<ConversionOutput>> {
    let ready = Arc::new(Barrier::new(configs.len() + 1));
    thread::scope(|scope| {
        let workers: Vec<_> = configs
            .into_iter()
            .zip(runtimes)
            .map(|(cfg, runtime)| {
                let ready = ready.clone();
                scope.spawn(move || {
                    ready.wait();
                    convert_with_context(
                        source.to_str().unwrap(),
                        ConvertOptions {
                            config: Some(cfg),
                            ..Default::default()
                        },
                        ConvertContext {
                            llm_runtime: Some(&runtime),
                            ..Default::default()
                        },
                    )
                })
            })
            .collect();
        ready.wait();
        gate.wait_for(distinct);
        // Hold the first real POST while the other public conversions enter.
        // This is contention setup, never a latency/performance assertion. The
        // private coordinator tests separately prove already-attached wakeups.
        thread::sleep(Duration::from_millis(200));
        gate.release();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    })
}
fn server(gate: &Arc<Gate>) -> Server {
    let gate = gate.clone();
    Server::new(move |request, _| {
        gate.entered();
        (200, typed(user(request), "Complete shared document"))
    })
}
#[test]
fn shared_runtime_coalesces_real_posts_with_zero_waiter_usage_even_when_cache_writes_fail() {
    if isolated(
        "shared_runtime_coalesces_real_posts_with_zero_waiter_usage_even_when_cache_writes_fail",
    ) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = input(dir.path());
    let gate = Arc::new(Gate::default());
    let server = server(&gate);
    let cfg = config(&server, dir.path());
    let runtime = LlmRuntime::new(4).unwrap();
    let results = concurrent(&path, vec![cfg; 4], vec![runtime; 4], &gate, 1)
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(server.count(), 1);
    assert_eq!(results.iter().map(|v| v.usage.requests).sum::<u64>(), 1);
    assert_eq!(results.iter().map(|v| v.usage.input_tokens).sum::<u64>(), 7);
    assert_eq!(results.iter().filter(|v| v.llm_cache_hit()).count(), 3);
    for result in &results {
        assert_eq!(result.llm_markdown, results[0].llm_markdown);
        assert!(
            result
                .llm_markdown
                .as_ref()
                .unwrap()
                .contains("Final text.")
        );
    }
    assert!(results.iter().any(|v| {
        v.warnings
            .iter()
            .any(|warning| warning.contains("could not save"))
    }));
}
#[test]
fn disabled_refresh_pattern_bypass_and_independent_runtimes_each_send_new_requests() {
    if isolated("disabled_refresh_pattern_bypass_and_independent_runtimes_each_send_new_requests") {
        return;
    }
    for mode in 0..4 {
        let dir = tempfile::tempdir().unwrap();
        let path = input(dir.path());
        let gate = Arc::new(Gate::default());
        let server = server(&gate);
        let mut cfg = config(&server, dir.path());
        match mode {
            0 => cfg["cache"]["enabled"] = json!(false),
            1 => cfg["cache"]["no_cache"] = json!(true),
            2 => cfg["cache"]["no_cache_patterns"] = json!(["*.md"]),
            _ => (),
        }
        let first = LlmRuntime::new(2).unwrap();
        let second = if mode == 3 {
            LlmRuntime::new(2).unwrap()
        } else {
            first.clone()
        };
        let results = concurrent(&path, vec![cfg; 2], vec![first, second], &gate, 2);
        assert_eq!(server.count(), 2, "mode {mode}");
        for result in results {
            let result = result.unwrap();
            assert_eq!(result.usage.requests, 1);
            assert!(!result.llm_cache_hit());
        }
    }
}
#[test]
fn different_credentials_model_pool_and_budget_policy_never_join_active_requests() {
    if isolated("different_credentials_model_pool_and_budget_policy_never_join_active_requests") {
        return;
    }
    for mode in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let path = input(dir.path());
        let gate = Arc::new(Gate::default());
        let server = server(&gate);
        let cfg = config(&server, dir.path());
        let mut other = cfg.clone();
        match mode {
            0 => {
                other["llm"]["model_list"][0]["litellm_params"]["api_key"] =
                    json!("rotated-fixture-key")
            }
            1 => {
                other["llm"]["model_list"][0]["litellm_params"]["model"] =
                    json!("openai/other-fixture")
            }
            _ => other["llm"]["max_requests_per_document"] = json!(9),
        }
        let runtime = LlmRuntime::new(2).unwrap();
        let results = concurrent(&path, vec![cfg, other], vec![runtime; 2], &gate, 2);
        assert_eq!(server.count(), 2, "mode {mode}");
        assert!(
            results
                .into_iter()
                .all(|result| result.unwrap().usage.requests == 1)
        );
    }
}
#[test]
fn failed_owner_does_not_share_its_error_and_waiter_can_send_under_its_own_budget() {
    if isolated("failed_owner_does_not_share_its_error_and_waiter_can_send_under_its_own_budget") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = input(dir.path());
    let gate = Arc::new(Gate::default());
    let held = gate.clone();
    let server = Server::new(move |request, index| {
        held.entered();
        if index == 0 {
            (
                503,
                json!({"error":{"message":"fixture unavailable"},"usage":{"prompt_tokens":7,"completion_tokens":5}}),
            )
        } else {
            (200, typed(user(request), "Recovered complete document"))
        }
    });
    let mut cfg = config(&server, dir.path());
    cfg["llm"]["max_requests_per_document"] = json!(1);
    let runtime = LlmRuntime::new(2).unwrap();
    let results = concurrent(&path, vec![cfg; 2], vec![runtime; 2], &gate, 1);
    assert_eq!(server.count(), 2);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    let survivor = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(survivor.usage.requests, 1);
    assert!(!survivor.llm_cache_hit());
    assert!(survivor.llm_markdown.unwrap().contains("Final text."));
}
fn tiff(root: &Path) -> PathBuf {
    let path = root.join("pages.tiff");
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut encoder = tiff::encoder::TiffEncoder::new(&mut bytes).unwrap();
        for color in [30, 80] {
            let pixels = image::RgbImage::from_pixel(32, 32, image::Rgb([color, 100, 200]));
            encoder
                .write_image::<tiff::encoder::colortype::RGB8>(32, 32, pixels.as_raw())
                .unwrap();
        }
    }
    std::fs::write(&path, bytes.into_inner()).unwrap();
    path
}
#[test]
fn shared_first_visual_batch_keeps_all_frames_and_only_owner_pays() {
    if isolated("shared_first_visual_batch_keeps_all_frames_and_only_owner_pays") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = tiff(dir.path());
    let gate = Arc::new(Gate::default());
    let held = gate.clone();
    let server = Server::new(move |request, _| {
        held.entered();
        let content = request["messages"][1]["content"].as_array().unwrap();
        assert_eq!(
            content
                .iter()
                .filter(|part| part["type"] == "image_url")
                .count(),
            2
        );
        (
            200,
            typed(
                &format!("{}\n\nComplete transcription of both pages.", user(request)),
                "Two visual pages",
            ),
        )
    });
    let cfg = config(&server, dir.path());
    let runtime = LlmRuntime::new(2).unwrap();
    let results = concurrent(&path, vec![cfg; 2], vec![runtime; 2], &gate, 1)
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(server.count(), 1);
    assert_eq!(results.iter().map(|v| v.usage.requests).sum::<u64>(), 1);
    assert_eq!(results.iter().filter(|v| v.llm_cache_hit()).count(), 1);
    assert_eq!(results[0].llm_markdown, results[1].llm_markdown);
    let body = results[0].llm_markdown.as_ref().unwrap();
    assert_eq!(body.matches("<!-- Page number:").count(), 2);
    assert!(body.contains("Complete transcription of both pages."));
}
#[test]
fn complete_long_documents_share_each_chunk_but_preserve_order_and_full_source() {
    if isolated("complete_long_documents_share_each_chunk_but_preserve_order_and_full_source") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("long.md");
    let original = format!(
        "# Long source\n\n{}\nTAIL MUST SURVIVE\n",
        (0..1800)
            .map(|n| format!(
                "Paragraph {n:04} contains authored facts that must survive intact.\n\n"
            ))
            .collect::<String>()
    );
    std::fs::write(&path, &original).unwrap();
    let gate = Arc::new(Gate::default());
    let server = server(&gate);
    let cfg = config(&server, dir.path());
    let runtime = LlmRuntime::new(8).unwrap();
    let results = concurrent(&path, vec![cfg; 2], vec![runtime; 2], &gate, 1)
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let requests = server.requests.lock().unwrap();
    assert!(requests.len() > 1, "fixture must span multiple chunks");
    let mut chunks = std::collections::HashSet::new();
    for request in requests.iter() {
        assert!(chunks.insert(user(request)), "duplicate chunk POST");
    }
    assert_eq!(
        results.iter().map(|v| v.usage.requests).sum::<u64>(),
        requests.len() as u64
    );
    for result in results {
        let body = result.llm_markdown.unwrap();
        let mut position = 0;
        for n in 0..1800 {
            let needle = format!("Paragraph {n:04} contains");
            let at = body.find(&needle).unwrap();
            assert!(at >= position);
            position = at;
        }
        assert!(body.contains("TAIL MUST SURVIVE"));
    }
}
#[test]
fn endpoint_and_rendered_prompt_changes_send_independent_posts() {
    if isolated("endpoint_and_rendered_prompt_changes_send_independent_posts") {
        return;
    }
    for mode in 0..2 {
        let dir = tempfile::tempdir().unwrap();
        let path = input(dir.path());
        let gate = Arc::new(Gate::default());
        let first = server(&gate);
        let second = server(&gate);
        let cfg = config(&first, dir.path());
        let mut other = cfg.clone();
        if mode == 0 {
            other["llm"]["model_list"][0]["litellm_params"]["api_base"] = json!(second.base);
        } else {
            let prompt = dir.path().join("custom-user.md");
            std::fs::write(&prompt, "{content}\n\nKeep these complete source facts.").unwrap();
            other["prompts"]["document_process_user"] = json!(prompt);
        }
        let runtime = LlmRuntime::new(2).unwrap();
        let results = concurrent(&path, vec![cfg, other], vec![runtime; 2], &gate, 2);
        assert_eq!(first.count() + second.count(), 2);
        assert!(
            results
                .into_iter()
                .all(|result| result.unwrap().usage.requests == 1)
        );
    }
}
