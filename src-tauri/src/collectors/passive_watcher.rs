//! Zero-intrusion read-only passive session discovery for Antigravity, Codex, and Claude Code.
//! Inspired by Dashbar & Loom: watches local SQLite summaries and tails recent `.jsonl` transcripts
//! so active sessions appear immediately on launch even before client hooks are installed.

use crate::models::{AgentProvider, SessionPhase, SourceChannel};
use crate::state::{AppState, SessionMutation};
use chrono::{DateTime, Utc};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tauri::AppHandle;
use tokio::sync::mpsc;

pub fn spawn_passive_watcher(app: AppHandle, state: AppState) {
    tauri::async_runtime::spawn(async move {
        let home = state.home_dir().clone();
        let (tx, mut rx) = mpsc::channel::<()>(16);

        // Best-effort filesystem event watcher to wake up polling immediately on log changes
        let _watcher = setup_fs_watcher(&home, tx);

        // Initial hydration pass (suppress_toast = true so historical completed runs don't pop Toasts)
        scan_all_providers(&home, &app, &state, true);

        loop {
            let _ = tokio::time::timeout(Duration::from_millis(1800), rx.recv()).await;
            // Drain burst events and coalesce over 180ms
            tokio::time::sleep(Duration::from_millis(180)).await;
            while rx.try_recv().is_ok() {}
            scan_all_providers(&home, &app, &state, false);
        }
    });
}

fn setup_fs_watcher(home: &Path, tx: mpsc::Sender<()>) -> Option<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.try_send(());
        }
    })
    .ok()?;

    let watch_roots = [
        home.join(".gemini").join("antigravity"),
        home.join(".codex").join("sessions"),
        home.join(".claude").join("projects"),
    ];

    for root in watch_roots {
        if root.exists() {
            let _ = watcher.watch(&root, RecursiveMode::Recursive);
        }
    }
    Some(watcher)
}

fn scan_all_providers(home: &Path, app: &AppHandle, state: &AppState, suppress_toast: bool) {
    for m in scan_antigravity(home, suppress_toast) {
        state.apply_mutation(Some(app), m);
    }
    for m in scan_codex(home, suppress_toast) {
        state.apply_mutation(Some(app), m);
    }
    for m in scan_claude(home, suppress_toast) {
        state.apply_mutation(Some(app), m);
    }
}

fn read_tail_string(path: &Path, max_bytes: u64) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len > max_bytes {
        let _ = f.seek(SeekFrom::End(-(max_bytes as i64)));
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if len > max_bytes {
        // Drop partial first line after seek
        text.find('\n').map(|idx| text[idx + 1..].to_string())
    } else {
        Some(text)
    }
}

fn file_mtime_ms(path: &Path) -> Option<i64> {
    let mtime = path.metadata().ok()?.modified().ok()?;
    let dur = mtime.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(dur.as_millis() as i64)
}

fn parse_any_timestamp_ms(raw: &str) -> Option<i64> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.timestamp_millis());
    }
    // SQLite datetime fallback "YYYY-MM-DD HH:MM:SS..."
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(
        raw.split('.').next().unwrap_or(raw),
        "%Y-%m-%d %H:%M:%S",
    ) {
        return Some(naive.and_utc().timestamp_millis());
    }
    None
}

// -----------------------------------------------------------------------------
// 1. Google Antigravity Passive Reader (`conversation_summaries.db` + `transcript.jsonl`)
// -----------------------------------------------------------------------------
fn scan_antigravity(home: &Path, suppress_toast: bool) -> Vec<SessionMutation> {
    let mut out = Vec::new();
    let agy_root = home.join(".gemini").join("antigravity");
    let db_path = agy_root.join("conversation_summaries.db");
    if !db_path.exists() {
        return out;
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = Connection::open_with_flags(&db_path, flags) else {
        return out;
    };
    let _ = conn.busy_timeout(Duration::from_millis(80));

    let sql = "SELECT conversation_id, title, preview, step_count, last_modified_time, workspace_uris, status, not_fully_idle
               FROM conversation_summaries
               WHERE parent_conversation_id = '' AND killed = 0
               ORDER BY last_modified_time DESC
               LIMIT 8";

    let Ok(mut stmt) = conn.prepare(sql) else {
        return out;
    };

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1).unwrap_or_default(),
            row.get::<_, String>(2).unwrap_or_default(),
            row.get::<_, i64>(3).unwrap_or(0),
            row.get::<_, String>(4).unwrap_or_default(),
            row.get::<_, String>(5).unwrap_or_default(),
            row.get::<_, String>(6).unwrap_or_default(),
            row.get::<_, i64>(7).unwrap_or(0),
        ))
    });

    let Ok(rows) = rows else {
        return out;
    };

    let now_ms = Utc::now().timestamp_millis();

    for row in rows.flatten() {
        let (conv_id, title, preview, step_count, last_mod, ws_uris, status_raw, not_idle) = row;
        let cwd = parse_workspace_uri(&ws_uris);
        let transcript_path = agy_root
            .join("brain")
            .join(&conv_id)
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");

        let transcript_mtime = file_mtime_ms(&transcript_path);
        let updated_ms = transcript_mtime
            .max(parse_any_timestamp_ms(&last_mod))
            .unwrap_or(now_ms);

        // Skip very old conversations (> 12h)
        if now_ms - updated_ms > 12 * 3600 * 1000 {
            continue;
        }

        let (tail_action, tail_summary, tail_busy) = inspect_agy_transcript(&transcript_path, now_ms);
        let phase = antigravity_phase(&status_raw, not_idle != 0, tail_busy);

        out.push(SessionMutation {
            raw_session_id: conv_id,
            provider: AgentProvider::Antigravity,
            cwd,
            git_branch: None,
            title: Some(if title.trim().is_empty() {
                preview.clone()
            } else {
                title
            }),
            phase,
            current_action: match phase {
                SessionPhase::Working | SessionPhase::WaitingApproval => tail_action,
                _ => None,
            },
            step_count: Some(step_count.max(0) as u32),
            last_summary: tail_summary.or_else(|| (!preview.is_empty()).then_some(preview)),
            host_pid: None,
            host_app: Some("Antigravity".to_string()),
            source_channel: SourceChannel::Passive,
            updated_at_ms: Some(updated_ms),
            suppress_toast,
        });
    }

    out
}

fn antigravity_phase(status: &str, not_fully_idle: bool, transcript_busy: bool) -> SessionPhase {
    let normalized = status.trim().to_ascii_uppercase();
    let status = normalized
        .strip_prefix("CASCADE_RUN_STATUS_")
        .unwrap_or(&normalized);

    match status {
        "IDLE" if !not_fully_idle => SessionPhase::Completed,
        "BUSY" | "RUNNING" | "WORKING" | "CANCELING" => SessionPhase::Working,
        "WAITING_APPROVAL" | "WAITING_FOR_APPROVAL" | "AWAITING_APPROVAL"
        | "WAITING_FOR_USER" | "PENDING_APPROVAL" => {
            SessionPhase::WaitingApproval
        }
        "ERROR" | "FAILED" | "FAILURE" => SessionPhase::Failed,
        _ if not_fully_idle || transcript_busy => SessionPhase::Working,
        _ => SessionPhase::Idle,
    }
}

fn parse_workspace_uri(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // workspace_uris may be a JSON array `["file:///Users/..."]` or comma/newline separated
    let first = if let Ok(arr) = serde_json::from_str::<Vec<String>>(trimmed) {
        arr.into_iter().next()?
    } else {
        trimmed
            .trim_matches(['[', ']', '"'])
            .split(',')
            .next()?
            .trim()
            .trim_matches('"')
            .to_string()
    };
    Some(first.strip_prefix("file://").unwrap_or(&first).to_string())
}

fn inspect_agy_transcript(path: &Path, now_ms: i64) -> (Option<String>, Option<String>, bool) {
    let Some(mtime) = file_mtime_ms(path) else {
        return (None, None, false);
    };
    let Some(tail) = read_tail_string(path, 32 * 1024) else {
        return (None, None, false);
    };

    let mut latest_tool: Option<String> = None;
    let mut latest_summary: Option<String> = None;
    let mut last_step_has_tools = false;
    let mut last_step_is_user = false;

    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let step_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if step_type == "USER_INPUT" {
            last_step_is_user = true;
            last_step_has_tools = false;
        } else if step_type == "PLANNER_RESPONSE" {
            last_step_is_user = false;
            if let Some(tcs) = v.get("tool_calls").and_then(|x| x.as_array()) {
                if let Some(tc) = tcs.last() {
                    last_step_has_tools = true;
                    let tname = tc.get("name").and_then(|x| x.as_str()).unwrap_or("tool");
                    let detail = tc.get("arguments").or_else(|| tc.get("args")).and_then(|a| {
                        a.get("toolSummary")
                            .or_else(|| a.get("CommandLine"))
                            .or_else(|| a.get("Path"))
                            .or_else(|| a.get("query"))
                            .and_then(|x| x.as_str())
                    });
                    latest_tool = Some(match detail {
                        Some(d) if !d.is_empty() => format!("{tname}: {d}"),
                        _ => tname.to_string(),
                    });
                } else {
                    last_step_has_tools = false;
                }
            } else {
                last_step_has_tools = false;
            }

            if !last_step_has_tools {
                if let Some(c) = v.get("content").and_then(|x| x.as_str()) {
                    if !c.trim().is_empty() {
                        latest_summary = Some(c.lines().next().unwrap_or(c).to_string());
                    }
                }
            }
        }
    }

    let pending_step = last_step_has_tools || last_step_is_user;
    let is_busy = pending_step && (now_ms - mtime) < 28_000;
    let action = if pending_step {
        latest_tool.or_else(|| Some("正在推理生成回复…".to_string()))
    } else {
        None
    };

    (action, latest_summary, is_busy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn antigravity_database_status_takes_precedence_over_transcript_tail() {
        for (status, not_fully_idle, tail_busy, expected) in [
            ("CASCADE_RUN_STATUS_IDLE", false, true, SessionPhase::Completed),
            ("CASCADE_RUN_STATUS_IDLE", true, false, SessionPhase::Working),
            ("CASCADE_RUN_STATUS_BUSY", false, false, SessionPhase::Working),
            ("CASCADE_RUN_STATUS_RUNNING", false, false, SessionPhase::Working),
            ("CASCADE_RUN_STATUS_CANCELING", false, false, SessionPhase::Working),
            ("WAITING_APPROVAL", true, true, SessionPhase::WaitingApproval),
            ("WAITING_FOR_TOOL", true, true, SessionPhase::Working),
            ("FAILED", true, true, SessionPhase::Failed),
            ("CASCADE_RUN_STATUS_UNSPECIFIED", false, true, SessionPhase::Working),
            ("CASCADE_RUN_STATUS_UNSPECIFIED", false, false, SessionPhase::Idle),
        ] {
            assert_eq!(antigravity_phase(status, not_fully_idle, tail_busy), expected, "{status}");
        }
    }

    #[test]
    fn codex_output_keeps_turn_active_until_completion() {
        let transcript = tempfile::NamedTempFile::new().unwrap();
        let output = r#"{"type":"session_meta","payload":{"id":"codex-session","originator":"Codex Desktop"}}
{"type":"event_msg","payload":{"type":"task_started"}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"正在检查输出"}]}}
"#;
        std::fs::write(transcript.path(), output).unwrap();
        let titles = HashMap::new();
        let progress = parse_codex_rollout(transcript.path(), 1, &titles, false).unwrap();
        assert_eq!(progress.phase, SessionPhase::Working);
        assert_eq!(progress.host_app.as_deref(), Some("Codex"));

        let end = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
        std::fs::write(transcript.path(), format!("{output}{end}\n")).unwrap();
        let completed = parse_codex_rollout(transcript.path(), 2, &titles, false).unwrap();
        assert_eq!(completed.phase, SessionPhase::Completed);
    }

    #[test]
    fn claude_output_requires_a_stop_reason_or_turn_end() {
        let transcript = tempfile::NamedTempFile::new().unwrap();
        let output = r#"{"type":"user","sessionId":"claude-session","message":{"content":"检查当前进度"}}
{"type":"assistant","sessionId":"claude-session","message":{"stop_reason":null,"content":[{"type":"text","text":"正在检查输出"}]}}
"#;
        std::fs::write(transcript.path(), output).unwrap();
        let progress = parse_claude_transcript(transcript.path(), 1, false).unwrap();
        assert_eq!(progress.phase, SessionPhase::Working);

        let end = r#"{"type":"system","subtype":"turn_duration","durationMs":8200}"#;
        std::fs::write(transcript.path(), format!("{output}{end}\n")).unwrap();
        let completed = parse_claude_transcript(transcript.path(), 2, false).unwrap();
        assert_eq!(completed.phase, SessionPhase::Completed);
    }
}

// -----------------------------------------------------------------------------
// 2. OpenAI Codex Passive Reader (`session_index.jsonl` + `sessions/YYYY/MM/DD/rollout-*.jsonl`)
// -----------------------------------------------------------------------------
fn scan_codex(home: &Path, suppress_toast: bool) -> Vec<SessionMutation> {
    let mut out = Vec::new();
    let codex_dir = home.join(".codex");
    let sessions_dir = codex_dir.join("sessions");
    if !sessions_dir.exists() {
        return out;
    }

    let mut thread_titles: HashMap<String, String> = HashMap::new();
    if let Some(idx_tail) = read_tail_string(&codex_dir.join("session_index.jsonl"), 32 * 1024) {
        for line in idx_tail.lines() {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                if let (Some(id), Some(name)) = (
                    v.get("id").and_then(|x| x.as_str()),
                    v.get("thread_name").and_then(|x| x.as_str()),
                ) {
                    thread_titles.insert(id.to_string(), name.to_string());
                }
            }
        }
    }

    let rollout_files = collect_recent_jsonl_files(&sessions_dir, 4, 6);
    let now_ms = Utc::now().timestamp_millis();

    for (path, mtime_ms) in rollout_files {
        if now_ms - mtime_ms > 12 * 3600 * 1000 {
            continue;
        }
        if let Some(m) = parse_codex_rollout(&path, mtime_ms, &thread_titles, suppress_toast)
        {
            out.push(m);
        }
    }

    out
}

fn parse_codex_rollout(
    path: &Path,
    mtime_ms: i64,
    titles: &HashMap<String, String>,
    suppress_toast: bool,
) -> Option<SessionMutation> {
    let f = File::open(path).ok()?;
    let mut reader = BufReader::new(f);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;
    let first_json: Value = serde_json::from_str(&first_line).ok()?;

    let payload = first_json.get("payload").unwrap_or(&first_json);
    let session_id = payload
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })?;

    let cwd = payload
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let desktop_host = (payload.get("originator").and_then(Value::as_str) == Some("Codex Desktop"))
        .then(|| "Codex".to_string());
    let git_branch = payload
        .get("git")
        .and_then(|g| g.get("branch"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tail = read_tail_string(path, 48 * 1024)?;
    let mut phase = SessionPhase::Idle;
    let mut latest_action: Option<String> = None;
    let mut latest_summary: Option<String> = None;
    let mut user_prompt: Option<String> = None;
    let mut step_count = 0u32;

    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let top_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        let p = v.get("payload");

        match top_type {
            "event_msg" => {
                let ev_type = p
                    .and_then(|x| x.get("type"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                match ev_type {
                    "task_started" | "turn_started" => {
                        phase = SessionPhase::Working;
                        latest_action = Some("正在分析任务…".to_string());
                    }
                    "task_complete" | "turn_complete" => {
                        phase = SessionPhase::Completed;
                        if let Some(msg) = p
                            .and_then(|x| x.get("last_agent_message"))
                            .and_then(|x| x.as_str())
                        {
                            latest_summary = Some(msg.to_string());
                        }
                    }
                    "turn_aborted" => phase = SessionPhase::Idle,
                    "user_message" => {
                        if let Some(msg) = p.and_then(|x| x.get("message")).and_then(|x| x.as_str())
                        {
                            user_prompt = Some(msg.to_string());
                        }
                    }
                    _ => {}
                }
            }
            "response_item" => {
                let item_type = p
                    .and_then(|x| x.get("type"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                match item_type {
                    "function_call" | "custom_tool_call" => {
                        step_count += 1;
                        phase = SessionPhase::Working;
                        let name = p
                            .and_then(|x| x.get("name"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("tool");
                        latest_action = Some(format!("执行工具: {name}"));
                    }
                    "reasoning" => {
                        if phase == SessionPhase::Working && latest_action.is_none() {
                            latest_action = Some("正在深度推理…".to_string());
                        }
                    }
                    "message" => {
                        let role = p
                            .and_then(|x| x.get("role"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        if role == "assistant" {
                            if let Some(txt) = extract_codex_message_text(p) {
                                latest_summary = Some(txt);
                            }
                        } else if role == "user" && user_prompt.is_none() {
                            user_prompt = extract_codex_message_text(p);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let title = titles.get(&session_id).cloned().or(user_prompt);

    Some(SessionMutation {
        raw_session_id: session_id,
        provider: AgentProvider::Codex,
        cwd,
        git_branch,
        title,
        phase,
        current_action: if phase == SessionPhase::Working { latest_action } else { None },
        step_count: (step_count > 0).then_some(step_count),
        last_summary: latest_summary,
        host_pid: None,
        host_app: desktop_host,
        source_channel: SourceChannel::Passive,
        updated_at_ms: Some(mtime_ms),
        suppress_toast,
    })
}

fn extract_codex_message_text(payload: Option<&Value>) -> Option<String> {
    let arr = payload?.get("content")?.as_array()?;
    for item in arr {
        if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
            let trimmed = t.trim();
            if !trimmed.is_empty() && !trimmed.starts_with("<environment_context>") {
                return Some(trimmed.lines().next().unwrap_or(trimmed).to_string());
            }
        }
    }
    None
}

// -----------------------------------------------------------------------------
// 3. Claude Code Passive Reader (`~/.claude/projects/*/*.jsonl`)
// -----------------------------------------------------------------------------
fn scan_claude(home: &Path, suppress_toast: bool) -> Vec<SessionMutation> {
    let mut out = Vec::new();
    let proj_dir = home.join(".claude").join("projects");
    if !proj_dir.exists() {
        return out;
    }

    let files = collect_recent_jsonl_files(&proj_dir, 2, 6);
    let now_ms = Utc::now().timestamp_millis();

    for (path, mtime_ms) in files {
        if now_ms - mtime_ms > 12 * 3600 * 1000 {
            continue;
        }
        if let Some(m) = parse_claude_transcript(&path, mtime_ms, suppress_toast) {
            out.push(m);
        }
    }

    out
}

fn parse_claude_transcript(
    path: &Path,
    mtime_ms: i64,
    suppress_toast: bool,
) -> Option<SessionMutation> {
    let tail = read_tail_string(path, 48 * 1024)?;
    let mut session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut title: Option<String> = None;
    let mut latest_tool: Option<String> = None;
    let mut latest_summary: Option<String> = None;
    let mut phase = SessionPhase::Idle;
    let mut step_count = 0u32;

    for line in tail.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if session_id.is_none() {
            session_id = v
                .get("sessionId")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
        }
        if cwd.is_none() {
            cwd = v.get("cwd").and_then(|x| x.as_str()).map(|s| s.to_string());
        }
        if git_branch.is_none() {
            git_branch = v
                .get("gitBranch")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
        }

        let entry_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if entry_type == "last-prompt" {
            if let Some(lp) = v.get("lastPrompt").and_then(|x| x.as_str()) {
                title = Some(lp.to_string());
            }
        } else if entry_type == "user" {
            phase = SessionPhase::Working;
            if let Some(content_str) = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                if !content_str.trim().is_empty() {
                    title = Some(content_str.to_string());
                }
            }
        } else if entry_type == "system"
            && v.get("subtype").and_then(Value::as_str) == Some("turn_duration")
        {
            phase = SessionPhase::Completed;
        } else if let ("assistant", Some(message)) = (entry_type, v.get("message")) {
            if let Some(blocks) = message.get("content").and_then(Value::as_array) {
                let mut has_tool_use = false;
                for b in blocks {
                    let btype = b.get("type").and_then(|x| x.as_str()).unwrap_or("");
                    if btype == "tool_use" {
                        has_tool_use = true;
                        step_count += 1;
                        let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("Tool");
                        let detail = b.get("input").and_then(|inp| {
                            inp.get("command")
                                .or_else(|| inp.get("file_path"))
                                .or_else(|| inp.get("description"))
                                .and_then(|x| x.as_str())
                        });
                        latest_tool = Some(match detail {
                            Some(d) if !d.is_empty() => format!("{name}: {d}"),
                            _ => name.to_string(),
                        });
                    } else if btype == "text" {
                        if let Some(txt) = b.get("text").and_then(|x| x.as_str()) {
                            if !txt.trim().is_empty() {
                                latest_summary =
                                    Some(txt.trim().lines().next().unwrap_or(txt).to_string());
                            }
                        }
                    }
                }
                if has_tool_use {
                    phase = SessionPhase::Working;
                } else if matches!(
                    message.get("stop_reason").and_then(Value::as_str),
                    Some("end_turn" | "stop_sequence")
                ) {
                    phase = SessionPhase::Completed;
                }
            }
        }
    }

    let sid = session_id.or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
    })?;

    // Require at least a real user prompt or assistant reply to skip metadata-only empty files
    if title.is_none() && latest_summary.is_none() && latest_tool.is_none() {
        return None;
    }

    Some(SessionMutation {
        raw_session_id: sid,
        provider: AgentProvider::ClaudeCode,
        cwd,
        git_branch,
        title,
        phase,
        current_action: if phase == SessionPhase::Working {
            latest_tool.or_else(|| Some("正在思考…".to_string()))
        } else {
            None
        },
        step_count: (step_count > 0).then_some(step_count),
        last_summary: latest_summary,
        host_pid: None,
        host_app: None,
        source_channel: SourceChannel::Passive,
        updated_at_ms: Some(mtime_ms),
        suppress_toast,
    })
}

fn collect_recent_jsonl_files(root: &Path, max_depth: usize, limit: usize) -> Vec<(PathBuf, i64)> {
    let mut found: Vec<(PathBuf, i64)> = Vec::new();
    walk_jsonl(root, 0, max_depth, &mut found);
    found.sort_by(|a, b| b.1.cmp(&a.1));
    found.truncate(limit);
    found
}

fn walk_jsonl(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<(PathBuf, i64)>) {
    if depth > max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            // Skip subagents subdir to avoid noisy child sessions
            if p.file_name().is_some_and(|n| n == "subagents") {
                continue;
            }
            subdirs.push(p);
        } else if p.extension().is_some_and(|ext| ext == "jsonl") {
            if let Some(mt) = file_mtime_ms(&p) {
                out.push((p, mt));
            }
        }
    }
    // Sort subdirectories descending (ideal for YYYY/MM/DD date trees)
    subdirs.sort_by(|a, b| b.cmp(a));
    for sd in subdirs.into_iter().take(12) {
        walk_jsonl(&sd, depth + 1, max_depth, out);
    }
}
