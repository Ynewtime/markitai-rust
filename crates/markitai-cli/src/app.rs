use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use markitai_core::{ConversionOutput, ConvertOptions, config};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

#[derive(Parser, Debug)]
#[command(name="markitai", version=markitai_core::VERSION,
    about="Convert documents and URLs to Markdown", disable_help_subcommand=true, args_override_self=true)]
struct Cli {
    #[arg(value_name = "INPUT")]
    input: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(short = 'o', long, value_name = "PATH")]
    output: Option<PathBuf>,
    #[arg(long, requires="output", conflicts_with_all=["dry_run","llm_batch_collect"])]
    json: bool,
    #[arg(short = 'c', long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    config_json: Option<String>,
    #[arg(short = 'p', long)]
    preset: Option<String>,
    #[arg(long, value_parser=["rag","obsidian","okf"], ignore_case=true)]
    profile: Option<String>,
    #[arg(long, overrides_with = "no_llm")]
    llm: bool,
    #[arg(long, overrides_with = "llm")]
    no_llm: bool,
    #[arg(long, overrides_with = "no_alt")]
    alt: bool,
    #[arg(long, overrides_with = "alt")]
    no_alt: bool,
    #[arg(long, overrides_with = "no_desc")]
    desc: bool,
    #[arg(long, overrides_with = "desc")]
    no_desc: bool,
    #[arg(long, overrides_with = "no_ocr")]
    ocr: bool,
    #[arg(long, overrides_with = "ocr")]
    no_ocr: bool,
    #[arg(long, overrides_with = "no_screenshot")]
    screenshot: bool,
    #[arg(long, overrides_with = "screenshot")]
    no_screenshot: bool,
    #[arg(long, overrides_with = "no_screenshot_only")]
    screenshot_only: bool,
    #[arg(long, overrides_with = "screenshot_only")]
    no_screenshot_only: bool,
    #[arg(long, overrides_with = "no_pure")]
    pure: bool,
    #[arg(long, overrides_with = "pure")]
    no_pure: bool,
    #[arg(long)]
    keep_base: bool,
    #[arg(long)]
    resume: bool,
    #[arg(long, overrides_with = "compress")]
    no_compress: bool,
    #[arg(long, overrides_with = "no_compress")]
    compress: bool,
    #[arg(long, overrides_with = "cache")]
    no_cache: bool,
    #[arg(long, overrides_with = "no_cache")]
    cache: bool,
    #[arg(long)]
    no_cache_for: Option<String>,
    #[arg(short='j', long, value_parser=clap::value_parser!(u32).range(1..))]
    batch_concurrency: Option<u32>,
    #[arg(long, value_parser=clap::value_parser!(u32).range(1..))]
    url_concurrency: Option<u32>,
    #[arg(long, value_parser=clap::value_parser!(u32).range(1..))]
    llm_concurrency: Option<u32>,
    #[arg(short='g', long="glob", action=ArgAction::Append)]
    globs: Vec<String>,
    #[arg(long)]
    max_depth: Option<usize>,
    #[arg(long)]
    llm_batch: bool,
    #[arg(long, value_parser=clap::value_parser!(u64).range(60..))]
    llm_batch_timeout: Option<u64>,
    #[arg(long)]
    llm_batch_collect: Option<String>,
    #[arg(short='s', long, value_parser=["auto","static","playwright","defuddle","jina","cloudflare"])]
    strategy: Option<String>,
    #[arg(short='b', long, value_parser=["native","cloudflare"])]
    backend: Option<String>,
    #[arg(long)]
    no_remote_fetch: bool,
    #[arg(short = 'v', long)]
    verbose: bool,
    #[arg(short = 'q', long)]
    quiet: bool,
    #[arg(long, value_parser=["DEBUG","INFO","WARNING","ERROR","CRITICAL"])]
    log_level: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, overrides_with = "no_record_history")]
    record_history: bool,
    #[arg(long, overrides_with = "record_history")]
    no_record_history: bool,
    #[arg(short = 'I', long)]
    interactive: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Inspect or edit configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Create a minimal configuration file.
    Init {
        #[arg(short = 'y', long)]
        yes: bool,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
        #[arg(long)]
        local: bool,
    },
    /// Report native runtime capabilities.
    Doctor {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        fix: bool,
        #[arg(long)]
        suggest_extras: bool,
    },
    /// Inspect and clear persistent document enhancement cache.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Provider authentication (not yet implemented).
    Auth {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Web service (not yet implemented).
    Serve {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// MCP service (not yet implemented).
    Mcp,
}

#[derive(Subcommand, Debug)]
enum CacheCommand {
    Stats {
        #[arg(long)]
        json: bool,
        #[arg(short, long)]
        verbose: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Clear {
        #[arg(short, long)]
        yes: bool,
        #[arg(long)]
        include_spa_domains: bool,
    },
    /// Learned browser domains (not yet implemented).
    SpaDomains {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        clear: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigCommand {
    List {
        #[arg(short='f', long="format", default_value="json", value_parser=["json","yaml","table"], ignore_case=true)]
        format: String,
        #[arg(long)]
        show_secrets: bool,
    },
    Path,
    Validate {
        config_file: Option<PathBuf>,
    },
    Get {
        key: String,
        #[arg(long)]
        show_secrets: bool,
    },
    Set {
        key: String,
        value: String,
        #[arg(long)]
        show_secrets: bool,
    },
    Edit,
}

pub fn run() -> i32 {
    if std::env::args_os().len() == 1 {
        let _ = Cli::command().print_help();
        println!();
        return 0;
    }
    let cli = Cli::parse();
    match execute(&cli) {
        Ok(code) => code,
        Err((code, message)) => {
            eprintln!("Error: {message}");
            if cli.json && code != 2 {
                emit_json(&[], Some(&message));
            }
            code
        }
    }
}

type CliResult<T> = Result<T, (i32, String)>;
fn runtime(error: impl std::fmt::Display) -> (i32, String) {
    (1, error.to_string())
}
fn unsupported(feature: &str) -> (i32, String) {
    (
        1,
        format!("{feature} is not implemented in this Rust development build; see docs/cli.md"),
    )
}
fn tri(yes: bool, no: bool) -> Option<bool> {
    if yes {
        Some(true)
    } else if no {
        Some(false)
    } else {
        None
    }
}

fn execute(cli: &Cli) -> CliResult<i32> {
    if cli.command.is_some() && cli.input.is_some() {
        return Err((2, "Cannot mix INPUT with a subcommand".into()));
    }
    let permits_missing = matches!(
        cli.command,
        Some(Command::Config {
            command: ConfigCommand::Set { .. } | ConfigCommand::Edit
        })
    );
    if let Some(path) = &cli.config
        && !path.is_file()
        && !permits_missing
    {
        return Err((
            2,
            format!("Configuration file does not exist: {}", path.display()),
        ));
    }
    let overrides = cli
        .config_json
        .as_deref()
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|e| (2, format!("Invalid --config-json: {e}")))?;
    if overrides.as_ref().is_some_and(|v| !v.is_object()) {
        return Err((2, "--config-json must be a JSON object".into()));
    }
    if let Some(command) = &cli.command {
        return subcommand(cli, command, overrides);
    }
    for (requested, name) in [
        (cli.interactive, "--interactive"),
        (cli.resume, "--resume"),
        (
            cli.llm_batch || cli.llm_batch_timeout.is_some() || cli.llm_batch_collect.is_some(),
            "LLM Batch API",
        ),
        (cli.record_history, "--record-history"),
        (cli.log_level.is_some(), "--log-level"),
    ] {
        if requested {
            return Err(unsupported(name));
        }
    }
    let Some(input) = cli.input.as_deref() else {
        return Err((2, "INPUT is required".into()));
    };
    let mut cfg = config::load(cli.config.as_deref(), overrides).map_err(runtime)?;
    if let Some(name) = &cli.preset {
        let preset = match name.as_str() {
            "minimal" => {
                json!({"llm":false,"alt":false,"desc":false,"ocr":false,"screenshot":false})
            }
            "standard" => json!({"llm":true,"alt":true,"desc":true,"ocr":false,"screenshot":false}),
            "rich" => json!({"llm":true,"alt":true,"desc":true,"ocr":false,"screenshot":true}),
            _ => cfg["presets"]
                .get(name)
                .cloned()
                .ok_or_else(|| (1, format!("Unknown preset: {name}")))?,
        };
        for (key, section, field) in [
            ("llm", "llm", "enabled"),
            ("ocr", "ocr", "enabled"),
            ("alt", "image", "alt_enabled"),
            ("desc", "image", "desc_enabled"),
            ("screenshot", "screenshot", "enabled"),
        ] {
            cfg[section][field] = json!(preset[key].as_bool().unwrap_or(false));
        }
    }
    for (section, field, value) in [
        ("llm", "enabled", tri(cli.llm, cli.no_llm)),
        ("ocr", "enabled", tri(cli.ocr, cli.no_ocr)),
        ("image", "alt_enabled", tri(cli.alt, cli.no_alt)),
        ("image", "desc_enabled", tri(cli.desc, cli.no_desc)),
        ("image", "compress", tri(cli.compress, cli.no_compress)),
        (
            "screenshot",
            "enabled",
            tri(cli.screenshot, cli.no_screenshot),
        ),
        (
            "screenshot",
            "screenshot_only",
            tri(cli.screenshot_only, cli.no_screenshot_only),
        ),
        ("llm", "pure", tri(cli.pure, cli.no_pure)),
    ] {
        if let Some(value) = value {
            cfg[section][field] = json!(value);
        }
    }
    let env = config::environment();
    if !cli.pure
        && !cli.no_pure
        && env
            .get("MARKITAI_PURE")
            .is_some_and(|v| ["1", "true", "yes"].contains(&v.trim()))
    {
        cfg["llm"]["pure"] = json!(true);
    }
    if cli.keep_base {
        cfg["llm"]["keep_base"] = json!(true);
    }
    if cli.screenshot_only {
        cfg["screenshot"]["enabled"] = json!(true);
    }
    if let Some(value) = env
        .get("MARKITAI_RECORD_HISTORY")
        .filter(|v| !v.trim().is_empty())
    {
        cfg["history"]["record"] =
            json!(["1", "true", "yes", "on"].contains(&value.trim().to_lowercase().as_str()));
    }
    if cli.no_record_history {
        cfg["history"]["record"] = json!(false);
    }
    if config::enabled(&cfg, "/history/record") {
        return Err(unsupported("history.record"));
    }
    if config::enabled(&cfg, "/output/report") {
        return Err(unsupported("Persistent conversion reports"));
    }
    if let Some(bypass) = tri(cli.no_cache, cli.cache) {
        cfg["cache"]["no_cache"] = json!(bypass);
    }
    if let Some(patterns) = cli
        .no_cache_for
        .as_ref()
        .filter(|patterns| !patterns.is_empty())
    {
        cfg["cache"]["no_cache_patterns"] = json!(
            patterns
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
        );
    }
    if let Some(profile) = &cli.profile {
        cfg["output"]["profile"] = json!(profile.to_lowercase());
    }
    if let Some(strategy) = &cli.strategy {
        cfg["fetch"]["strategy"] = json!(strategy);
    }
    if cli.no_remote_fetch {
        cfg["fetch"]["remote_consent"] = json!("never");
        cfg["fetch"]["no_remote"] = json!(true);
    }
    if cli.backend.as_deref() == Some("cloudflare") {
        return Err(unsupported("Cloudflare file conversion"));
    }
    for (field, value) in [
        ("concurrency", cli.batch_concurrency),
        ("url_concurrency", cli.url_concurrency),
    ] {
        if let Some(n) = value {
            cfg["batch"][field] = json!(n);
        }
    }
    if let Some(n) = cli.llm_concurrency {
        cfg["llm"]["concurrency"] = json!(n);
    }
    config::validate(&cfg).map_err(runtime)?;
    let input_path = Path::new(input);
    let batch = input_path.is_dir()
        || input_path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("urls"));
    if !input_path.is_dir() && (!cli.globs.is_empty() || cli.max_depth.is_some()) {
        return Err((2, "--glob and --max-depth require a directory input".into()));
    }
    let mut output = cli.output.clone();
    if batch && output.is_none() {
        output = cfg["output"]["dir"].as_str().map(PathBuf::from);
    }
    if batch && output.is_none() {
        return Err((1, "Batch conversion needs -o or output.dir".into()));
    }
    if let Some(path) = &output
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        && !path.is_dir()
    {
        if batch {
            return Err((
                2,
                "A batch requires an output directory, not a .md file".into(),
            ));
        }
        cfg["output"]["filename"] = json!(path.file_name().unwrap_or_default().to_string_lossy());
        output = Some(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_owned(),
        );
    }
    let mut tasks = if input_path.is_dir() {
        discover(input_path, output.as_deref().unwrap(), cli, &cfg)?
    } else if batch {
        parse_urls(input_path, output.as_deref().unwrap())?
    } else {
        vec![Task {
            source: input.into(),
            display: input.into(),
            output,
            filename: None,
            reserved_stem: None,
            source_file: None,
        }]
    };
    if cli.dry_run {
        if !is_url(input) && !input_path.exists() {
            return Err((1, format!("Input does not exist: {input}")));
        }
        for task in &tasks {
            println!(
                "{} -> {}",
                task.display,
                task.output
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "stdout".into())
            );
        }
        return Ok(0);
    }
    if !batch {
        let task = &tasks[0];
        let result = convert_task(task, &cfg);
        let item = outcome(task, &result);
        let failed = result.is_err();
        if cli.json {
            emit_json(&[item], None);
        } else {
            match result {
                Ok(result) => {
                    if task.output.is_none() {
                        print_stdout(&result, &cfg).map_err(runtime)?;
                    }
                    if !cli.quiet {
                        for warning in &result.warnings {
                            eprintln!("Warning: {warning}");
                        }
                    }
                    if cli.verbose
                        && !cli.quiet
                        && let Some(path) = result.llm_output_path.or(result.output_path)
                    {
                        eprintln!("Wrote {}", path.display());
                    }
                }
                Err(error) => eprintln!("Error: {error}"),
            }
        }
        return Ok(if failed { 1 } else { 0 });
    }
    reserve_batch_names(&mut tasks, &cfg)?;
    let tasks = Arc::new(tasks);
    let cursor = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel();
    let concurrency = if input_path.is_dir() {
        cfg["batch"]["concurrency"].as_u64().unwrap_or(10)
    } else {
        cfg["batch"]["url_concurrency"].as_u64().unwrap_or(5)
    } as usize;
    let exit_code = std::thread::scope(|scope| {
        for _ in 0..concurrency.min(tasks.len()) {
            let sender = sender.clone();
            let tasks = &tasks;
            let cfg = &cfg;
            let cursor = &cursor;
            scope.spawn(move || {
                loop {
                    let index = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(task) = tasks.get(index) else {
                        break;
                    };
                    let result = convert_task(task, cfg);
                    let item = outcome(task, &result);
                    if sender.send(item).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let items: Vec<Value> = receiver.into_iter().collect();
        let failed = items.iter().filter(|i| i["status"] == "failed").count();
        if cli.json {
            emit_json(&items, None);
        } else {
            for item in &items {
                if !cli.quiet {
                    for warning in item["warnings"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        eprintln!(
                            "Warning: {}: {warning}",
                            item["source"].as_str().unwrap_or("")
                        );
                    }
                }
                if item["status"] == "failed" {
                    eprintln!(
                        "Error: {}: {}",
                        item["source"].as_str().unwrap_or(""),
                        item["error"].as_str().unwrap_or("")
                    );
                }
            }
            if !cli.quiet {
                eprintln!(
                    "{} items, {} completed, {} failed",
                    items.len(),
                    items.iter().filter(|i| i["status"] == "completed").count(),
                    failed
                );
            }
        }
        if failed > 0 { 10 } else { 0 }
    });
    Ok(exit_code)
}

#[derive(Clone)]
struct Task {
    source: String,
    display: String,
    output: Option<PathBuf>,
    filename: Option<String>,
    reserved_stem: Option<String>,
    source_file: Option<String>,
}
/// Claim document stems before any worker starts. Disk conflicts use the configured
/// policy; another item in this run always gets a separate stem.
fn reserve_batch_names(tasks: &mut [Task], cfg: &Value) -> CliResult<()> {
    use std::collections::{HashMap, HashSet};
    let mut claimed = HashSet::<(PathBuf, String)>::new();
    let mut case_rules = HashMap::<PathBuf, bool>::new();
    let mode = cfg["output"]["on_conflict"].as_str().unwrap_or("rename");
    for task in tasks {
        let Some(directory) = task.output.as_deref() else {
            continue;
        };
        markitai_core::output::check_path(
            directory,
            config::enabled(cfg, "/output/allow_symlinks"),
        )
        .map_err(runtime)?;
        std::fs::create_dir_all(directory).map_err(runtime)?;
        let directory = absolute(directory);
        let folds = *case_rules.entry(directory.clone()).or_insert_with(|| {
            tempfile::Builder::new()
                .prefix(".MarkitaiCaseProbe-")
                .tempfile_in(&directory)
                .map(|probe| {
                    let lower = probe
                        .path()
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_lowercase();
                    directory.join(lower).exists()
                })
                .unwrap_or(true)
        });
        let stem = if let Some(name) = &task.filename {
            name.strip_suffix(".md").unwrap_or(name).to_owned()
        } else if is_url(&task.source) {
            markitai_core::output::url_name(&task.source, &serde_json::Map::new())
        } else {
            Path::new(&task.source)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let key = |stem: &str| {
            (
                directory.clone(),
                if folds {
                    stem.to_lowercase()
                } else {
                    stem.to_owned()
                },
            )
        };
        let occupied = |stem: &str| {
            [".md", ".llm.md"].iter().any(|suffix| {
                std::fs::symlink_metadata(directory.join(format!("{stem}{suffix}"))).is_ok()
            })
        };
        let duplicate = claimed.contains(&key(&stem));
        let mut resolved = stem.clone();
        if !duplicate && mode == "skip" && occupied(&stem) {
            // No new result is claimed when this item will reuse an existing file.
            task.reserved_stem = Some(stem);
            continue;
        }
        if duplicate || (mode == "rename" && occupied(&stem)) {
            let mut version = 2u64;
            loop {
                resolved = format!("{stem}.v{version}");
                if !claimed.contains(&key(&resolved)) && !occupied(&resolved) {
                    break;
                }
                version = version
                    .checked_add(1)
                    .ok_or_else(|| runtime("Output version counter exhausted"))?;
            }
        }
        claimed.insert(key(&resolved));
        task.reserved_stem = Some(resolved);
    }
    Ok(())
}

fn convert_task(task: &Task, cfg: &Value) -> Result<ConversionOutput, String> {
    let mut cfg = cfg.clone();
    if let Some(name) = &task.reserved_stem {
        cfg["output"]["reserved_stem"] = json!(name);
    } else if let Some(name) = &task.filename {
        cfg["output"]["reserved_stem"] = json!(name.strip_suffix(".md").unwrap_or(name));
    }
    markitai_core::convert(
        &task.source,
        ConvertOptions {
            output_dir: task.output.clone(),
            config: Some(cfg),
            ..Default::default()
        },
    )
    .or_else(|error| match error {
        markitai_core::Error::ImageOnly(_) => {
            let mut result = ConversionOutput::default();
            result.source = task.source.clone();
            result.skip_reason = Some("image_only".into());
            Ok(result)
        }
        other => Err(other.to_string()),
    })
}
fn outcome(task: &Task, result: &Result<ConversionOutput, String>) -> Value {
    let mut item = json!({"kind":if is_url(&task.source){"url"}else{"file"},"source":task.display,"status":"failed","output":null,"error":null,"warnings":[],"skip_reason":null,"images":0,"screenshots":0,"cost_usd":0.0,"duration_s":null,"cache_hit":false,"fetch_cache_hit":false,"llm_cache_hit":false,"fetch_strategy":null,"source_file":task.source_file,"llm_usage":{}});
    match result {
        Ok(result) => {
            item["status"] = json!(if result.skip_reason.is_some() {
                "skipped"
            } else {
                "completed"
            });
            item["output"] = json!(
                result
                    .llm_output_path
                    .as_ref()
                    .or(result.output_path.as_ref())
            );
            item["warnings"] = json!(result.warnings);
            item["skip_reason"] = json!(result.skip_reason);
            item["images"] = json!(result.assets.len());
            item["screenshots"] = json!(result.screenshots.len());
            item["cost_usd"] = json!(round(result.usage.cost_usd, 1_000_000.0));
            item["duration_s"] = json!(round(result.duration, 1000.0));
            item["llm_usage"] = json!(result.usage.by_model);
            item["llm_cache_hit"] = json!(result.llm_cache_hit());
            item["cache_hit"] = json!(result.llm_cache_hit());
            if is_url(&task.source) {
                item["fetch_strategy"] = result
                    .frontmatter
                    .get("fetch_strategy")
                    .cloned()
                    .unwrap_or(Value::Null);
            }
        }
        Err(error) => item["error"] = json!(error),
    }
    item
}
fn round(value: f64, factor: f64) -> f64 {
    (value * factor).round() / factor
}
fn envelope(items: &[Value], error: Option<&str>) -> Value {
    let count = |status: &str| items.iter().filter(|i| i["status"] == status).count();
    json!({"version":"1.0","ok":count("failed")==0 && count("pending")==0 && error.is_none(),"error":error,"batch":null,"items":items,"totals":{"total":items.len(),"completed":count("completed"),"failed":count("failed"),"skipped":count("skipped"),"pending":count("pending"),"cost_usd":round(items.iter().filter_map(|i|i["cost_usd"].as_f64()).sum(),1_000_000.0),"duration_s":round(items.iter().filter_map(|i|i["duration_s"].as_f64()).sum(),1000.0)}})
}
fn emit_json(items: &[Value], error: Option<&str>) {
    println!(
        "{}",
        serde_json::to_string_pretty(&envelope(items, error)).expect("JSON values serialize")
    );
}
fn print_stdout(result: &ConversionOutput, cfg: &Value) -> io::Result<()> {
    if result.skip_reason.is_some() {
        return Ok(());
    }
    let mut stdout = io::stdout().lock();
    let rendered = markitai_core::output::content(result, cfg, result.llm_markdown.is_some())
        .map_err(io::Error::other)?;
    stdout.write_all(rendered.as_bytes())?;
    if !rendered.ends_with('\n') {
        stdout.write_all(b"\n")?;
    }
    Ok(())
}
fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn discover(input: &Path, output: &Path, cli: &Cli, cfg: &Value) -> CliResult<Vec<Task>> {
    use globset::{GlobBuilder, GlobSetBuilder};
    let mut positive = GlobSetBuilder::new();
    let mut negative = GlobSetBuilder::new();
    let mut has_positive = false;
    for pattern in &cli.globs {
        let (exclude, pattern) = pattern
            .strip_prefix('!')
            .map(|p| (true, p))
            .unwrap_or((false, pattern.as_str()));
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| (2, format!("Invalid glob: {e}")))?;
        if exclude {
            negative.add(glob);
        } else {
            positive.add(glob);
            has_positive = true;
        }
    }
    let positive = positive.build().map_err(runtime)?;
    let negative = negative.build().map_err(runtime)?;
    let depth = cli
        .max_depth
        .or_else(|| cfg["batch"]["scan_max_depth"].as_u64().map(|n| n as usize))
        .unwrap_or(5);
    let max_files = cfg["batch"]["scan_max_files"].as_u64().unwrap_or(10000) as usize;
    let absolute_output = absolute(output);
    let absolute_input = absolute(input);
    let follow = config::enabled(cfg, "/output/allow_symlinks");
    let mut tasks = Vec::new();
    let entries = walkdir::WalkDir::new(input)
        .follow_links(follow)
        .max_depth(depth.saturating_add(1))
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            if entry.file_name() == ".markitai" {
                return false;
            }
            !(entry.file_type().is_dir()
                && absolute_output != absolute_input
                && absolute(entry.path()) == absolute_output)
        });
    for entry in entries {
        let entry = entry.map_err(runtime)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(input).map_err(runtime)?;
        if (has_positive && !positive.is_match(relative)) || negative.is_match(relative) {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let out = output.join(relative.parent().unwrap_or(Path::new("")));
        if ext == "urls" {
            tasks.extend(parse_urls(path, &out)?);
        } else if markitai_core::formats::supports_extension(&ext)
            || markitai_core::is_image_extension(&ext)
        {
            tasks.push(Task {
                source: path.to_string_lossy().into_owned(),
                display: relative.to_string_lossy().replace('\\', "/"),
                output: Some(out),
                filename: None,
                reserved_stem: None,
                source_file: None,
            });
        }
        if tasks.len() > max_files {
            return Err((
                1,
                format!(
                    "Batch exceeds scan_max_files ({max_files}); narrow --glob or increase the configuration limit"
                ),
            ));
        }
    }
    tasks.sort_by(|a, b| a.display.cmp(&b.display));
    Ok(tasks)
}
fn absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    })
}
fn parse_urls(path: &Path, output: &Path) -> CliResult<Vec<Task>> {
    let raw = std::fs::read_to_string(path).map_err(runtime)?;
    let raw = raw.trim_start_matches('\u{feff}').trim();
    let mut entries = Vec::new();
    if raw.starts_with('[') {
        let values: Vec<Value> = serde_json::from_str(raw).map_err(runtime)?;
        for value in values {
            let pair = if let Some(url) = value.as_str() {
                Some((url.to_string(), None))
            } else {
                value["url"].as_str().map(|url| {
                    (
                        url.to_string(),
                        value["output_name"].as_str().map(String::from),
                    )
                })
            };
            if let Some(pair) = pair {
                entries.push(pair);
            }
        }
    } else {
        for line in raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let (url, name) = line
                .split_once(char::is_whitespace)
                .map(|(u, n)| (u, Some(n.trim().trim_matches(['\'', '"']).to_string())))
                .unwrap_or((line, None));
            entries.push((url.into(), name));
        }
    }
    let mut tasks = Vec::new();
    for (url, name) in entries {
        let url = url.trim();
        if !is_url(url) {
            eprintln!("Warning: skipping invalid URL entry in {}", path.display());
            continue;
        }
        let filename = match name.filter(|n| !n.is_empty()) {
            Some(name) => {
                if name.contains(['/', '\\']) || name == "." || name == ".." {
                    return Err((
                        1,
                        "URL output_name must be a filename without directory components".into(),
                    ));
                }
                Some(if name.ends_with(".md") {
                    name
                } else {
                    format!("{name}.md")
                })
            }
            None => None,
        };
        if tasks
            .iter()
            .any(|t: &Task| t.source == url && t.filename == filename)
        {
            continue;
        }
        tasks.push(Task {
            source: url.into(),
            display: url.into(),
            output: Some(output.to_owned()),
            filename,
            reserved_stem: None,
            source_file: Some(path.to_string_lossy().into_owned()),
        });
    }
    Ok(tasks)
}

fn selected_config(cli: &Cli) -> Option<PathBuf> {
    config::selected_path(cli.config.as_deref())
}
fn subcommand(cli: &Cli, command: &Command, overrides: Option<Value>) -> CliResult<i32> {
    match command {
        Command::Config { command } => match command {
            ConfigCommand::Path => {
                if let Some(path) = selected_config(cli) {
                    println!("{}", path.display());
                } else {
                    println!("No configuration file found; using built-in defaults");
                }
            }
            ConfigCommand::Validate { config_file } => {
                if let Some(path) = config_file
                    && !path.exists()
                {
                    return Err((
                        2,
                        format!("Configuration file does not exist: {}", path.display()),
                    ));
                }
                config::load(config_file.as_deref().or(cli.config.as_deref()), overrides)
                    .map_err(runtime)?;
                println!("Configuration is valid");
            }
            ConfigCommand::List {
                format,
                show_secrets,
            } => {
                let cfg = config::load(cli.config.as_deref(), overrides).map_err(runtime)?;
                let mut cfg = config::display_value(&cfg, None).map_err(runtime)?;
                if !show_secrets {
                    redact(&mut cfg);
                }
                match format.to_lowercase().as_str() {
                    "yaml" => print!("{}", serde_yaml::to_string(&cfg).map_err(runtime)?),
                    "table" => print_table("", &cfg),
                    _ => println!("{}", serde_json::to_string_pretty(&cfg).map_err(runtime)?),
                }
            }
            ConfigCommand::Get { key, show_secrets } => {
                let cfg = config::load(cli.config.as_deref(), overrides).map_err(runtime)?;
                let pointer = key_pointer(key)?;
                let value = cfg
                    .pointer(&pointer)
                    .ok_or_else(|| (1, format!("Unknown configuration key: {key}")))?;
                if value.is_null() {
                    println!("null");
                    return Ok(0);
                }
                let value = config::display_value(&cfg, Some(key)).map_err(runtime)?;
                let visible = if *show_secrets {
                    value
                } else {
                    config::redact_for_key(key, &value)
                };
                let value = &visible;
                match value {
                    Value::String(s) => println!("{s}"),
                    _ => println!("{}", serde_json::to_string_pretty(value).map_err(runtime)?),
                }
            }
            ConfigCommand::Set {
                key,
                value,
                show_secrets,
            } => {
                if overrides.is_some() {
                    return Err((2, "--config-json overrides cannot be saved; drop --config-json to write a config file".into()));
                }
                let path =
                    selected_config(cli).unwrap_or_else(|| config::home().join("config.json"));
                let mut raw = if path.is_file() {
                    serde_json::from_slice::<Value>(&std::fs::read(&path).map_err(runtime)?)
                        .map_err(runtime)?
                } else {
                    json!({})
                };
                if !raw.is_object() {
                    return Err((1, "Configuration must be a JSON object".into()));
                }
                let value = config::parse_cli_value(&raw, key, value).map_err(runtime)?;
                let value = config::set_value(&mut raw, key, value).map_err(runtime)?;
                write_config(&path, &raw)?;
                let mut visible = value;
                if !show_secrets {
                    visible = config::redact_for_key(key, &visible);
                }
                println!("{key} = {visible}");
            }
            ConfigCommand::Edit => {
                if overrides.is_some() {
                    return Err((2, "--config-json overrides cannot be saved; drop --config-json to edit a config file".into()));
                }
                return Err(unsupported("Interactive configuration editor"));
            }
        },
        Command::Init { yes, output, local } => {
            if !yes {
                return Err(unsupported(
                    "Interactive init; use init --yes for a minimal config",
                ));
            }
            let path = output.clone().unwrap_or_else(|| {
                if *local {
                    PathBuf::from("markitai.json")
                } else {
                    config::home().join("config.json")
                }
            });
            if path.exists() {
                return Err((
                    1,
                    format!("Configuration already exists: {}", path.display()),
                ));
            }
            write_config(
                &path,
                &json!({"output":{"dir":"./output"},"llm":{"enabled":false}}),
            )?;
            println!("Created {}", path.display());
        }
        Command::Doctor {
            json: as_json,
            fix,
            suggest_extras,
        } => {
            if *fix {
                return Err(unsupported("Automatic runtime repairs"));
            }
            if *suggest_extras {
                return Err(unsupported(
                    "Python extras recommendations (this build is native Rust)",
                ));
            }
            config::load(cli.config.as_deref(), overrides).map_err(runtime)?;
            let diagnostic = json!({"version":markitai_core::VERSION,"runtime":"rust","configuration":"valid","capabilities":{"local_conversion":true,"static_fetch":true,"openai_compatible_llm":true,"ocr":false,"screenshots":false,"browser":false,"cache":true,"serve":false,"mcp":false},"status":"development"});
            if *as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&diagnostic).map_err(runtime)?
                );
            } else {
                println!(
                    "Markitai {} — native Rust runtime\nConfiguration: valid\nAvailable: local conversion, static URL fetch, OpenAI-compatible LLM, persistent document LLM cache\nNot available: OCR, screenshots, browser, fetch cache, serve, MCP",
                    markitai_core::VERSION
                );
            }
        }
        Command::Cache { command } => {
            let cfg = config::load(cli.config.as_deref(), overrides).map_err(runtime)?;
            return cache_command(command, &cfg);
        }
        Command::Auth { .. } => return Err(unsupported("Provider authentication commands")),
        Command::Serve { .. } => return Err(unsupported("Web server")),
        Command::Mcp => return Err(unsupported("MCP server")),
    }
    Ok(0)
}
fn cache_command(command: &CacheCommand, cfg: &Value) -> CliResult<i32> {
    match command {
        CacheCommand::Stats {
            json: as_json,
            verbose,
            limit,
        } => {
            let stats = markitai_core::llm_cache::stats(cfg, *verbose, *limit).map_err(runtime)?;
            let failed = ["cache", "fetch_cache"]
                .iter()
                .any(|name| stats[name].get("error").is_some());
            if *as_json {
                println!("{}", serde_json::to_string_pretty(&stats).map_err(runtime)?);
            } else {
                println!("Cache enabled: {}", stats["enabled"]);
                if let Some(error) = stats["cache"].get("error") {
                    println!("LLM cache: {}", error.as_str().unwrap_or("unavailable"));
                } else {
                    println!(
                        "LLM cache: {} entries ({} bytes)",
                        stats["cache"]["count"].as_u64().unwrap_or(0),
                        stats["cache"]["size_bytes"].as_u64().unwrap_or(0)
                    );
                    if *verbose && !stats["cache"].is_null() {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&stats["cache"]).map_err(runtime)?
                        );
                    }
                }
                if let Some(error) = stats["fetch_cache"].get("error") {
                    println!(
                        "URL fetch cache: {}",
                        error.as_str().unwrap_or("unavailable")
                    );
                }
            }
            Ok(i32::from(failed))
        }
        CacheCommand::Clear {
            yes,
            include_spa_domains,
        } => {
            let dir = config::state_path(Path::new(
                cfg["cache"]["global_dir"].as_str().unwrap_or("~/.markitai"),
            ));
            // Check unsupported stores before changing any cache. A partially
            // completed clear must not be reported as a complete clear.
            if *include_spa_domains {
                return Err(unsupported("Learned browser-domain cache management"));
            }
            if dir.join("fetch_cache.db").try_exists().map_err(runtime)? {
                return Err(unsupported("Existing URL fetch cache management"));
            }
            if !yes {
                print!("Clear LLM + URL fetch caches ({})? [y/N]: ", dir.display());
                io::stdout().flush().map_err(runtime)?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer).map_err(runtime)?;
                if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    println!("Aborted");
                    return Ok(0);
                }
            }
            let count = markitai_core::llm_cache::clear(cfg).map_err(runtime)?;
            println!("Cleared {count} cache entries");
            Ok(0)
        }
        CacheCommand::SpaDomains { .. } => {
            Err(unsupported("Learned browser-domain cache management"))
        }
    }
}
fn key_pointer(key: &str) -> CliResult<String> {
    config::key_pointer(key).map_err(runtime)
}
fn redact(value: &mut Value) {
    *value = config::redact(value);
}
fn print_table(prefix: &str, value: &Value) {
    if let Value::Object(map) = value {
        for (key, value) in map {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            print_table(&path, value);
        }
    } else {
        println!("{prefix}\t{value}");
    }
}
fn write_config(path: &Path, value: &Value) -> CliResult<()> {
    // Configuration paths are user-selected; preserve an existing symlink and
    // atomically update its target, as the Python configuration manager did.
    let resolved;
    let path = if path.is_symlink() {
        resolved = std::fs::canonicalize(path).map_err(runtime)?;
        &resolved
    } else {
        path
    };
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(runtime)?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(runtime)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(runtime)?;
    writeln!(file).map_err(runtime)?;
    file.as_file().sync_all().map_err(runtime)?;
    file.persist(path).map_err(runtime)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn envelope_keeps_failed_items_and_totals() {
        let items = vec![
            json!({"status":"completed","cost_usd":0.1234567,"duration_s":0.101}),
            json!({"status":"failed","cost_usd":0.0,"duration_s":null}),
        ];
        let result = envelope(&items, None);
        assert_eq!(result["version"], "1.0");
        assert_eq!(result["ok"], false);
        assert_eq!(result["totals"]["failed"], 1);
        assert_eq!(result["totals"]["cost_usd"], 0.123457);
    }
    #[test]
    fn credentials_remain_redacted_at_nested_paths() {
        let mut cfg = json!({"llm":{"model_list":[{"litellm_params":{"api_key":"secret","model":"test"}}]},"fetch":{"playwright":{"extra_http_headers":{"Authorization":"secret"}}}});
        redact(&mut cfg);
        assert_eq!(
            cfg["llm"]["model_list"][0]["litellm_params"]["api_key"],
            "[REDACTED]"
        );
        assert_eq!(
            cfg["fetch"]["playwright"]["extra_http_headers"]["Authorization"],
            "[REDACTED]"
        );
        assert_eq!(
            key_pointer("llm.model_list[0].model_name").unwrap(),
            "/llm/model_list/0/model_name"
        );
    }
    #[test]
    fn reservations_rename_same_run_collisions_under_skip_and_overwrite() {
        for mode in ["skip", "overwrite"] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("shared.v2.llm.md"), "existing").unwrap();
            let mut tasks: Vec<_> = ["one", "two"]
                .iter()
                .map(|id| Task {
                    source: format!("https://example.com/page?id={id}"),
                    display: id.to_string(),
                    output: Some(dir.path().to_owned()),
                    filename: Some("shared.md".into()),
                    reserved_stem: None,
                    source_file: None,
                })
                .collect();
            let mut cfg = config::defaults();
            cfg["output"]["on_conflict"] = json!(mode);
            reserve_batch_names(&mut tasks, &cfg).unwrap();
            assert_eq!(tasks[0].reserved_stem.as_deref(), Some("shared"));
            assert_eq!(tasks[1].reserved_stem.as_deref(), Some("shared.v3"));
            assert_eq!(
                std::fs::read_to_string(dir.path().join("shared.v2.llm.md")).unwrap(),
                "existing"
            );
            assert!(!dir.path().join("shared.md").exists());
        }
    }
}
