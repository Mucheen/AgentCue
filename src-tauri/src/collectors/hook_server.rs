use crate::models::{
    AgentProvider, DaemonInfo, HookWireEnvelope, SessionPhase, SourceChannel,
};
use crate::platform::detect_host_from_pid;
use crate::state::{AppState, SessionMutation};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tauri::AppHandle;
use tokio::net::TcpListener;

#[derive(Clone)]
struct ServerCtx {
    state: AppState,
    app: AppHandle,
}

pub fn spawn_hook_server(app: AppHandle, state: AppState) {
    tauri::async_runtime::spawn(async move {
        let listener = match TcpListener::bind("127.0.0.1:47831").await {
            Ok(l) => l,
            Err(_) => match TcpListener::bind("127.0.0.1:0").await {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("agent-ing hook server failed to bind: {e}");
                    return;
                }
            },
        };

        let addr: SocketAddr = match listener.local_addr() {
            Ok(a) => a,
            Err(_) => return,
        };

        let daemon_info = DaemonInfo {
            port: addr.port(),
            token: state.auth_token.clone(),
            pid: std::process::id(),
            updated_at_ms: Utc::now().timestamp_millis(),
        };

        let app_dir = state.app_data_dir();
        let _ = std::fs::create_dir_all(&app_dir);
        if let Ok(serialized) = serde_json::to_string_pretty(&daemon_info) {
            let _ = std::fs::write(app_dir.join("daemon.json"), serialized);
        }

        let ctx = ServerCtx { state, app };
        let router = Router::new()
            .route("/api/health", get(|| async { Json(json!({ "ok": true })) }))
            .route("/api/hook", post(handle_hook_post))
            .with_state(ctx);

        let _ = axum::serve(listener, router).await;
    });
}

async fn handle_hook_post(
    State(ctx): State<ServerCtx>,
    headers: HeaderMap,
    Json(envelope): Json<HookWireEnvelope>,
) -> (StatusCode, Json<Value>) {
    let provided_token = headers
        .get("x-agent-ing-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if provided_token != ctx.state.auth_token {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid token" })),
        );
    }

    if let Some(mutation) = parse_hook_envelope(&envelope) {
        ctx.state.apply_mutation(Some(&ctx.app), mutation);
    }

    (StatusCode::OK, Json(json!({ "ok": true })))
}

pub fn parse_hook_envelope(env: &HookWireEnvelope) -> Option<SessionMutation> {
    let provider = AgentProvider::from_cli_arg(&env.agent)?;
    let p = &env.payload;

    let event_name = p
        .get("hookEventName")
        .or_else(|| p.get("hook_event_name"))
        .or_else(|| p.get("event"))
        .and_then(|v| v.as_str())
        .or(env.event_hint.as_deref())
        .unwrap_or("Unknown");

    let cwd = p
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            p.get("workspacePaths")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    let raw_session_id = p
        .get("conversationId")
        .or_else(|| p.get("session_id"))
        .or_else(|| p.get("sessionId"))
        .or_else(|| p.get("thread_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| env.env_hints.get("CODEX_THREAD_ID").cloned())
        .unwrap_or_else(|| {
            cwd.clone()
                .unwrap_or_else(|| format!("pid-{}", env.parent_pid.unwrap_or(env.client_pid)))
        });

    let title = p
        .get("lastUserInput")
        .or_else(|| p.get("prompt"))
        .or_else(|| p.get("user_prompt"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let step_count = p
        .get("stepIdx")
        .or_else(|| p.get("step_idx"))
        .or_else(|| p.get("invocationNum"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    let tool_desc = extract_tool_description(p);

    let probe_pid = env.parent_pid.unwrap_or(env.client_pid);
    let term_env = env.env_hints.get("TERM_PROGRAM").map(|s| s.as_str());
    let (host_pid, host_app) = if probe_pid > 1 {
        detect_host_from_pid(probe_pid, term_env)
    } else {
        (None, None)
    };

    let mut last_summary: Option<String> = None;
    let (phase, current_action) = match event_name {
        "SessionStart" => (SessionPhase::Idle, None),
        "UserPromptSubmit" | "PreInvocation" => (
            SessionPhase::Working,
            Some("正在推理与规划下一步…".to_string()),
        ),
        "PreToolUse" => (
            SessionPhase::Working,
            Some(tool_desc.unwrap_or_else(|| "正在调用工具…".to_string())),
        ),
        "PostToolUse" | "PostToolBatch" | "PostInvocation" => {
            let next_act = tool_desc
                .map(|t| format!("已完成 {t} · 继续执行…"))
                .or_else(|| Some("正在整理工具结果…".to_string()));
            (SessionPhase::Working, next_act)
        }
        "PostToolUseFailure" => (
            SessionPhase::Working,
            Some(
                tool_desc
                    .map(|t| format!("工具重试中: {t}"))
                    .unwrap_or_else(|| "工具返回错误，正在恢复…".to_string()),
            ),
        ),
        "PermissionRequest" | "Elicitation" => {
            let wait_msg = tool_desc
                .map(|t| format!("等待授权: {t}"))
                .or_else(|| {
                    p.get("message")
                        .and_then(|v| v.as_str())
                        .map(|m| m.to_string())
                })
                .unwrap_or_else(|| "等待你的权限确认或输入".to_string());
            (SessionPhase::WaitingApproval, Some(wait_msg))
        }
        "Notification" => {
            let ntype = p
                .get("notification_type")
                .or_else(|| p.get("notificationType"))
                .or_else(|| p.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let msg = p
                .get("message")
                .or_else(|| p.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if matches!(ntype, "permission_prompt" | "elicitation_dialog") {
                (
                    SessionPhase::WaitingApproval,
                    Some(if msg.is_empty() {
                        "等待你确认权限请求".to_string()
                    } else {
                        msg.to_string()
                    }),
                )
            } else if ntype == "idle_prompt" {
                (SessionPhase::Idle, None)
            } else {
                return None;
            }
        }
        "Stop" => {
            let term_reason = p
                .get("terminationReason")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            last_summary = p
                .get("last_assistant_message")
                .or_else(|| p.get("summary"))
                .or_else(|| p.get("message"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            if term_reason == "ERROR" {
                last_summary = p
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or(last_summary);
                (SessionPhase::Failed, None)
            } else if term_reason == "USER_CANCELED" {
                (SessionPhase::Idle, None)
            } else {
                (SessionPhase::Completed, None)
            }
        }
        "StopFailure" => {
            last_summary = p
                .get("error")
                .or_else(|| p.get("reason"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (SessionPhase::Failed, None)
        }
        "Interrupt" | "SessionEnd" => (SessionPhase::Idle, None),
        _ => return None,
    };

    Some(SessionMutation {
        raw_session_id,
        provider,
        cwd,
        git_branch: None,
        title,
        phase,
        current_action,
        step_count,
        last_summary,
        host_pid,
        host_app,
        source_channel: SourceChannel::Hook,
        updated_at_ms: None,
        suppress_toast: false,
    })
}

fn extract_tool_description(p: &Value) -> Option<String> {
    // Antigravity format: { "toolCall": { "name": "run_command", "args": { ... } } }
    if let Some(tc) = p.get("toolCall") {
        let name = tc.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
        let args = tc.get("args");
        let detail = args.and_then(|a| {
            a.get("toolSummary")
                .or_else(|| a.get("CommandLine"))
                .or_else(|| a.get("Path"))
                .or_else(|| a.get("query"))
                .and_then(|v| v.as_str())
        });
        return Some(match detail {
            Some(d) if !d.trim().is_empty() => format!("{name}: {d}"),
            _ => name.to_string(),
        });
    }

    // Claude Code / Codex format: { "tool_name": "Bash", "tool_input": { "command": "..." } }
    let name = p
        .get("tool_name")
        .or_else(|| p.get("toolName"))
        .and_then(|v| v.as_str())?;

    let input = p.get("tool_input").or_else(|| p.get("toolInput"));
    let detail = input.and_then(|inp| {
        inp.get("command")
            .or_else(|| inp.get("description"))
            .or_else(|| inp.get("file_path"))
            .or_else(|| inp.get("path"))
            .or_else(|| inp.get("pattern"))
            .and_then(|v| v.as_str())
    });

    Some(match detail {
        Some(d) if !d.trim().is_empty() => format!("{name}: {d}"),
        _ => name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn ordinary_notification_text_is_not_an_approval_request() {
        let mut event = HookWireEnvelope {
            agent: "claude".into(),
            event_hint: Some("Notification".into()),
            client_pid: 1,
            parent_pid: None,
            env_hints: HashMap::new(),
            payload: json!({
                "session_id": "notice",
                "notification_type": "auth_success",
                "message": "The permission check succeeded."
            }),
        };
        assert!(parse_hook_envelope(&event).is_none());
        event.payload["notification_type"] = json!("permission_prompt");
        assert_eq!(
            parse_hook_envelope(&event).unwrap().phase,
            SessionPhase::WaitingApproval
        );
    }

    #[test]
    fn parses_antigravity_and_claude_hook_envelopes() {
        let agy_stop = HookWireEnvelope {
            agent: "antigravity".into(),
            event_hint: Some("Stop".into()),
            client_pid: 1,
            parent_pid: None,
            env_hints: HashMap::new(),
            payload: json!({
                "conversationId": "conv-123",
                "cwd": "/work/agentcue",
                "lastUserInput": "做一个跨平台悬浮窗",
                "hookEventName": "Stop",
                "terminationReason": "NO_TOOL_CALL"
            }),
        };
        let m = parse_hook_envelope(&agy_stop).unwrap();
        assert_eq!(m.provider, AgentProvider::Antigravity);
        assert_eq!(m.phase, SessionPhase::Completed);
        assert_eq!(m.raw_session_id, "conv-123");

        let claude_perm = HookWireEnvelope {
            agent: "claude".into(),
            event_hint: None,
            client_pid: 1,
            parent_pid: None,
            env_hints: HashMap::new(),
            payload: json!({
                "session_id": "cl-999",
                "cwd": "/work/demo",
                "hook_event_name": "PermissionRequest",
                "tool_name": "Bash",
                "tool_input": { "command": "git push origin main" }
            }),
        };
        let m2 = parse_hook_envelope(&claude_perm).unwrap();
        assert_eq!(m2.provider, AgentProvider::ClaudeCode);
        assert_eq!(m2.phase, SessionPhase::WaitingApproval);
        assert_eq!(
            m2.current_action.as_deref(),
            Some("等待授权: Bash: git push origin main")
        );
    }
}
