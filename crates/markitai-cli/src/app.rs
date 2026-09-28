#[path = "app/interactive.rs"]
mod interactive;
#[path = "app/logging.rs"]
mod logging;
macro_rules! eprintln {
    ($($argument:tt)*) => { $crate::app::logging::diagnostic(format_args!($($argument)*)) };
}
#[cfg_attr(unix, path = "batch_run.rs")]
#[cfg_attr(not(unix), path = "batch_run_portable.rs")]
mod batch_run;
#[cfg(all(test, unix))]
#[path = "batch_run_portable.rs"]
mod batch_run_portable_tests;
use crate::report::{
    self, ItemKind, ItemStatus, ReportOptions, RunFinished, RunInfo, RunItem, RunMode,
};
use chrono::{Local, SecondsFormat};
use clap::{ArgAction, CommandFactory, Parser, Subcommand};
use markitai_core::{ConversionOutput, ConversionUsage, ConvertContext, ConvertOptions, config};
use serde_json::{Value, json};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

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
    /// Run the native REST conversion service.
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 3600)]
        port: u16,
        #[arg(long)]
        no_open: bool,
        #[arg(long)]
        no_auth: bool,
        #[arg(long, action = ArgAction::Append, value_name = "HOSTNAME")]
        allowed_host: Vec<String>,
    },
    /// Run the native MCP service over standard input/output.
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
    let code = match execute(&cli) {
        Ok(code) => code,
        Err((code, message)) => {
            eprintln!("Error: {message}");
            if cli.json && code != 2 {
                emit_json(&[], Some(&message));
            }
            code
        }
    };
    if let Err(error) = logging::finish(code) {
        eprintln!("Error: {error}");
        return if code == 0 { 1 } else { code };
    }
    code
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
        (
            cli.llm_batch || cli.llm_batch_timeout.is_some() || cli.llm_batch_collect.is_some(),
            "LLM Batch API",
        ),
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
    if let Some(enabled) = tri(cli.record_history, cli.no_record_history) {
        cfg["history"]["record"] = json!(enabled);
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
    logging::start(&cfg, cli.log_level.as_deref()).map_err(runtime)?;
    logging::event(
        logging::Level::Debug,
        "Configuration loaded; native CLI conversion starting",
    );
    let input_path = Path::new(input);
    let directory = !is_url(input) && input_path.is_dir();
    let batch = !is_url(input)
        && (directory
            || input_path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("urls")));
    if cli.resume && !batch {
        return Err(unsupported("--resume for a single file or URL"));
    }
    let mode = if directory {
        RunMode::Directory
    } else if batch {
        RunMode::UrlList
    } else if is_url(input) {
        RunMode::SingleUrl
    } else {
        RunMode::SingleFile
    };
    if !directory && (!cli.globs.is_empty() || cli.max_depth.is_some()) {
        return Err((2, "--glob and --max-depth require a directory input".into()));
    }
    let mut output = cli.output.clone();
    if is_url(input)
        && config::enabled(&cfg, "/screenshot/screenshot_only")
        && !config::enabled(&cfg, "/llm/enabled")
        && output.is_none()
    {
        output = Some(
            cfg["output"]["dir"]
                .as_str()
                .map(PathBuf::from)
                .unwrap_or(std::env::current_dir().map_err(runtime)?),
        );
    }
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
    let mut tasks = if directory {
        discover(input_path, output.as_deref().unwrap(), cli, &cfg)?
    } else if batch {
        parse_urls(input_path, output.as_deref().unwrap())?
    } else {
        vec![Task {
            source: input.into(),
            display: input.into(),
            report_key: if is_url(input) {
                input.into()
            } else {
                input_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            },
            output: output.clone(),
            filename: None,
            reserved_stem: None,
            source_file: None,
        }]
    };
    if mode == RunMode::UrlList && tasks.is_empty() {
        return Err((
            1,
            format!("No valid URLs found in {}.", input_path.display()),
        ));
    }
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
    if tasks.is_empty() && !cli.resume {
        if cli.json {
            emit_json(&[], None);
        }
        return Ok(0);
    }
    let run_clock = Instant::now();
    let started_at = timestamp();
    let history_plan = crate::history::Plan::new(
        &cfg,
        output.as_deref(),
        mode,
        cli.preset.as_deref(),
        &started_at,
        !cli.quiet && !cli.json,
    );
    let report_plan = if let Some(output_dir) = output.clone() {
        report::plan(
            RunInfo {
                mode,
                input: input.into(),
                output_dir,
                started_at,
                log_file: logging::path(),
                options: ReportOptions::from_config(&cfg, cli.max_depth, &cli.globs),
            },
            cfg["output"]["report"].as_bool(),
            cfg["output"]["on_conflict"].as_str().unwrap_or("rename"),
            config::enabled(&cfg, "/output/allow_symlinks"),
        )
        .map_err(runtime)?
    } else {
        None
    };
    let llm_runtime = if config::enabled(&cfg, "/llm/enabled") {
        Some(
            markitai_core::LlmRuntime::new(
                usize::try_from(cfg["llm"]["concurrency"].as_u64().unwrap_or(1))
                    .map_err(runtime)?,
            )
            .map_err(runtime)?,
        )
    } else {
        None
    };
    let context = ConvertContext {
        explicit_fetch_strategy: cli
            .strategy
            .as_deref()
            .filter(|strategy| *strategy != "auto"),
        llm_runtime: llm_runtime.as_ref(),
    };
    if !batch {
        let mut task = tasks.remove(0);
        let claim =
            batch_run::claim(&mut task, &cfg, None, None, &Default::default()).map_err(runtime)?;
        let (record, result) = convert_item(
            &task,
            0,
            &cfg,
            context,
            claim
                .as_ref()
                .map(|claim| claim as &dyn markitai_core::output::Publication),
        );
        let item = outcome(&record);
        let failed = result.is_err();
        let report_error = if record.status == ItemStatus::Completed {
            finish_report(
                report_plan.as_ref(),
                std::slice::from_ref(&record),
                run_clock,
                cli.verbose && !cli.quiet,
            )
            .err()
        } else {
            None
        };
        if let Some(error) = &report_error {
            eprintln!("Error: {error}");
        }
        if let Some(plan) = &history_plan {
            plan.record(std::slice::from_ref(&record));
        }
        if cli.json {
            emit_json(&[item], report_error.as_deref());
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
        return Ok(if failed || report_error.is_some() {
            1
        } else {
            0
        });
    }
    batch_run::run(
        cli,
        &cfg,
        tasks,
        BatchDestination {
            mode,
            output: output.as_deref().unwrap(),
            history: history_plan.as_ref(),
        },
        report_plan.as_ref(),
        run_clock,
        context,
    )
}

struct BatchDestination<'a> {
    mode: RunMode,
    output: &'a Path,
    history: Option<&'a crate::history::Plan>,
}

#[derive(Clone)]
struct Task {
    source: String,
    display: String,
    report_key: String,
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
    let mut next_versions = HashMap::<(PathBuf, String), u64>::new();
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
        let directory = crate::report_store::resolve_path(directory).map_err(runtime)?;
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
        let members = |stem: &str| [key(&format!("{stem}.md")), key(&format!("{stem}.llm.md"))];
        let duplicate = members(&stem).iter().any(|member| claimed.contains(member));
        let mut resolved = stem.clone();
        if !duplicate && mode == "skip" && occupied(&stem) {
            // No new result is claimed when this item will reuse an existing file.
            task.reserved_stem = Some(stem);
            continue;
        }
        if duplicate || (mode == "rename" && occupied(&stem)) {
            let mut version = *next_versions.entry(key(&stem)).or_insert(2);
            loop {
                resolved = format!("{stem}.v{version}");
                if !members(&resolved)
                    .iter()
                    .any(|member| claimed.contains(member))
                    && !occupied(&resolved)
                {
                    next_versions.insert(key(&stem), version.saturating_add(1));
                    break;
                }
                version = version
                    .checked_add(1)
                    .ok_or_else(|| runtime("Output version counter exhausted"))?;
            }
        }
        claimed.extend(members(&resolved));
        task.reserved_stem = Some(resolved);
    }
    Ok(())
}

fn convert_task(
    task: &Task,
    cfg: &Value,
    context: ConvertContext<'_>,
    publication: Option<&dyn markitai_core::output::Publication>,
) -> Result<ConversionOutput, String> {
    let mut cfg = cfg.clone();
    if let Some(name) = &task.reserved_stem {
        cfg["output"]["reserved_stem"] = json!(name);
        if cfg["output"]["filename"].is_string() {
            cfg["output"]["filename"] = json!(format!("{name}.md"));
        }
    } else if let Some(name) = &task.filename {
        cfg["output"]["reserved_stem"] = json!(name.strip_suffix(".md").unwrap_or(name));
    }
    markitai_core::convert_with_publication(
        &task.source,
        ConvertOptions {
            output_dir: task.output.clone(),
            config: Some(cfg),
            ..Default::default()
        },
        context,
        publication,
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
fn timestamp() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Micros, false)
}

fn convert_item(
    task: &Task,
    index: usize,
    cfg: &Value,
    context: ConvertContext<'_>,
    publication: Option<&dyn markitai_core::output::Publication>,
) -> (RunItem, Result<ConversionOutput, String>) {
    let clock = Instant::now();
    let started_at = timestamp();
    let history_enabled = task.output.is_some() && config::enabled(cfg, "/history/record");
    let history_eligible = history_enabled && crate::history::eligible(&task.source);
    logging::event(logging::Level::Info, format!("Converting {}", task.display));
    let result = convert_task(task, cfg, context, publication);
    let mut record = recorded(task, index, clock, started_at, &result);
    record.history_eligible = history_eligible;
    match &result {
        Ok(output) => {
            logging::event(
                logging::Level::Info,
                format!(
                    "{} {}",
                    if output.skip_reason.is_some() {
                        "Skipped"
                    } else {
                        "Completed"
                    },
                    task.display
                ),
            );
            for warning in &output.warnings {
                logging::event(
                    logging::Level::Warning,
                    format!("{}: {warning}", task.display),
                );
            }
        }
        Err(error) => logging::event(logging::Level::Error, format!("{}: {error}", task.display)),
    }
    if history_enabled && record.skip_reason.as_deref() == Some("exists") {
        record.history_output = task.output.as_ref().map(|directory| {
            let fallback = if is_url(&task.source) {
                markitai_core::output::url_name(&task.source, &Default::default())
            } else {
                Path::new(&task.source)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            };
            let stem = cfg["output"]["filename"]
                .as_str()
                .map(|name| name.strip_suffix(".md").unwrap_or(name))
                .or(task.reserved_stem.as_deref())
                .or_else(|| {
                    task.filename
                        .as_deref()
                        .map(|name| name.strip_suffix(".md").unwrap_or(name))
                })
                .unwrap_or(&fallback);
            config::expand_home(directory).join(format!("{stem}.md"))
        });
    }
    (record, result)
}

fn recorded(
    task: &Task,
    index: usize,
    clock: Instant,
    started_at: String,
    result: &Result<ConversionOutput, String>,
) -> RunItem {
    let mut record = RunItem {
        index,
        kind: if is_url(&task.source) {
            ItemKind::Url
        } else {
            ItemKind::File
        },
        display: task.display.clone(),
        report_key: task.report_key.clone(),
        source_file: task.source_file.clone(),
        status: ItemStatus::Failed,
        output: None,
        history_output: None,
        history_eligible: true,
        error: None,
        warnings: Vec::new(),
        skip_reason: None,
        started_at,
        completed_at: timestamp(),
        elapsed_s: clock.elapsed().as_secs_f64(),
        conversion_duration_s: None,
        images: 0,
        screenshots: 0,
        usage: ConversionUsage::default(),
        llm_cache_hit: false,
        fetch_cache_hit: false,
        fetch_strategy: None,
    };
    match &result {
        Ok(output) => {
            record.status = if output.skip_reason.is_some() {
                ItemStatus::Skipped
            } else {
                ItemStatus::Completed
            };
            record.output = output
                .llm_output_path
                .as_ref()
                .or(output.output_path.as_ref())
                .or_else(|| output.screenshots.first())
                .cloned();
            record.warnings = output.warnings.clone();
            record.skip_reason = output.skip_reason.clone();
            record.images = output.assets.len();
            record.screenshots = output.screenshots.len();
            record.conversion_duration_s = Some(output.duration);
            record.usage = ConversionUsage {
                cost_usd: output.usage.cost_usd,
                requests: output.usage.requests,
                input_tokens: output.usage.input_tokens,
                output_tokens: output.usage.output_tokens,
                by_model: output.usage.by_model.clone(),
            };
            record.llm_cache_hit = output.llm_cache_hit();
            record.fetch_cache_hit = output.fetch_cache_hit();
            if record.kind == ItemKind::Url {
                record.fetch_strategy = output.fetch_strategy().map(str::to_owned);
            }
        }
        Err(error) => record.error = Some(error.clone()),
    }
    record
}

fn outcome(item: &RunItem) -> Value {
    json!({
        "kind": if item.kind == ItemKind::Url { "url" } else { "file" },
        "source": item.display,
        "status": match item.status {
            ItemStatus::Completed => "completed",
            ItemStatus::Skipped => "skipped",
            ItemStatus::Failed => "failed",
        },
        "output": item.output,
        "error": item.error,
        "warnings": item.warnings,
        "skip_reason": item.skip_reason,
        "images": item.images,
        "screenshots": item.screenshots,
        "cost_usd": round(item.usage.cost_usd, 1_000_000.0),
        "duration_s": item.conversion_duration_s.map(|duration| round(duration, 1000.0)),
        "cache_hit": item.llm_cache_hit,
        "fetch_cache_hit": item.fetch_cache_hit,
        "llm_cache_hit": item.llm_cache_hit,
        "fetch_strategy": item.fetch_strategy,
        "source_file": item.source_file,
        "llm_usage": item.usage.by_model,
    })
}

fn finish_report(
    plan: Option<&report::ReportPlan>,
    items: &[RunItem],
    clock: Instant,
    show_path: bool,
) -> Result<(), String> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let finished = RunFinished {
        updated_at: timestamp(),
        duration_s: clock.elapsed().as_secs_f64(),
    };
    let bytes = report::render(plan, items, &finished)?;
    match report::publish(plan, &bytes)? {
        crate::report_store::Publication::Written(path) => {
            if show_path {
                eprintln!("Report: {}", path.display());
            }
        }
        crate::report_store::Publication::SkippedExisting(path) => {
            if show_path {
                eprintln!("Existing report preserved: {}", path.display());
            }
        }
    }
    Ok(())
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
    for pattern in cli
        .globs
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty())
    {
        let (exclude, pattern) = pattern
            .strip_prefix('!')
            .map(|p| (true, p))
            .unwrap_or((false, pattern));
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
                report_key: relative.to_string_lossy().replace('\\', "/"),
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
    let mut url_keys = std::collections::HashSet::new();
    tasks.retain(|task| !is_url(&task.source) || url_keys.insert(task.report_key.clone()));
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
        let name = name.filter(|name| !name.is_empty());
        let report_key = name
            .as_ref()
            .map(|name| format!("{url} {name}"))
            .unwrap_or_else(|| url.to_string());
        let filename = match name {
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
        if tasks.iter().any(|t: &Task| t.report_key == report_key) {
            continue;
        }
        tasks.push(Task {
            source: url.into(),
            display: url.into(),
            report_key,
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
                let path =
                    selected_config(cli).unwrap_or_else(|| config::home().join("config.json"));
                interactive::edit(&path)?;
            }
        },
        Command::Init { yes, output, local } => {
            interactive::init(*yes, output.as_deref(), *local)?;
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
            let browser = markitai_core::browser_available();
            let ocr = markitai_core::local_ocr_available();
            let pdf = markitai_core::pdf_raster_available();
            let diagnostic = json!({"version":markitai_core::VERSION,"runtime":"rust","configuration":"valid","capabilities":{"local_conversion":true,"static_fetch":true,"openai_compatible_llm":true,"ocr":ocr,"screenshots":browser || pdf,"browser":browser,"cache":true,"serve":true,"mcp":true},"status":"development"});
            if *as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&diagnostic).map_err(runtime)?
                );
            } else {
                println!(
                    "Markitai {} — native Rust runtime\nConfiguration: valid\nAvailable: local conversion, static URL fetch, OpenAI-compatible LLM, persistent document LLM cache, static HTML/text fetch cache, REST conversion service, stdio MCP\nLocal image OCR: {}\nInstalled browser and URL screenshots: {}\nPDF page screenshots: {}\nPDF page OCR: {}\nOffice screenshots: unavailable",
                    markitai_core::VERSION,
                    if ocr { "available" } else { "unavailable" },
                    if browser { "available" } else { "unavailable" },
                    if pdf { "available" } else { "unavailable" },
                    if pdf && ocr {
                        "available"
                    } else {
                        "unavailable"
                    }
                );
            }
        }
        Command::Cache { command } => {
            let cfg = config::load(cli.config.as_deref(), overrides).map_err(runtime)?;
            return cache_command(command, &cfg);
        }
        Command::Auth { .. } => return Err(unsupported("Provider authentication commands")),
        Command::Serve {
            host,
            port,
            no_open,
            no_auth,
            allowed_host,
        } => {
            let cfg = config::load(cli.config.as_deref(), overrides).map_err(runtime)?;
            crate::server::run(
                cfg,
                crate::server::ServeOptions {
                    host: host.clone(),
                    port: *port,
                    no_open: *no_open,
                    no_auth: *no_auth,
                    allowed_host: allowed_host.clone(),
                },
            )
            .map_err(runtime)?;
        }
        Command::Mcp => crate::mcp::run(cli.config.clone(), overrides).map_err(runtime)?,
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
                } else {
                    println!(
                        "URL fetch cache: {} entries ({} bytes)",
                        stats["fetch_cache"]["count"].as_u64().unwrap_or(0),
                        stats["fetch_cache"]["size_bytes"].as_u64().unwrap_or(0)
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
            markitai_core::llm_cache::preflight_clear(cfg).map_err(runtime)?;
            markitai_core::fetch_cache::preflight_clear(cfg).map_err(runtime)?;
            let llm_count = markitai_core::llm_cache::clear(cfg).map_err(runtime)?;
            let fetch_count = markitai_core::fetch_cache::clear(cfg).map_err(|error| {
                runtime(format!(
                    "LLM cache cleared ({llm_count} entries); URL fetch cache clear failed: {error}"
                ))
            })?;
            println!(
                "Cleared {} cache entries",
                llm_count.saturating_add(fetch_count)
            );
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
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(runtime)?;
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
                    report_key: format!("https://example.com/page?id={id}"),
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
