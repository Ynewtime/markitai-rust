//! Authentication stays with the official runtime; no token store is inspected here.
use super::{CliResult, runtime};
use clap::Subcommand;
use markitai_core::{config, subscription};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Subcommand, Debug, Clone)]
pub(super) enum Command {
    /// GitHub Copilot through the installed official CLI.
    Copilot {
        #[command(subcommand)]
        command: Option<Action>,
    },
    /// Claude subscription adapter status.
    Claude {
        #[command(subcommand)]
        command: Option<Action>,
    },
    /// ChatGPT subscription adapter status.
    Chatgpt {
        #[command(subcommand)]
        command: Option<Action>,
    },
}
#[derive(Subcommand, Debug, Clone)]
pub(super) enum Action {
    /// Inspect existing authentication without initiating login.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Open the official runtime's interactive login flow.
    Login,
}

pub(super) fn copilot_status(env: &HashMap<String, String>) -> subscription::AuthStatus {
    let result = subscription::CopilotConfig::from_env(env).and_then(|cfg| {
        subscription::status(&cfg, Duration::from_secs(15)).map_err(|failure| failure.error)
    });
    result.unwrap_or_else(|error| subscription::AuthStatus {
        provider: "copilot",
        authenticated: false,
        user: None,
        expires_at: None,
        error: Some(error.to_string()),
        details: serde_json::json!({"source":"official_cli","verification":"unavailable"}),
    })
}
fn unavailable(provider: &'static str) -> subscription::AuthStatus {
    subscription::AuthStatus {
        provider,
        authenticated: false,
        user: None,
        expires_at: None,
        error: Some("This subscription adapter is not implemented in the native runtime".into()),
        details: serde_json::json!({"verification":"unsupported"}),
    }
}
fn display(status: &subscription::AuthStatus) {
    println!(
        "{}: {}",
        status.provider,
        if status.authenticated {
            "authenticated"
        } else {
            "unavailable"
        }
    );
    if let Some(user) = &status.user {
        println!("  Account: {}", terminal_text(user));
    }
    if let Some(error) = &status.error {
        println!("  {}", terminal_text(error));
    }
}
fn terminal_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(2048)
        .collect()
}

pub(super) fn run(command: Option<&Command>) -> CliResult<i32> {
    let env = config::environment();
    let Some(command) = command else {
        display(&unavailable("claude-agent"));
        display(&unavailable("chatgpt"));
        display(&copilot_status(&env));
        return Ok(0);
    };
    let (provider, action) = match command {
        Command::Copilot { command } => ("copilot", command),
        Command::Claude { command } => ("claude-agent", command),
        Command::Chatgpt { command } => ("chatgpt", command),
    };
    if matches!(action, Some(Action::Login)) {
        if provider != "copilot" {
            return Err(runtime(
                "This subscription login adapter is not implemented",
            ));
        }
        return login(&env);
    }
    let status = if provider == "copilot" {
        copilot_status(&env)
    } else {
        unavailable(provider)
    };
    if matches!(action, Some(Action::Status { json: true })) {
        let mut value = serde_json::to_value(status).map_err(runtime)?;
        if provider == "claude-agent" {
            value["cli_path"] = serde_json::Value::Null;
            value["sdk_installed"] = serde_json::json!(false);
        }
        println!("{}", serde_json::to_string_pretty(&value).map_err(runtime)?);
        // The reference's JSON status exits zero even for unavailable authentication.
        Ok(0)
    } else {
        display(&status);
        Ok(if status.authenticated { 0 } else { 1 })
    }
}

#[cfg(unix)]
fn login(env: &HashMap<String, String>) -> CliResult<i32> {
    use std::os::unix::process::CommandExt;
    let cfg = subscription::CopilotConfig::from_env(env).map_err(runtime)?;
    let mut command = std::process::Command::new(cfg.executable());
    command.arg("login").env_clear();
    for key in [
        "HOME",
        "PATH",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "TERM",
        "COLORTERM",
        "COPILOT_HOME",
        "COPILOT_GITHUB_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ] {
        if let Some(value) = env.get(key).cloned().or_else(|| std::env::var(key).ok()) {
            command.env(key, value);
        }
    }
    // Replacing this process preserves terminal ownership, signals, and the exact
    // official login exit status without adding an unbounded intermediary waiter.
    let _error = command.exec();
    Err(runtime(
        "The official Copilot login process could not be started",
    ))
}
#[cfg(not(unix))]
fn login(_env: &HashMap<String, String>) -> CliResult<i32> {
    Err(runtime(
        "Copilot interactive login delegation is not supported on this platform",
    ))
}
