use super::{
    State, store,
    types::{ApiError, ApiResult, Item, now},
};
use crate::diagnostics::{AttemptDiagnostics, Operation};
use futures_util::{StreamExt, stream::FuturesUnordered};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Instant,
};
use tokio::sync::broadcast;

#[derive(Clone)]
pub(super) struct JobData {
    pub id: String,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub status: String,
    pub persistence_error: Option<String>,
    pub options: Value,
    pub items: Vec<Item>,
    pub size: u64,
    pub bases: HashMap<String, String>,
    pub assets: HashMap<String, Vec<String>>,
    pub item_options: HashMap<String, Value>,
    pub transactions: Vec<String>,
}
impl JobData {
    pub fn progress(&self) -> Value {
        json!({"status":self.status,"done":self.items.iter().filter(|i|i.status=="done").count(),
            "failed":self.items.iter().filter(|i|i.status=="error").count(),"total":self.items.len()})
    }
    pub fn snapshot(&self) -> Value {
        let mut value = self.progress();
        value["job_id"] = json!(self.id);
        value["created_at"] = json!(self.created_at);
        value["finished_at"] = json!(self.finished_at);
        value["options"] = self.options.clone();
        value["items"] = json!(self.items);
        if let Some(error) = &self.persistence_error {
            value["persistence_error"] = json!(error);
        }
        value
    }
    pub fn history(&self) -> Value {
        let max_duration = self.items.iter().filter_map(|i| i.duration_ms).max();
        let duration = if self.items.iter().any(|i| i.operation != "convert") {
            max_duration
        } else {
            chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .ok()
                .zip(
                    self.finished_at
                        .as_deref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()),
                )
                .map(|(start, end)| (end - start).num_milliseconds().max(0) as u64)
                .or(max_duration)
        };
        let cost = self
            .items
            .iter()
            .any(|i| i.cost_usd.is_some())
            .then(|| self.items.iter().filter_map(|i| i.cost_usd).sum::<f64>());
        let mut value = self.progress();
        value["job_id"] = json!(self.id);
        value["created_at"] = json!(self.created_at);
        value["finished_at"] = json!(self.finished_at);
        value["skipped"] = json!(self.items.iter().filter(|i| i.skipped).count());
        value["llm_enhanced"] = json!(self.items.iter().filter(|i| i.llm_enhanced).count());
        value["cost_usd"] = json!(cost);
        value["names_preview"] = json!(
            self.items
                .iter()
                .take(3)
                .map(|i| &i.name)
                .collect::<Vec<_>>()
        );
        value["kinds_preview"] = json!(
            self.items
                .iter()
                .take(3)
                .map(|i| &i.kind)
                .collect::<Vec<_>>()
        );
        value["duration_ms"] = json!(duration);
        value["size_bytes"] = json!(self.size);
        value["origin"] = json!(self.options["origin"].as_str().unwrap_or("web"));
        value["retryable"] = json!(self.items.iter().any(|i| i.retryable));
        value
    }
}

pub(super) struct Job {
    pub folder: PathBuf,
    pub data: Mutex<JobData>,
    pub access: Mutex<()>,
    pub active: AtomicUsize,
    pub sequence: AtomicU64,
    pub retry_queue: Mutex<VecDeque<super::rerun::Work>>,
    pub retry_draining: AtomicBool,
    pub retry_pending: Mutex<HashSet<String>>,
    pub runtime: OnceLock<Arc<markitai_core::LlmRuntime>>,
    pub events: broadcast::Sender<(&'static str, Value)>,
}
impl Job {
    pub fn new(folder: PathBuf, data: JobData) -> Self {
        let (events, _) = broadcast::channel(128);
        Self {
            folder,
            data: Mutex::new(data),
            access: Mutex::new(()),
            active: AtomicUsize::new(0),
            sequence: AtomicU64::new(0),
            retry_queue: Mutex::new(VecDeque::new()),
            retry_draining: AtomicBool::new(false),
            retry_pending: Mutex::new(HashSet::new()),
            runtime: OnceLock::new(),
            events,
        }
    }
}

pub(super) fn sanitize_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or("upload");
    let mut clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    clean = clean.trim_matches([' ', '.']).to_owned();
    if clean.is_empty() {
        clean = "upload".into();
    }
    let reserved = clean.split('.').next().unwrap_or("").to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&reserved.as_str())
        || (reserved.len() == 4
            && (reserved.starts_with("COM") || reserved.starts_with("LPT"))
            && matches!(reserved.as_bytes()[3], b'1'..=b'9'))
    {
        clean.insert(0, '_');
    }
    if clean.len() > 180 {
        let extension = std::path::Path::new(&clean)
            .extension()
            .and_then(|v| v.to_str())
            .map(|s| format!(".{s}"))
            .filter(|s| s.len() < 90)
            .unwrap_or_default();
        let mut end = 180 - extension.len();
        while !clean.is_char_boundary(end) {
            end -= 1;
        }
        clean = format!("{}{extension}", &clean[..end]);
    }
    clean
}
pub(super) fn unique_name(name: &str, taken: &mut HashSet<String>) -> String {
    let stem = std::path::Path::new(name)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let suffix = std::path::Path::new(name)
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    let mut candidate = name.to_owned();
    let mut count = 2;
    while !taken.insert(caseless::default_case_fold_str(&candidate)) {
        candidate = format!("{stem} ({count}){suffix}");
        count += 1;
    }
    candidate
}
pub(super) fn reserve_outputs(items: &[Item]) -> HashMap<String, String> {
    let mut taken = HashSet::new();
    let mut result = HashMap::new();
    for item in items {
        let raw = if item.kind == "url" {
            markitai_core::output::url_name(&item.name, &Default::default())
        } else {
            item.name.clone()
        };
        let name = sanitize_name(&raw);
        let mut base = name.clone();
        let mut count = 2;
        loop {
            let keys = [format!("{base}.md"), format!("{base}.llm.md")]
                .map(|s| caseless::default_case_fold_str(&s));
            if keys.iter().all(|key| !taken.contains(key)) {
                taken.extend(keys);
                break;
            }
            base = format!("{name} ({count})");
            count += 1;
        }
        result.insert(item.item_id.clone(), base);
    }
    result
}

pub(super) fn get(state: &State, id: &str) -> ApiResult<Arc<Job>> {
    state
        .jobs
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| ApiError::new(404, "job not found"))
}

async fn convert_one(
    state: Arc<State>,
    job: Arc<Job>,
    index: usize,
    cfg: Value,
    runtime: Arc<markitai_core::LlmRuntime>,
    explicit: Option<String>,
) {
    let item = job.data.lock().unwrap().items[index].clone();
    let semaphore = if item.kind == "url" {
        state.url_slots.clone()
    } else {
        state.file_slots.clone()
    };
    let mut closing = state.shutdown.subscribe();
    let permit = if state.closing.load(Ordering::SeqCst) {
        None
    } else {
        tokio::select! { biased; _=closing.changed()=>None, permit=semaphore.acquire_owned()=>permit.ok() }
    };
    if permit.is_none() || state.closing.load(Ordering::SeqCst) {
        let mut data = job.data.lock().unwrap();
        let item = &mut data.items[index];
        item.status = "error".into();
        item.error = Some("cancelled (server shutdown)".into());
        item.finished_at = Some(now());
        let _ = job.events.send(("item", json!(item)));
        return;
    }
    let base = job.data.lock().unwrap().bases[&item.item_id].clone();
    {
        let mut data = job.data.lock().unwrap();
        data.items[index].status = "running".into();
        let _ = job.events.send(("item", json!(data.items[index])));
    }
    let started = Instant::now();
    let out = job.folder.join("out");
    let upload = job.folder.join("uploads").join(&item.name);
    let target = if item.kind == "url" {
        item.name.clone()
    } else {
        upload.to_string_lossy().into_owned()
    };
    let mut cfg = cfg;
    cfg["output"]["filename"] = json!(format!("{base}.md"));
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        markitai_core::convert_with_context_detailed(
            &target,
            markitai_core::ConvertOptions {
                output_dir: Some(out),
                config: Some(cfg),
                ..Default::default()
            },
            markitai_core::ConvertContext {
                explicit_fetch_strategy: explicit.as_deref(),
                llm_runtime: Some(&runtime),
            },
        )
    })
    .await;
    let mut data = job.data.lock().unwrap();
    let mut assets = None;
    let item = &mut data.items[index];
    item.duration_ms = Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
    item.finished_at = Some(now());
    match result {
        Ok(Ok(result)) => {
            item.status = "done".into();
            item.skipped = result.skip_reason.is_some();
            item.skip_reason = result.skip_reason;
            item.error = item
                .skip_reason
                .as_ref()
                .map(|reason| format!("skipped ({reason})"));
            item.llm_enhanced = result.llm_output_path.is_some();
            item.cost_usd = Some(result.usage.cost_usd);
            item.diagnostics = AttemptDiagnostics::completed(Operation::Convert, result.usage);
            item.warnings = result.warnings;
            item.output = result.llm_output_path.or(result.output_path).and_then(|p| {
                p.strip_prefix(job.folder.join("out"))
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned())
            });
            assets = Some(
                result
                    .assets
                    .into_iter()
                    .chain(result.screenshots)
                    .filter_map(|p| {
                        p.strip_prefix(job.folder.join("out"))
                            .ok()
                            .map(|p| p.to_string_lossy().into_owned())
                    })
                    .collect(),
            );
        }
        Ok(Err(failure)) if matches!(&failure.error, markitai_core::Error::ImageOnly(_)) => {
            item.status = "done".into();
            item.skipped = true;
            item.skip_reason = Some("image_only".into());
            item.error = Some("skipped (image_only)".into());
        }
        Ok(Err(failure)) => {
            item.status = "error".into();
            let message = failure.error.to_string();
            item.diagnostics =
                AttemptDiagnostics::failed(Operation::Convert, message.clone(), failure.usage);
            item.error = Some(message);
        }
        Err(_) => {
            item.status = "error".into();
            item.error = Some("internal conversion error".into());
        }
    }
    let id = item.item_id.clone();
    let payload = json!(item);
    if let Some(assets) = assets {
        data.assets.insert(id, assets);
    }
    let _ = job.events.send(("item", payload));
    let _ = job.events.send(("job", data.progress()));
}

pub(super) fn start(state: Arc<State>, job: Arc<Job>, cfg: Value) -> ApiResult<()> {
    let concurrency = cfg["llm"]["concurrency"].as_u64().unwrap_or(10).max(1) as usize;
    let runtime =
        Arc::new(markitai_core::LlmRuntime::new(concurrency).map_err(ApiError::internal)?);
    let _ = job.runtime.set(runtime.clone());
    let count = job.data.lock().unwrap().items.len();
    job.active.fetch_add(count, Ordering::SeqCst);
    let explicit = job.data.lock().unwrap().options["strategy"]
        .as_str()
        .map(str::to_owned);
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        let mut pending = FuturesUnordered::new();
        for index in 0..count {
            pending.push(convert_one(
                task_state.clone(),
                job.clone(),
                index,
                cfg.clone(),
                runtime.clone(),
                explicit.clone(),
            ));
        }
        while pending.next().await.is_some() {
            complete(&task_state, job.clone()).await;
        }
    });
    state.tasks.lock().unwrap().push(task);
    Ok(())
}

// Admission and finalization take access before data; no network work holds either.
pub(super) async fn complete(state: &Arc<State>, job: Arc<Job>) {
    let finalized = tokio::task::spawn_blocking(move || {
        let _access = job.access.lock().unwrap();
        if job.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            store::finish(&job)
        } else {
            Ok(())
        }
    })
    .await;
    if !matches!(finalized, Ok(Ok(()))) {
        state.persistence_failed.store(true, Ordering::SeqCst);
    }
}
