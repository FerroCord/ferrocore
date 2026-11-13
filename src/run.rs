use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use ferrocore_driver::{budget::{Budget, BudgetConfig}, llm::{LlmClient, LlmConfig}, run_llm_step, SessionLogWriter};
use ferrocore_runtime_pty::{PtyCommandOutcome, PtyRuntime};
use ferrocore_types::{AgentLoop, LoopAction, Message, Session, TerminalSnapshot};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use crate::config::Config;

fn bounded_deadline(session: Option<tokio::time::Instant>, seconds: Option<u64>) -> Result<Option<tokio::time::Instant>, String> {
    let now = tokio::time::Instant::now();
    let step = match seconds {
        Some(s) => match now.checked_add(Duration::from_secs(s)) {
            Some(deadline) => Some(deadline),
            None if session.is_some() => session,
            None => return Err("configured timeout exceeds the supported deadline range".into()),
        },
        None => None,
    };
    Ok(match (session, step) {
        (Some(session), Some(step)) => Some(session.min(step)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    })
}

pub async fn run(config: Config, task: String) -> Result<String, String> {
    let client = LlmClient::new(LlmConfig {
        base_url: config.llm.base_url.clone(), model: config.llm.model.clone(),
        api_key: config.llm.api_key.clone(), max_retries: config.llm.max_retries,
        retry_base: Duration::from_millis(config.llm.retry_base_ms),
        retry_cap: Duration::from_millis(config.llm.retry_cap_ms),
        max_response_bytes: config.budget.max_response_bytes,
    }).map_err(|e| format!("invalid LLM configuration: {e:?}"))?;
    let mut budget = Budget::new(BudgetConfig {
        max_turns: config.budget.max_turns, max_total_tokens: config.budget.max_total_tokens,
        max_cost_micros_usd: config.budget.max_cost_micros_usd,
        input_cost_per_million_micros: config.budget.input_cost_per_million_micros,
        output_cost_per_million_micros: config.budget.output_cost_per_million_micros,
    }).map_err(|e| e.to_string())?;
    let deadline = bounded_deadline(None, config.session.session_timeout_secs)?;
    let log = SessionLogWriter::open(&config.session.log_dir, config.llm.api_key.clone())
        .await.map_err(|e| e.to_string())?;
    let cancel = CancellationToken::new();
    let command_cancel = Arc::new(AtomicBool::new(false));
    let signal_token = cancel.clone();
    let signal_flag = command_cancel.clone();
    let signal = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signal_flag.store(true, Ordering::Release);
            signal_token.cancel();
        }
    });
    let result = drive(&config, task, &client, &mut budget, &log, &cancel, command_cancel, deadline).await;
    signal.abort();
    let closed = log.close().await.map_err(|e| e.to_string());
    let result = match (result, closed) {
        (Ok(answer), Ok(())) => Ok(answer),
        (Err(error), Err(close_error)) => Err(format!("{error}; session log close failed: {close_error}")),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    };
    result.map(|s| s.replace(&config.llm.api_key, "[REDACTED]"))
        .map_err(|s| s.replace(&config.llm.api_key, "[REDACTED]"))
}

async fn drive(config: &Config, task: String, client: &LlmClient, budget: &mut Budget,
    log: &SessionLogWriter, cancel: &CancellationToken, command_cancel: Arc<AtomicBool>,
    deadline: Option<tokio::time::Instant>) -> Result<String, String> {
    let root = log.append(None, 0, "rlm", "session_start", json!({"task": task})).await.map_err(|e| e.to_string())?;
    log.sync_turn().await.map_err(|e| e.to_string())?;
    if cancel.is_cancelled() || deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
        let error_id = log.append(Some(root), 0, "rlm", "error", json!({"reason":"cancelled_or_timeout"})).await.map_err(|e| e.to_string())?;
        log.append(Some(error_id), 0, "rlm", "session_end", json!({"success":false})).await.map_err(|e| e.to_string())?;
        log.sync_turn().await.map_err(|e| e.to_string())?;
        return Err("session cancelled or timed out".into());
    }
    let startup_timeout = Duration::from_secs(config.pty.startup_timeout_secs);
    let startup_timeout = deadline.map(|d| startup_timeout.min(d.saturating_duration_since(tokio::time::Instant::now()))).unwrap_or(startup_timeout);
    let mut pty = match tokio::task::block_in_place(|| PtyRuntime::new_cancellable(startup_timeout,
        &config.llm.api_key_env, config.pty.output_view_bytes, &command_cancel)) {
        Ok(pty) => pty,
        Err(error) => {
            let message = error.to_string();
            let id = log.append(Some(root), 0, "rlm", "error", json!({"reason":"pty_startup", "message":message})).await.map_err(|e| e.to_string())?;
            log.append(Some(id), 0, "rlm", "session_end", json!({"success":false})).await.map_err(|e| e.to_string())?;
            log.sync_turn().await.map_err(|e| e.to_string())?;
            return Err(message);
        }
    };
    let mut session = Session { id: log.session_id().to_string(), loop_type: config.active_loop.clone(),
        agent_depth: 0, history: vec![Message { role: "user".into(), content: task }],
        turns: 0, total_tokens: 0, last_record_id: Some(root.to_string()) };
    let mut parent = root;
    let agent = ferrocore_loop_rlm::RlmLoop::default();
    let mut malformed_count = 0u8;
    let result = loop {
        let state = pty.snapshot(config.pty.scratch_entries);
        let snapshot = TerminalSnapshot { pwd: state.pwd, last_exit_code: state.last_exit_code,
            jobs: state.jobs, scratch_names: state.scratch_names, scratch_total: state.scratch_total };
        let messages = match agent.prepare_turn(&session, &snapshot).await {
            Ok(m) => m, Err(e) => break Err(e.to_string()),
        };
        let step_deadline = match bounded_deadline(deadline, config.pty.step_timeout_secs) {
            Ok(deadline) => deadline, Err(error) => break Err(error),
        };
        let step = match run_llm_step(client, budget, log, parent, &messages, cancel, step_deadline).await {
            Ok(s) => s, Err(e) => {
                if let ferrocore_driver::LlmStepError::Budget { last_record_id: Some(id), .. } | ferrocore_driver::LlmStepError::Llm { last_record_id: Some(id), .. } = &e { parent = *id; }
                break Err(e.to_string());
            },
        };
        parent = step.last_record_id;
        session.turns = budget.turns();
        session.total_tokens = budget.total_tokens();
        if step.response.finish_reason.as_deref() != Some("stop") {
            let reason = if step.response.finish_reason.as_deref() == Some("length") { "finish_reason_length" } else { "invalid_finish_reason" };
            parent = match log.append(Some(parent), 0, "rlm", "error", json!({"reason":reason})).await {
                Ok(id) => id, Err(e) => break Err(e.to_string()),
            };
            break Err("model response did not finish normally".into());
        }
        session.history.push(Message { role: "assistant".into(), content: step.response.text.clone() });
        let action = match agent.on_model_response(&step.response.text).await {
            Ok(a) => a, Err(e) => break Err(e.to_string()),
        };
        match action {
            LoopAction::Final(answer) => {
                match log.append(Some(parent), 0, "rlm", "final", json!({"answer": answer})).await {
                    Ok(id) => { parent = id; break Ok(answer); }, Err(e) => break Err(e.to_string()),
                }
            }
            LoopAction::Malformed(reason) => {
                malformed_count += 1;
                parent = match log.append(Some(parent), 0, "rlm", "error", json!({"reason":"malformed_response", "message":reason})).await {
                    Ok(id) => id, Err(e) => break Err(e.to_string()),
                };
                if malformed_count >= 3 { break Err("malformed model response".into()); }
                session.history.push(Message { role: "user".into(), content: format!("Malformed response: {reason}. Please correct the response format.") });
                session.last_record_id = Some(parent.to_string());
                if let Err(e) = log.sync_turn().await { break Err(e.to_string()); }
            }
            LoopAction::ExecShell(command) => {
                malformed_count = 0;
                parent = match log.append(Some(parent), 0, "rlm", "pty_exec", json!({"command": command})).await {
                    Ok(id) => id, Err(e) => break Err(e.to_string()),
                };
                let step_deadline = match bounded_deadline(deadline, config.pty.step_timeout_secs) {
                    Ok(deadline) => deadline, Err(error) => break Err(error),
                };
                if cancel.is_cancelled() || step_deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                    break Err("session cancelled or timed out".into());
                }
                let timeout = step_deadline.map(|d| d.saturating_duration_since(tokio::time::Instant::now())).unwrap_or(Duration::MAX);
                let outcome = tokio::task::block_in_place(|| pty.execute(&command, timeout, &command_cancel));
                match outcome {
                    Ok(PtyCommandOutcome::Completed { exit_code, output }) => {
                        parent = match log.append(Some(parent), 0, "rlm", "pty_result", json!({"exit_code":exit_code, "output":output.text, "raw_bytes":output.raw_bytes, "lines":output.lines, "pruned":output.pruned, "invalid_utf8":output.invalid_utf8})).await {
                            Ok(id) => id, Err(e) => break Err(e.to_string()),
                        };
                        session.history.push(Message { role:"user".into(), content:format!("Exit code: {exit_code}\n{}", output.text) });
                    }
                    Ok(PtyCommandOutcome::ShellExited { exit_status, partial_output, partial_output_truncated }) => {
                        let logged = log.append(Some(parent), 0, "rlm", "pty_result", json!({"exit_status":exit_status, "output":String::from_utf8_lossy(&partial_output), "partial":true, "truncated":partial_output_truncated})).await;
                        match logged { Ok(id) => parent = id, Err(e) => break Err(e.to_string()) };
                        break Err("shell exited".into());
                    }
                    Err(e) => {
                        let logged = log.append(Some(parent), 0, "rlm", "pty_result", json!({"output":String::from_utf8_lossy(&e.partial_output), "partial":true, "truncated":e.partial_output_truncated, "error":e.to_string()})).await;
                        match logged { Ok(id) => parent = id, Err(error) => break Err(format!("{e}; PTY result logging failed: {error}")) };
                        break Err(e.to_string());
                    }
                }
                if let Err(e) = log.sync_turn().await { break Err(e.to_string()); }
                session.last_record_id = Some(parent.to_string());
            }
        }
    };
    if let Err(e) = pty.shutdown() { eprintln!("cleanup warning: {}", e.to_string().replace(&config.llm.api_key, "[REDACTED]")); }
    let logged = async {
        if let Err(error) = &result {
            parent = log.append(Some(parent), 0, "rlm", "error", json!({"reason":"session_failed", "message":error})).await.map_err(|e| e.to_string())?;
        }
        log.append(Some(parent), 0, "rlm", "session_end", json!({"success":result.is_ok()})).await.map_err(|e| e.to_string())?;
        log.sync_turn().await.map_err(|e| e.to_string())
    }.await;
    match (result, logged) {
        (Err(error), Err(log_error)) => Err(format!("{error}; session finalization logging failed: {log_error}")),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Ok(answer), Ok(())) => Ok(answer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_step_does_not_mask_earlier_session_deadline() {
        let session = tokio::time::Instant::now() + Duration::from_secs(1);
        assert_eq!(bounded_deadline(Some(session), Some(u64::MAX)).unwrap(), Some(session));
    }
}
