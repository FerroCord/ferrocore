use std::{env, fs, path::{Path, PathBuf}};

use serde::Deserialize;

pub struct Config {
    pub active_loop: String,
    pub llm: LlmConfig,
    pub budget: BudgetConfig,
    pub pty: PtyConfig,
    pub session: SessionConfig,
}

pub struct LlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
    pub api_key: String,
    pub max_retries: u32,
    pub retry_base_ms: u64,
    pub retry_cap_ms: u64,
}


#[derive(Debug, Deserialize)]
pub struct BudgetConfig {
    #[serde(default = "default_max_turns")]
    pub max_turns: u64,
    #[serde(default = "default_max_total_tokens")]
    pub max_total_tokens: u64,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
    pub max_cost_micros_usd: Option<u64>,
    pub input_cost_per_million_micros: Option<u64>,
    pub output_cost_per_million_micros: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct PtyConfig {
    #[serde(default = "default_startup_timeout_secs")]
    pub startup_timeout_secs: u64,
    pub step_timeout_secs: Option<u64>,
    #[serde(default = "default_output_view_bytes")]
    pub output_view_bytes: usize,
    #[serde(default = "default_scratch_entries")]
    pub scratch_entries: usize,
}

#[derive(Debug, Deserialize)]
pub struct SessionConfig {
    pub session_timeout_secs: Option<u64>,
    pub log_dir: PathBuf,
}

#[derive(Deserialize)]
struct RawConfig {
    active_loop: String,
    llm: RawLlmConfig,
    budget: BudgetConfig,
    pty: PtyConfig,
    session: SessionConfig,
}

#[derive(Deserialize)]
struct RawLlmConfig {
    base_url: String,
    model: String,
    api_key_env: String,
    #[serde(default = "default_max_retries")]
    max_retries: u32,
    #[serde(default = "default_retry_base_ms")]
    retry_base_ms: u64,
    #[serde(default = "default_retry_cap_ms")]
    retry_cap_ms: u64,
}

fn default_max_retries() -> u32 { 3 }
fn default_retry_base_ms() -> u64 { 500 }
fn default_retry_cap_ms() -> u64 { 8_000 }
fn default_max_turns() -> u64 { 20 }
fn default_max_total_tokens() -> u64 { 50_000 }
fn default_max_response_bytes() -> usize { 1_048_576 }
fn default_startup_timeout_secs() -> u64 { 5 }
fn default_output_view_bytes() -> usize { 16_384 }
fn default_scratch_entries() -> usize { 20 }

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = fs::read_to_string(path)
            .map_err(|err| format!("Cannot read configuration: {err}"))?;
        // Do not include TOML input in parse errors: it may contain credentials.
        let raw: RawConfig = toml::from_str(&text)
            .map_err(|_| "Invalid configuration TOML or missing required field".to_string())?;
        if raw.active_loop != "rlm" {
            return Err("Unsupported active_loop; expected rlm".into());
        }
        if raw.llm.base_url.trim().is_empty() || raw.llm.model.trim().is_empty()
            || raw.llm.api_key_env.trim().is_empty() || raw.session.log_dir.as_os_str().is_empty()
        {
            return Err("LLM settings and session.log_dir must not be empty".into());
        }
        if raw.llm.retry_base_ms == 0 || raw.llm.retry_cap_ms == 0
            || raw.budget.max_turns == 0 || raw.budget.max_total_tokens == 0
            || raw.budget.max_response_bytes == 0 || raw.pty.startup_timeout_secs == 0
            || raw.pty.step_timeout_secs == Some(0) || raw.pty.output_view_bytes == 0
            || raw.pty.scratch_entries == 0 || raw.session.session_timeout_secs == Some(0)
            || raw.budget.max_cost_micros_usd == Some(0)
        {
            return Err("Limits and timeouts must be greater than zero".into());
        }
        if raw.pty.output_view_bytes < ferrocore_runtime_pty::output::MIN_VIEW_BYTES_FOR_NOTICE {
            return Err("pty.output_view_bytes must be at least 192".into());
        }
        let cost_fields = [raw.budget.max_cost_micros_usd, raw.budget.input_cost_per_million_micros, raw.budget.output_cost_per_million_micros];
        if cost_fields.iter().any(Option::is_some) && !cost_fields.iter().all(Option::is_some) {
            return Err("Cost limit and both prices must be configured together".into());
        }
        let api_key = env::var(&raw.llm.api_key_env)
            .map_err(|_| "API key environment variable is missing or invalid".to_string())?;
        if api_key.trim().is_empty() {
            return Err("API key environment variable is empty".into());
        }
        Ok(Config {
            active_loop: raw.active_loop,
            llm: LlmConfig {
                base_url: raw.llm.base_url,
                model: raw.llm.model,
                api_key_env: raw.llm.api_key_env,
                api_key,
                max_retries: raw.llm.max_retries,
                retry_base_ms: raw.llm.retry_base_ms,
                retry_cap_ms: raw.llm.retry_cap_ms,
            },
            budget: raw.budget,
            pty: raw.pty,
            session: raw.session,
        })
    }
}
