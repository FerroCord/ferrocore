use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

const USAGE: &str = "Usage: ferrocore check | ferrocore run --config <file> [--] <task> | ferrocore run --config -- <file> <task>";
const KEY_ENV: &str = "FERROCORE_CLI_TEST_API_KEY";
const SECRET: &str = "ferrocore-test-secret-must-not-be-printed";
const CONFIG: &str = r#"active_loop = "rlm"

[llm]
base_url = "https://example.com/v1"
model = "test-model"
api_key_env = "FERROCORE_CLI_TEST_API_KEY"
max_retries = 0
retry_base_ms = 1
retry_cap_ms = 1

[budget]
max_turns = 1
max_total_tokens = 1
max_response_bytes = 1
max_cost_micros_usd = 1
input_cost_per_million_micros = 1
output_cost_per_million_micros = 1

[pty]
startup_timeout_secs = 1
step_timeout_secs = 1
output_view_bytes = 192
scratch_entries = 1

[session]
session_timeout_secs = 1
log_dir = "logs"
"#;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        loop {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("ferrocore-cli-{}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create temporary directory: {error}"),
            }
        }
    }

    fn config(&self, contents: &str) -> PathBuf {
        let path = self.0.join("config.toml");
        fs::write(&path, contents).expect("temporary config should be written");
        path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_cli(args: &[&str], code: i32, stdout: &[&str], stderr: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_ferrocore"))
        .args(args)
        .output()
        .expect("ferrocore process should start");
    assert_eq!(output.status.code(), Some(code));
    let actual_stdout = std::str::from_utf8(&output.stdout).expect("stdout is UTF-8");
    let actual_stderr = std::str::from_utf8(&output.stderr).expect("stderr is UTF-8");
    assert_eq!(actual_stdout.lines().collect::<Vec<_>>(), stdout);
    assert_eq!(actual_stderr.lines().collect::<Vec<_>>(), stderr);
}

fn run_config(path: &std::path::Path, key: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ferrocore"));
    command.args(["run", "--config"]).arg(path).arg("test task");
    match key {
        Some(value) => { command.env(KEY_ENV, value); }
        None => { command.env_remove(KEY_ENV); }
    }
    command.output().expect("ferrocore process should start")
}

fn assert_config_error(output: &Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = std::str::from_utf8(&output.stderr).expect("stderr is UTF-8");
    assert!(stderr.starts_with("configuration error:"), "{stderr}");
    assert!(!stderr.contains(SECRET), "secret leaked in stderr");
}

#[test]
fn check_integrates_ferrocord() {
    assert_cli(&["check"], 0, &["ferrocord integration: ok"], &[]);
}

#[test]
fn usage_without_arguments() {
    assert_cli(&[], 0, &[USAGE], &[]);
}

#[test]
fn usage_with_help() {
    assert_cli(&["--help"], 0, &[USAGE], &[]);
}

#[test]
fn invalid_arguments_are_rejected() {
    for args in [
        vec!["unknown"],
        vec!["check", "extra"],
        vec!["run"],
        vec!["run", "--config"],
        vec!["run", "--config", "config.toml"],
        vec!["run", "--other", "config.toml", "task"],
        vec!["run", "--config", "--unknown", "task"],
        vec!["run", "--config", "--", "config.toml"],
        vec!["run", "--config", "config.toml", "--"],
        vec!["run", "--config", "", "task"],
        vec!["run", "--config", "config.toml", ""],
        vec!["run", "--config", "config.toml", "--unknown"],
        vec!["run", "--config", "config.toml", "task", "extra"],
    ] {
        assert_cli(&args, 2, &[], &[USAGE]);
    }
}

#[test]
fn terminator_allows_hyphenated_config_and_task() {
    let dir = TestDir::new();
    let invalid = CONFIG.replace("active_loop = \"rlm\"", "active_loop = \"react\"");
    fs::write(dir.0.join("-config.toml"), &invalid).expect("temporary config should be written");
    for args in [
        vec!["run", "--config", "--", "-config.toml", "-task"],
        vec!["run", "--config", "--", "-config.toml", "task"],
        vec!["run", "--config", "-config.toml", "-task"],
        vec!["run", "--config", "config.toml", "--", "-task"],
    ] {
        let expected = if args[2] == "-config.toml" { 2 } else { 1 };
        if args[2] == "config.toml" {
            dir.config(&invalid);
        }
        let output = Command::new(env!("CARGO_BIN_EXE_ferrocore"))
            .current_dir(&dir.0)
            .env(KEY_ENV, SECRET)
            .args(&args)
            .output()
            .expect("ferrocore process should start");
        assert_eq!(output.status.code(), Some(expected), "args: {args:?}");
        assert!(output.stdout.is_empty());
        let stderr = std::str::from_utf8(&output.stderr).expect("stderr is UTF-8");
        let message = if expected == 2 { USAGE } else { "configuration error: Unsupported active_loop; expected rlm" };
        assert_eq!(stderr.trim_end(), message, "args: {args:?}");
    }
}

#[test]
fn missing_or_empty_key_is_rejected() {
    let dir = TestDir::new();
    let path = dir.config(CONFIG);
    for key in [None, Some("")] {
        assert_config_error(&run_config(&path, key));
    }
}

#[test]
fn missing_malformed_and_unsupported_config_are_rejected() {
    let dir = TestDir::new();
    assert_config_error(&run_config(&dir.0.join("missing.toml"), Some(SECRET)));
    for config in ["not valid = [", "", &CONFIG.replace("active_loop = \"rlm\"", "active_loop = \"react\"")] {
        assert_config_error(&run_config(&dir.config(config), Some(SECRET)));
    }
}

#[test]
fn required_fields_and_zero_limits_are_rejected() {
    let dir = TestDir::new();
    for line in CONFIG.lines().filter(|line| {
        ["active_loop =", "base_url =", "model =", "api_key_env =", "log_dir =", "max_cost_micros_usd =", "input_cost_per_million_micros =", "output_cost_per_million_micros ="]
            .iter().any(|field| line.starts_with(field))
    }) {
        let missing = CONFIG.replace(&format!("{line}\n"), "");
        assert_config_error(&run_config(&dir.config(&missing), Some(SECRET)));
    }
    for field in [
        "retry_base_ms", "retry_cap_ms", "max_turns", "max_total_tokens",
        "max_response_bytes", "max_cost_micros_usd", "startup_timeout_secs", "step_timeout_secs",
        "output_view_bytes", "scratch_entries", "session_timeout_secs",
    ] {
        let original = if field == "output_view_bytes" { 192 } else { 1 };
        let config = CONFIG.replace(&format!("{field} = {original}"), &format!("{field} = 0"));
        assert_config_error(&run_config(&dir.config(&config), Some(SECRET)));
    }
}
