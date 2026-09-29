#![cfg(unix)]
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

struct Fixture {
    root: tempfile::TempDir,
    executable: std::path::PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix("auth fixture ")
            .tempdir()
            .unwrap();
        let executable = root.path().join("official fixture");
        let text = include_str!("../../markitai-core/src/subscription/fake_cli.py");
        let text = text.replace(
            "root = pathlib.Path(os.environ['COPILOT_HOME'])",
            r#"root = pathlib.Path(os.environ['COPILOT_HOME'])
if sys.argv[1:] == ['login']:
    assert 'OPENAI_API_KEY' not in os.environ
    assert 'COPILOT_PROVIDER_API_KEY' not in os.environ
    state = json.loads((root / 'fixture.json').read_text())
    assert os.environ.get('HOME') == state['home']
    (root / 'login.json').write_text(json.dumps({'pid':os.getpid(),'args':sys.argv[1:]}))
    sys.exit(state.get('login_exit', 0))"#,
        );
        std::fs::write(&executable, text).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Self { root, executable };
        fixture.state(mode, 0);
        std::fs::write(fixture.root.path().join("config.json"), b"{}\n").unwrap();
        fixture
    }
    fn state(&self, mode: &str, exit: i32) {
        std::fs::write(
            self.root.path().join("fixture.json"),
            serde_json::to_vec(&json!({
                "mode":mode,"home":std::env::var("HOME").ok(),"login_exit":exit
            }))
            .unwrap(),
        )
        .unwrap();
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_markitai"));
        command.env_clear().current_dir(self.root.path());
        for key in ["HOME", "PATH", "TMPDIR", "LANG"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("MARKITAI_HOME", self.root.path().join("state"))
            .env("COPILOT_CLI_PATH", &self.executable)
            .env("COPILOT_HOME", self.root.path())
            .env("COPILOT_GITHUB_TOKEN", "fixture-only")
            .env("OPENAI_API_KEY", "must-be-scrubbed")
            .env("COPILOT_PROVIDER_API_KEY", "must-be-scrubbed")
            .args(["-c", "config.json"]);
        command
    }
    fn status(&self, json: bool) -> std::process::Output {
        let mut command = self.command();
        command.args(["auth", "copilot", "status"]);
        if json {
            command.arg("--json");
        }
        command.output().unwrap()
    }
}

#[test]
fn observed_status_preserves_public_shape_and_json_false_exit() {
    let fixture = Fixture::new("normal");
    let result = fixture.status(true);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["provider"], "copilot");
    assert_eq!(value["authenticated"], true);
    assert_eq!(value["user"], "fixture-user");
    assert!(value["expires_at"].is_null());
    assert!(value["error"].is_null());
    assert_eq!(value["details"]["protocol_version"], 3);
    fixture.state("unauth", 0);
    let result = fixture.status(true);
    assert!(result.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["authenticated"],
        false
    );
    assert_eq!(fixture.status(false).status.code(), Some(1));
    let requests = std::fs::read_to_string(fixture.root.path().join("requests.jsonl")).unwrap();
    assert!(!requests.contains("session.send"));
    assert!(!fixture.root.path().join("login.json").exists());
}

#[test]
fn unavailable_runtime_and_other_adapters_remain_explicit() {
    let fixture = Fixture::new("normal");
    let result = fixture
        .command()
        .env("COPILOT_CLI_PATH", fixture.root.path().join("missing"))
        .args(["auth", "copilot", "status", "--json"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["authenticated"], false);
    assert!(value["error"].is_string());
    for provider in ["claude", "chatgpt"] {
        let result = fixture
            .command()
            .args(["auth", provider, "status", "--json"])
            .output()
            .unwrap();
        assert!(result.status.success());
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["details"]["verification"], "unsupported");
        if provider == "claude" {
            assert_eq!(value["sdk_installed"], false);
            assert!(value["cli_path"].is_null());
        }
    }
    let result = fixture.command().arg("auth").output().unwrap();
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.find("claude-agent:").unwrap() < text.find("chatgpt:").unwrap());
    assert!(text.find("chatgpt:").unwrap() < text.find("copilot:").unwrap());
}

#[test]
fn explicit_login_replaces_process_and_preserves_exit_home_and_configuration() {
    let fixture = Fixture::new("normal");
    for code in [0, 7] {
        fixture.state("normal", code);
        let child = fixture
            .command()
            .args(["auth", "copilot", "login"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let result = child.wait_with_output().unwrap();
        assert_eq!(result.status.code(), Some(code));
        let login: Value =
            serde_json::from_slice(&std::fs::read(fixture.root.path().join("login.json")).unwrap())
                .unwrap();
        assert_eq!(login["pid"], pid);
        assert_eq!(login["args"], json!(["login"]));
        assert_eq!(
            std::fs::read(fixture.root.path().join("config.json")).unwrap(),
            b"{}\n"
        );
        assert!(result.stdout.is_empty());
    }
}
