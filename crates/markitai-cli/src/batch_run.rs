//! The coordinator owns recovery mutations and admits work only after durable claims.
use super::*;
use crate::output_claims::{
    Claim, Error as ClaimError, MemberLeases, Owner, Policy, adopt_owner, reservation_members,
    reserve_keys,
};
use crate::run_state::{
    self, Entry, ItemKey, Limits, LoadOutcome, Mode, Scope, Snapshot, StateStore, Status, codec,
};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

pub(super) type Reservations = HashMap<(u64, u64), BTreeSet<ItemKey>>;

fn item_key(task: &Task) -> ItemKey {
    if is_url(&task.source) {
        ItemKey::Url(task.report_key.clone())
    } else {
        ItemKey::File(task.report_key.clone())
    }
}
fn entry<'a>(snapshot: &'a Snapshot, key: &ItemKey) -> Option<&'a Entry> {
    match key {
        ItemKey::File(key) => snapshot.documents.get(key),
        ItemKey::Url(key) => snapshot.urls.get(key),
    }
}
fn stem(task: &Task, cfg: &Value) -> String {
    cfg["output"]["filename"]
        .as_str()
        .map(|name| name.strip_suffix(".md").unwrap_or(name).to_owned())
        .or_else(|| task.reserved_stem.clone())
        .or_else(|| {
            task.filename
                .as_ref()
                .map(|name| name.strip_suffix(".md").unwrap_or(name).to_owned())
        })
        .unwrap_or_else(|| {
            if is_url(&task.source) {
                markitai_core::output::url_name(&task.source, &Default::default())
            } else {
                Path::new(&task.source)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            }
        })
}
fn members(stem: &str) -> [String; 2] {
    [format!("{stem}.md"), format!("{stem}.llm.md")]
}
fn present(path: &Path) -> Result<bool, ClaimError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// A prior target grants no overwrite authority; the receipt must prove it.
pub(super) fn claim(
    task: &mut Task,
    cfg: &Value,
    owner: Option<Owner>,
    retry: Option<&Path>,
    reserved: &Reservations,
) -> Result<Option<Claim>, ClaimError> {
    let Some(directory) = task.output.as_deref() else {
        return Ok(None);
    };
    let mode = cfg["output"]["on_conflict"].as_str().unwrap_or("rename");
    let allow = config::enabled(cfg, "/output/allow_symlinks");
    let initial = if let Some(target) = retry {
        let parent = crate::report_store::resolve_path(
            target
                .parent()
                .ok_or_else(|| ClaimError::Invalid("retry target has no parent".into()))?,
        )?;
        if parent != crate::report_store::resolve_path(directory)? {
            return Err(ClaimError::Ownership(
                "retry target is outside its planned parent".into(),
            ));
        }
        target
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".md"))
            .ok_or_else(|| {
                ClaimError::Invalid("retry target is not a base Markdown filename".into())
            })?
            .to_owned()
    } else {
        stem(task, cfg)
    };
    // Bound conflict scans independently of the number of discovered work items.
    for version in 1..=10000usize {
        let candidate = if version == 1 {
            initial.clone()
        } else {
            format!("{initial}.v{version}")
        };
        let names = members(&candidate);
        let leases = match MemberLeases::acquire(directory, &names, allow) {
            Ok(leases) => leases,
            Err(ClaimError::Busy) if retry.is_none() && mode == "rename" => continue,
            Err(error) => return Err(error),
        };
        let identity = item_key(task);
        let blocked = leases.keys().iter().any(|key| {
            reserved
                .get(key)
                .is_some_and(|owners| owners.iter().any(|owner| owner != &identity))
        });
        if blocked {
            if retry.is_some() {
                return Err(ClaimError::Ownership(
                    "retry target is reserved by another completed or scheduled item".into(),
                ));
            }
            continue;
        }
        let occupied = names
            .iter()
            .map(|name| present(&leases.parent().join(name)))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .any(|yes| yes);
        if retry.is_none() && mode == "rename" && occupied {
            continue;
        }
        let policy = if retry.is_some() {
            Policy::RetryOwned
        } else if mode == "overwrite" {
            Policy::Overwrite
        } else {
            Policy::NoClobber
        };
        let skip = retry.is_none() && mode == "skip" && occupied;
        task.reserved_stem = Some(candidate);
        return Claim::new(leases, owner, policy, skip).map(Some);
    }
    Err(ClaimError::Invalid(
        "output conflict candidate limit exceeded".into(),
    ))
}

fn options(cli: &Cli, cfg: &Value, scope: &Scope) -> Value {
    let mut options = json!({"llm":config::enabled(cfg,"/llm/enabled"),"ocr":config::enabled(cfg,"/ocr/enabled"),
        "screenshot":config::enabled(cfg,"/screenshot/enabled"),"alt":config::enabled(cfg,"/image/alt_enabled"),
        "desc":config::enabled(cfg,"/image/desc_enabled"),"input_dir":if scope.mode==Mode::Directory {scope.input.as_path()} else {scope.input.parent().unwrap()},
        "output_dir":scope.output});
    if scope.mode == Mode::Directory {
        options["scan_max_depth"] = json!(
            cli.max_depth
                .unwrap_or_else(|| cfg["batch"]["scan_max_depth"].as_u64().unwrap_or(5) as usize)
        );
        let globs: Vec<_> = cli
            .globs
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !globs.is_empty() {
            options["glob_patterns"] = json!(globs);
        }
        options["concurrency"] = cfg["batch"]["concurrency"].clone();
    }
    options
}
fn discovered(tasks: &[Task], options: Value) -> Snapshot {
    let mut state = Snapshot {
        options: serde_json::from_value(options).expect("ordered state options"),
        ..Snapshot::default()
    };
    for task in tasks {
        if is_url(&task.source) {
            state.urls.insert(
                task.report_key.clone(),
                Entry {
                    source_file: task.source_file.clone(),
                    url: Some(task.source.clone()),
                    ..Entry::default()
                },
            );
        } else {
            state
                .documents
                .insert(task.report_key.clone(), Entry::default());
        }
    }
    state
}
fn owner(scope: &Scope, generation: &str, task: &Task) -> Owner {
    Owner {
        generation: generation.into(),
        mode: if scope.mode == Mode::Directory {
            "directory"
        } else {
            "url_list"
        }
        .into(),
        input: scope.input.clone(),
        output: scope.output.clone(),
        kind: if is_url(&task.source) { "url" } else { "file" }.into(),
        key: task.report_key.clone(),
    }
}
fn failure(task: &Task, index: usize, error: String) -> RunItem {
    recorded(
        task,
        index,
        Instant::now(),
        timestamp(),
        &Err(markitai_core::Error::Conversion(error).into()),
    )
}
fn terminal(store: &mut StateStore, task: &Task, record: &RunItem) -> Result<(), run_state::Error> {
    store.record(
        item_key(task),
        json!({"status":if record.status==ItemStatus::Failed {"failed"} else {"completed"},
        "output":record.output,"error":record.error,"diagnostics":record.diagnostics}),
    )?;
    Ok(())
}
struct Work {
    index: usize,
    task: Task,
    claim: Claim,
    previous: Entry,
}

// An older directory scan may have persisted work inside a package. Reject
// that scope before receipt adoption or checkpoint upgrade; replay is read-only
// and a stable state lock does not grant permission to rewrite old work.
fn reject_package_members(snapshot: &Snapshot, scope: &Scope) -> CliResult<()> {
    let mut checked = std::collections::HashMap::<PathBuf, bool>::new();
    let mut inspect = |source: PathBuf| -> CliResult<()> {
        let resolved = crate::report_store::resolve_path(&source).map_err(runtime)?;
        for path in [&source, &resolved] {
            for ancestor in path.ancestors().skip(1) {
                let package = *checked
                    .entry(ancestor.to_owned())
                    .or_insert_with(|| markitai_core::formats::is_numbers_package_path(ancestor));
                if package {
                    return Err(runtime(
                        "Recovery state contains an item inside a Numbers directory package; preserve this state and use a fresh output directory without --resume to convert the package as one document",
                    ));
                }
            }
        }
        Ok(())
    };
    for key in snapshot.documents.keys() {
        inspect(scope.input.join(key))?;
    }
    for entry in snapshot.urls.values() {
        if let Some(source) = entry
            .source_file
            .as_deref()
            .filter(|source| !source.is_empty())
        {
            inspect(PathBuf::from(source))?;
        }
    }
    Ok(())
}

pub(super) fn run(
    cli: &Cli,
    cfg: &Value,
    mut tasks: Vec<Task>,
    destination: BatchDestination<'_>,
    report_plan: Option<&report::ReportPlan>,
    clock: Instant,
    context: ConvertContext<'_>,
) -> CliResult<i32> {
    let BatchDestination {
        mode,
        output,
        history,
    } = destination;
    let scope = Scope::new(
        if mode == RunMode::Directory {
            Mode::Directory
        } else {
            Mode::UrlList
        },
        Path::new(cli.input.as_deref().unwrap()),
        output,
    )
    .map_err(runtime)?;
    let state_options = options(cli, cfg, &scope);
    let hash = codec::task_hash(&scope, &state_options).map_err(runtime)?;
    if tasks.is_empty()
        && !scope
            .output
            .join(format!(".markitai/states/markitai.{hash}.state.json"))
            .exists()
    {
        if cli.json {
            emit_json(&[], None);
        }
        return Ok(0);
    }
    // Platform preflight precedes any request, worker or state mutation.
    #[cfg(not(unix))]
    return Err(runtime(
        "Durable output ownership is not implemented for this platform yet",
    ));
    let _signals = crate::signals::Guard::install().map_err(runtime)?;
    let allow = config::enabled(cfg, "/output/allow_symlinks");
    let mut store =
        StateStore::open(scope.clone(), &hash, allow, Limits::default()).map_err(runtime)?;
    let mut snapshot = if cli.resume {
        match store.load().map_err(runtime)? {
            LoadOutcome::Loaded { snapshot, warnings } => {
                for warning in warnings {
                    eprintln!("Warning: {warning}");
                }
                *snapshot
            }
            LoadOutcome::Missing => {
                if !cli.quiet {
                    eprintln!(
                        "No recovery state matches these paths and options; starting a fresh batch."
                    );
                }
                Snapshot::default()
            }
            LoadOutcome::Corrupt { reason } => {
                eprintln!(
                    "Warning: recovery state could not be loaded ({reason}); preserving it before a fresh checkpoint"
                );
                Snapshot::default()
            }
        }
    } else {
        Snapshot::default()
    };
    if cli.resume {
        reject_package_members(&snapshot, &scope)?;
    }
    let fresh = discovered(&tasks, state_options);
    let url_order: Vec<_> = fresh.urls.keys().cloned().collect();
    let native = snapshot.checkpoint.is_some();
    if let Some(checkpoint) = &snapshot.checkpoint {
        // Copy publication evidence before persisting an adopted URL identity. Keeping the old receipt makes a crash before begin safe.
        let mut adopted = BTreeSet::new();
        for key in &url_order {
            let bare = fresh.urls[key].url.as_deref().unwrap();
            if key == bare
                || snapshot.urls.contains_key(key)
                || fresh.urls.contains_key(bare)
                || !adopted.insert(bare.to_owned())
            {
                continue;
            }
            let Some(old) = snapshot.urls.get(bare) else {
                continue;
            };
            if old.status != Status::Failed || old.target.is_none() {
                continue;
            }
            let Some(path) = old.target.as_ref().or(old.output.as_ref()) else {
                continue;
            };
            let Some(parent) = path.parent() else {
                continue;
            };
            let previous = Owner {
                generation: checkpoint.generation.clone(),
                mode: if scope.mode == Mode::Directory {
                    "directory"
                } else {
                    "url_list"
                }
                .into(),
                input: scope.input.clone(),
                output: scope.output.clone(),
                kind: "url".into(),
                key: bare.into(),
            };
            let mut next = previous.clone();
            next.key = key.clone();
            let names = if old.target.is_some() {
                let base = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix(".md"))
                    .ok_or_else(|| runtime("Adopted URL target is not a Markdown filename"))?;
                members(base).to_vec()
            } else if let Some(names) =
                reservation_members(parent, &previous, path).map_err(runtime)?
            {
                names
            } else {
                continue;
            };
            let leases = MemberLeases::acquire(parent, &names, allow).map_err(runtime)?;
            adopt_owner(&leases, &previous, &next).map_err(runtime)?;
        }
    }
    if cli.resume {
        codec::merge(&mut snapshot, &fresh, &url_order, Limits::default()).map_err(runtime)?;
    } else {
        snapshot = fresh;
    }
    if snapshot.options.is_empty() {
        snapshot.options = discovered(&[], options(cli, cfg, &scope)).options;
    }
    for task in tasks.iter_mut().filter(|task| is_url(&task.source)) {
        let key = item_key(task);
        if let Some(saved) = entry(&snapshot, &key) {
            task.source_file = saved.source_file.clone();
            let parent = codec::entry_parent(&key, saved, &scope, allow).map_err(runtime)?;
            if task
                .output
                .as_ref()
                .map(|path| crate::report_store::resolve_path(path))
                .transpose()
                .map_err(runtime)?
                .as_ref()
                != Some(&parent)
            {
                task.output = Some(parent);
            }
        }
    }
    // Directory recovery retains pending files even when discovery no longer sees them.
    if mode == RunMode::Directory {
        let keys: BTreeSet<_> = tasks
            .iter()
            .filter(|t| !is_url(&t.source))
            .map(|t| t.report_key.clone())
            .collect();
        for (key, entry) in &snapshot.documents {
            if entry.status != Status::Completed && !keys.contains(key) {
                tasks.push(Task {
                    source: scope.input.join(key).to_string_lossy().into_owned(),
                    display: key.clone(),
                    report_key: key.clone(),
                    output: Some(
                        scope
                            .output
                            .join(Path::new(key).parent().unwrap_or(Path::new(""))),
                    ),
                    filename: None,
                    reserved_stem: None,
                    source_file: None,
                });
            }
        }
    }
    if !native {
        // Legacy targets are naming hints, not native write authority. Do not upgrade an unproved target into a retry claim if interrupted before admission.
        for saved in snapshot
            .documents
            .values_mut()
            .chain(snapshot.urls.values_mut())
        {
            if saved.status != Status::Completed {
                saved.target = None;
            }
        }
    }
    let retries: Vec<_> = tasks
        .iter()
        .map(|task| {
            entry(&snapshot, &item_key(task))
                .filter(|entry| native && entry.status == Status::Failed)
                .and_then(|entry| entry.target.clone())
        })
        .collect();
    // Reserve all current identities consistently before work, but acquire OS locks only for active work.
    let pending: Vec<_> = tasks
        .iter()
        .enumerate()
        .filter(|(_, task)| {
            entry(&snapshot, &item_key(task)).is_some_and(|entry| entry.status != Status::Completed)
        })
        .map(|(index, _)| index)
        .collect();
    let mut ordinary: Vec<_> = pending
        .iter()
        .filter(|&&index| retries[index].is_none())
        .map(|&index| tasks[index].clone())
        .collect();
    reserve_batch_names(&mut ordinary, cfg)?;
    for (index, planned) in pending
        .iter()
        .copied()
        .filter(|&index| retries[index].is_none())
        .zip(ordinary)
    {
        tasks[index] = planned;
    }
    store.begin(snapshot).map_err(runtime)?;
    if !cli.quiet
        && let Some(backup) = store.legacy_backup()
    {
        eprintln!(
            "Preserved original legacy recovery files at {}. This backup does not undo output or model work.",
            backup.display()
        );
    }
    let generation = store
        .snapshot()
        .unwrap()
        .checkpoint
        .as_ref()
        .unwrap()
        .generation
        .clone();
    let mut reserved = Reservations::new();
    let snapshot = store.snapshot().unwrap();
    for (kind, key, entry) in snapshot
        .documents
        .iter()
        .map(|(key, entry)| ("file", key, entry))
        .chain(snapshot.urls.iter().map(|(key, entry)| ("url", key, entry)))
        .filter(|(_, _, entry)| entry.status == Status::Completed)
    {
        let Some(path) = &entry.output else { continue };
        let Some(parent) = path.parent() else {
            continue;
        };
        let identity = Owner {
            generation: generation.clone(),
            mode: if scope.mode == Mode::Directory {
                "directory"
            } else {
                "url_list"
            }
            .into(),
            input: scope.input.clone(),
            output: scope.output.clone(),
            kind: kind.into(),
            key: key.clone(),
        };
        let names = match reservation_members(parent, &identity, path).map_err(runtime)? {
            Some(names) => names,
            None => {
                // Legacy state lacks publication family evidence. Protect both interpretations of an .llm.md result.
                let Some(base) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix(".md"))
                else {
                    continue;
                };
                let mut names: BTreeSet<_> = members(base).into_iter().collect();
                if let Some(base) = base.strip_suffix(".llm") {
                    names.extend(members(base));
                }
                names.into_iter().collect()
            }
        };
        let item = if kind == "file" {
            ItemKey::File(key.clone())
        } else {
            ItemKey::Url(key.clone())
        };
        for names in names.chunks(2) {
            for key in reserve_keys(parent, names, allow).map_err(runtime)? {
                reserved.entry(key).or_default().insert(item.clone());
            }
        }
    }
    let file_limit = cfg["batch"]["concurrency"].as_u64().unwrap_or(10) as usize;
    let url_limit = cfg["batch"]["url_concurrency"].as_u64().unwrap_or(5) as usize;
    // Separate queues avoid rescanning every blocked item whenever one worker finishes.
    let (mut pending_urls, mut pending_files): (VecDeque<_>, VecDeque<_>) = pending
        .into_iter()
        .partition(|&index| is_url(&tasks[index].source));
    // An absent task class cannot use workers reserved for its concurrency cap.
    let count = file_limit
        .min(pending_files.len())
        .saturating_add(url_limit.min(pending_urls.len()))
        .max(1);
    let seconds = cfg["batch"]["state_flush_interval_seconds"]
        .as_u64()
        .unwrap_or(0);
    let interval = Duration::from_secs(if seconds > 0 { seconds } else { 5 });
    let (jobs, job_receiver) = mpsc::channel::<Work>();
    let job_receiver = Arc::new(Mutex::new(job_receiver));
    let (finished, receiver) = mpsc::channel::<(usize, RunItem)>();
    let mut records = Vec::new();
    let mut fatal = None;
    let mut signal = None;
    let mut last_flush = Instant::now();
    let mut active_files = 0usize;
    let mut active_urls = 0usize;
    std::thread::scope(|threads| {
        for _ in 0..count {
            let jobs = Arc::clone(&job_receiver);
            let finished = finished.clone();
            threads.spawn(move || {
                loop {
                    let work = match jobs.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(_) => break,
                    };
                    let Ok(work) = work else { break };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        convert_item(&work.task, work.index, cfg, context, Some(&work.claim)).0
                    }));
                    let record = result.unwrap_or_else(|_| {
                        failure(&work.task, work.index, "Conversion worker panicked".into())
                    });
                    if finished.send((work.index, record)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(finished);
        loop {
            if signal.is_none()
                && let Some(received) = crate::signals::interrupted()
            {
                signal = Some(received);
                eprintln!("Interrupted: stopping new work and waiting for active conversions.");
                if let Err(error) = store.flush() {
                    fatal.get_or_insert_with(|| error.to_string());
                }
            }
            let mut admitted = Vec::new();
            if signal.is_none() && fatal.is_none() {
                loop {
                    if crate::signals::interrupted().is_some() {
                        break;
                    }
                    let file = pending_files
                        .front()
                        .copied()
                        .filter(|_| active_files < file_limit);
                    let url = pending_urls
                        .front()
                        .copied()
                        .filter(|_| active_urls < url_limit);
                    let index = match (file, url) {
                        (Some(file), Some(url)) if file < url => pending_files.pop_front().unwrap(),
                        (Some(_), Some(_)) | (None, Some(_)) => pending_urls.pop_front().unwrap(),
                        (Some(_), None) => pending_files.pop_front().unwrap(),
                        (None, None) => break,
                    };
                    let previous = entry(store.snapshot().unwrap(), &item_key(&tasks[index]))
                        .unwrap()
                        .clone();
                    let identity = owner(&scope, &generation, &tasks[index]);
                    let claim = match claim(
                        &mut tasks[index],
                        cfg,
                        Some(identity),
                        retries[index].as_deref(),
                        &reserved,
                    ) {
                        Ok(Some(claim)) => claim,
                        Ok(None) => unreachable!("batch always writes"),
                        Err(error) => {
                            let record = failure(&tasks[index], index, error.to_string());
                            if let Err(error) = terminal(&mut store, &tasks[index], &record) {
                                fatal = Some(error.to_string());
                            }
                            records.push(record);
                            if fatal.is_some() {
                                break;
                            }
                            continue;
                        }
                    };
                    let target = claim.parent().join(format!(
                        "{}.md",
                        tasks[index].reserved_stem.as_ref().unwrap()
                    ));
                    if let Err(error) = store.record(
                        item_key(&tasks[index]),
                        json!({"status":"in_progress","target":target,"output":null,"error":null,"diagnostics":null}),
                    ) {
                        fatal = Some(error.to_string());
                        break;
                    }
                    if claim.is_skip() {
                        // A skip performs no provider work and owns no output name after its short check.
                        if let Err(error) = store.flush() {
                            fatal = Some(error.to_string());
                            break;
                        }
                        last_flush = Instant::now();
                        let record =
                            convert_item(&tasks[index], index, cfg, context, Some(&claim)).0;
                        if let Err(error) = terminal(&mut store, &tasks[index], &record) {
                            fatal = Some(error.to_string());
                        }
                        records.push(record);
                        if fatal.is_some() {
                            break;
                        }
                        continue;
                    }
                    for key in claim.keys() {
                        reserved
                            .entry(key)
                            .or_default()
                            .insert(item_key(&tasks[index]));
                    }
                    if is_url(&tasks[index].source) {
                        active_urls += 1;
                    } else {
                        active_files += 1;
                    }
                    admitted.push(Work {
                        index,
                        task: tasks[index].clone(),
                        claim,
                        previous,
                    });
                }
            }
            if !admitted.is_empty() {
                if fatal.is_none()
                    && let Err(error) = store.flush()
                {
                    fatal = Some(error.to_string());
                }
                last_flush = Instant::now();
                for work in admitted {
                    if fatal.is_some() || crate::signals::interrupted().is_some() {
                        if let Err(error)=store.record(item_key(&work.task),json!({"status":work.previous.status,
                            "target":work.previous.target,"output":work.previous.output,"error":work.previous.error,"diagnostics":work.previous.observations.get("diagnostics")})) {
                            fatal.get_or_insert_with(||error.to_string());
                        }
                        if is_url(&work.task.source) {
                            active_urls -= 1;
                        } else {
                            active_files -= 1;
                        }
                    } else if jobs.send(work).is_err() {
                        fatal = Some("Conversion worker queue closed unexpectedly".into());
                        break;
                    }
                }
            }
            if active_files + active_urls == 0
                && ((pending_files.is_empty() && pending_urls.is_empty())
                    || signal.is_some()
                    || fatal.is_some())
            {
                break;
            }
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(first) => {
                    // Drain ready completions together so the next durable admission can fill several slots with one flush.
                    let mut ready = Some(first);
                    while let Some((index, record)) = ready {
                        if is_url(&tasks[index].source) {
                            active_urls -= 1;
                        } else {
                            active_files -= 1;
                        }
                        if let Err(error) = terminal(&mut store, &tasks[index], &record) {
                            fatal.get_or_insert_with(|| error.to_string());
                        }
                        records.push(record);
                        ready = receiver.try_recv().ok();
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    fatal.get_or_insert("Conversion workers stopped unexpectedly".into());
                    break;
                }
            }
            if last_flush.elapsed() >= interval {
                if let Err(error) = store.flush() {
                    fatal.get_or_insert_with(|| error.to_string());
                }
                last_flush = Instant::now();
            }
        }
        drop(jobs);
    });
    if signal.is_none()
        && let Some(received) = crate::signals::interrupted()
    {
        signal = Some(received);
        eprintln!("Interrupted: stopping new work and waiting for active conversions.");
    }
    if let Err(error) = store.compact() {
        fatal.get_or_insert_with(|| error.to_string());
    }
    if let Some(error) = &fatal {
        eprintln!("Error: {error}");
    }
    let storage_failed = fatal.is_some();
    if signal.is_none()
        && fatal.is_none()
        && let Some(plan) = report_plan
    {
        let finished = RunFinished {
            updated_at: timestamp(),
            duration_s: clock.elapsed().as_secs_f64(),
        };
        let rendered = if cli.resume {
            report::render_resumed(
                plan,
                &records,
                store.snapshot().unwrap(),
                &url_order,
                &finished,
            )
        } else {
            report::render(plan, &records, &finished)
        };
        match rendered.and_then(|bytes| report::publish(plan, &bytes)) {
            Ok(publication) if cli.verbose && !cli.quiet => match publication {
                crate::report_store::Publication::Written(path) => {
                    eprintln!("Report: {}", path.display())
                }
                crate::report_store::Publication::SkippedExisting(path) => {
                    eprintln!("Existing report preserved: {}", path.display())
                }
            },
            Ok(_) => (),
            Err(error) => {
                eprintln!("Error: {error}");
                fatal.get_or_insert(error);
            }
        }
    }
    if signal.is_none()
        && !storage_failed
        && let Some(plan) = history
    {
        plan.record(&records);
    }
    if signal.is_none()
        && let Some(received) = crate::signals::interrupted()
    {
        signal = Some(received);
        eprintln!("Interrupted: stopping new work and waiting for active conversions.");
    }
    records.sort_by_key(|record| record.index);
    let items: Vec<_> = records.iter().map(outcome).collect();
    let failed = records
        .iter()
        .filter(|record| record.status == ItemStatus::Failed)
        .count();
    if cli.json {
        emit_json(
            &items,
            fatal.as_deref().or(if signal.is_some() {
                Some("Conversion interrupted")
            } else {
                None
            }),
        );
    } else {
        for record in &records {
            if !cli.quiet {
                for warning in &record.warnings {
                    eprintln!("Warning: {}: {warning}", record.display);
                }
            }
            if let Some(error) = &record.error {
                eprintln!("Error: {}: {error}", record.display);
            }
        }
        if !cli.quiet {
            eprintln!(
                "{} items, {} completed, {} failed",
                records.len(),
                records
                    .iter()
                    .filter(|record| record.status == ItemStatus::Completed)
                    .count(),
                failed
            );
        }
    }
    Ok(if let Some(signal) = signal {
        128 + signal
    } else if storage_failed {
        1
    } else if failed > 0 {
        10
    } else if fatal.is_some() {
        1
    } else {
        0
    })
}
