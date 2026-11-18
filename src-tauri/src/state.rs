use crate::hooks_installer;
use crate::models::{
    AgentProvider, AgentSession, AppSettings, DashboardSnapshot, SessionPhase, SourceChannel,
    ToastKind, ToastPayload,
};
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

pub struct SessionMutation {
    pub raw_session_id: String,
    pub provider: AgentProvider,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub title: Option<String>,
    pub phase: SessionPhase,
    pub current_action: Option<String>,
    pub step_count: Option<u32>,
    pub last_summary: Option<String>,
    pub host_pid: Option<u32>,
    pub host_app: Option<String>,
    pub source_channel: SourceChannel,
    pub updated_at_ms: Option<i64>,
    /// Allow passive watcher to hydrate historical sessions on startup without firing stale Toasts
    pub suppress_toast: bool,
}

struct InnerState {
    sessions: HashMap<String, AgentSession>,
    settings: AppSettings,
    /// Tracks `{session_id}:{kind}:{turn_started_at}` so we never fire duplicate Toasts for the same turn
    notified_keys: HashSet<String>,
}

#[derive(Clone)]
pub struct AppState {
    inner: Arc<RwLock<InnerState>>,
    home_dir: PathBuf,
    pub auth_token: String,
}

impl AppState {
    pub fn new() -> Self {
        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let settings = Self::load_settings_from(&home_dir);
        Self {
            inner: Arc::new(RwLock::new(InnerState {
                sessions: HashMap::new(),
                settings,
                notified_keys: HashSet::new(),
            })),
            home_dir,
            auth_token: Uuid::new_v4().to_string(),
        }
    }

    pub fn home_dir(&self) -> &PathBuf {
        &self.home_dir
    }

    pub fn app_data_dir(&self) -> PathBuf {
        self.home_dir.join(".agent-ing")
    }

    fn load_settings_from(home: &PathBuf) -> AppSettings {
        let path = home.join(".agent-ing").join("settings.json");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<AppSettings>(&s).ok())
            .unwrap_or_default()
    }

    fn persist_settings(&self, settings: &AppSettings) {
        let dir = self.app_data_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(settings) {
            let _ = std::fs::write(dir.join("settings.json"), json);
        }
    }

    pub fn settings(&self) -> AppSettings {
        self.inner.read().unwrap().settings.clone()
    }

    pub fn update_settings(&self, app: &AppHandle, next: AppSettings) -> DashboardSnapshot {
        {
            let mut guard = self.inner.write().unwrap();
            guard.settings = next.clone();
        }
        self.persist_settings(&next);
        let snap = self.snapshot();
        let _ = app.emit("sessions-updated", &snap);
        snap
    }

    pub fn save_capsule_pos(&self, x: f64, y: f64) {
        let next = {
            let mut guard = self.inner.write().unwrap();
            guard.settings.capsule_pos = Some((x, y));
            guard.settings.clone()
        };
        self.persist_settings(&next);
    }

    pub fn snapshot(&self) -> DashboardSnapshot {
        let now_ms = Utc::now().timestamp_millis();
        let guard = self.inner.read().unwrap();
        let mut sessions: Vec<AgentSession> = guard
            .sessions
            .values()
            .filter(|s| {
                // Keep active sessions always, and keep completed/idle sessions from the last 12 hours
                matches!(s.phase, SessionPhase::Working | SessionPhase::WaitingApproval)
                    || (now_ms - s.updated_at_ms) < 12 * 3600 * 1000
            })
            .cloned()
            .collect();

        sessions.sort_by(|a, b| {
            a.phase
                .sort_rank()
                .cmp(&b.phase.sort_rank())
                .then_with(|| b.started_at_ms.cmp(&a.started_at_ms))
                .then_with(|| a.id.cmp(&b.id))
        });
        sessions.truncate(25);

        let working_count = sessions
            .iter()
            .filter(|s| s.phase == SessionPhase::Working)
            .count();
        let attention_count = sessions
            .iter()
            .filter(|s| s.phase == SessionPhase::WaitingApproval)
            .count();

        DashboardSnapshot {
            sessions,
            settings: guard.settings.clone(),
            hooks: hooks_installer::get_all_hooks_status(&self.home_dir),
            working_count,
            attention_count,
        }
    }

    pub fn session(&self, session_id: &str) -> Option<AgentSession> {
        self.inner.read().unwrap().sessions.get(session_id).cloned()
    }

    pub fn dismiss_session(&self, app: &AppHandle, session_id: &str) -> DashboardSnapshot {
        {
            let mut guard = self.inner.write().unwrap();
            guard.sessions.remove(session_id);
        }
        let snap = self.snapshot();
        let _ = app.emit("sessions-updated", &snap);
        snap
    }

    pub fn apply_mutation(&self, app: Option<&AppHandle>, m: SessionMutation) -> Option<ToastPayload> {
        let now_ms = m.updated_at_ms.unwrap_or_else(|| Utc::now().timestamp_millis());
        let id = format!("{}:{}", m.provider.as_key(), m.raw_session_id);

        let mut toast_to_emit: Option<ToastPayload> = None;

        {
            let mut guard = self.inner.write().unwrap();
            let min_completion_ms = guard.settings.min_completion_seconds * 1000;
            let prev = guard.sessions.get(&id).cloned();

            // If this session is actively driven by real-time Hooks recently (<15s),
            // don't let a slightly lagging passive poll downgrade `WaitingApproval` or `Completed` back to stale states.
            if let Some(ref existing) = prev {
                if existing.source_channel == SourceChannel::Hook
                    && m.source_channel == SourceChannel::Passive
                    && (Utc::now().timestamp_millis() - existing.updated_at_ms) < 12_000
                {
                    // Still allow passive watcher to enrich missing title, step_count, or current_action!
                    let mut enriched = existing.clone();
                    if (enriched.title.is_empty() || enriched.title == "新会话")
                        && m.title.as_deref().is_some_and(|t| !t.is_empty())
                    {
                        enriched.title = m.title.unwrap();
                    }
                    if let Some(sc) = m.step_count {
                        if sc > enriched.step_count {
                            enriched.step_count = sc;
                        }
                    }
                    if enriched.current_action.is_none() && m.current_action.is_some() {
                        enriched.current_action = m.current_action;
                    }
                    if enriched.last_summary.is_none() && m.last_summary.is_some() {
                        enriched.last_summary = m.last_summary;
                    }
                    if m.provider == AgentProvider::Codex && m.host_app.as_deref() == Some("Codex") {
                        enriched.host_app = m.host_app;
                    }
                    guard.sessions.insert(id, enriched);
                    return None;
                }
            }

            let cwd = m
                .cwd
                .filter(|c| !c.is_empty())
                .or_else(|| prev.as_ref().map(|p| p.cwd.clone()))
                .unwrap_or_default();

            let project_name = extract_project_name(&cwd)
                .or_else(|| prev.as_ref().map(|p| p.project_name.clone()))
                .unwrap_or_else(|| m.provider.display_name().to_string());

            let title = m
                .title
                .filter(|t| !t.trim().is_empty())
                .or_else(|| prev.as_ref().map(|p| p.title.clone()))
                .unwrap_or_else(|| "执行任务中…".to_string());

            let started_at_ms = prev.as_ref().map(|p| p.started_at_ms).unwrap_or(now_ms);
            let prev_phase = prev.as_ref().map(|p| p.phase);

            let turn_started_at_ms = match m.phase {
                SessionPhase::Working | SessionPhase::WaitingApproval => prev
                    .as_ref()
                    .and_then(|p| p.turn_started_at_ms)
                    .or(Some(now_ms)),
                _ => prev.as_ref().and_then(|p| p.turn_started_at_ms),
            };

            let computed_duration_ms = turn_started_at_ms
                .map(|ts| now_ms.saturating_sub(ts) as u64)
                .or_else(|| prev.as_ref().and_then(|p| p.last_duration_ms));

            let step_count = match (m.step_count, prev.as_ref().map(|p| p.step_count)) {
                (Some(next_sc), Some(old_sc)) => next_sc.max(old_sc),
                (Some(next_sc), None) => next_sc,
                (None, Some(old_sc)) => {
                    if m.phase == SessionPhase::Working && m.current_action.is_some() {
                        old_sc + 1
                    } else {
                        old_sc
                    }
                }
                (None, None) => u32::from(m.phase == SessionPhase::Working),
            };

            let current_action = match m.phase {
                SessionPhase::Working | SessionPhase::WaitingApproval => m
                    .current_action
                    .or_else(|| prev.as_ref().and_then(|p| p.current_action.clone())),
                SessionPhase::Completed | SessionPhase::Idle => None,
                SessionPhase::Failed => m
                    .current_action
                    .or_else(|| prev.as_ref().and_then(|p| p.current_action.clone())),
            };

            let last_summary = m
                .last_summary
                .filter(|s| !s.trim().is_empty())
                .or_else(|| prev.as_ref().and_then(|p| p.last_summary.clone()));

            let host_pid = m.host_pid.or_else(|| prev.as_ref().and_then(|p| p.host_pid));
            let host_app = m
                .host_app
                .or_else(|| prev.as_ref().and_then(|p| p.host_app.clone()));
            let git_branch = m
                .git_branch
                .or_else(|| prev.as_ref().and_then(|p| p.git_branch.clone()));

            // Evaluate Toast trigger transitions
            if !m.suppress_toast {
                let turn_anchor = turn_started_at_ms.unwrap_or(started_at_ms);
                match m.phase {
                    SessionPhase::WaitingApproval
                        if prev_phase != Some(SessionPhase::WaitingApproval) =>
                    {
                        let dedup = format!("{id}:approval:{}:{:?}", turn_anchor, current_action);
                        if guard.notified_keys.insert(dedup) {
                            toast_to_emit = Some(ToastPayload {
                                id: Uuid::new_v4().to_string(),
                                session_id: id.clone(),
                                provider: m.provider,
                                project_name: project_name.clone(),
                                title: title.clone(),
                                kind: ToastKind::WaitingApproval,
                                message: current_action
                                    .clone()
                                    .unwrap_or_else(|| "需要你确认工具权限或回答问题".to_string()),
                                duration_ms: computed_duration_ms,
                                created_at_ms: now_ms,
                                host_pid,
                                host_app: host_app.clone(),
                                cwd: cwd.clone(),
                            });
                        }
                    }
                    SessionPhase::Completed
                        if matches!(
                            prev_phase,
                            Some(SessionPhase::Working | SessionPhase::WaitingApproval)
                        ) || (prev_phase.is_none() && m.source_channel == SourceChannel::Hook) =>
                    {
                        let dur = computed_duration_ms.unwrap_or(0);
                        let passes_threshold = dur >= min_completion_ms || prev_phase.is_none();
                        let dedup = format!("{id}:completed:{turn_anchor}");
                        if passes_threshold && guard.notified_keys.insert(dedup) {
                            toast_to_emit = Some(ToastPayload {
                                id: Uuid::new_v4().to_string(),
                                session_id: id.clone(),
                                provider: m.provider,
                                project_name: project_name.clone(),
                                title: title.clone(),
                                kind: ToastKind::Completed,
                                message: last_summary
                                    .clone()
                                    .unwrap_or_else(|| "本轮 Agent 执行已完成".to_string()),
                                duration_ms: Some(dur),
                                created_at_ms: now_ms,
                                host_pid,
                                host_app: host_app.clone(),
                                cwd: cwd.clone(),
                            });
                        }
                    }
                    SessionPhase::Failed if prev_phase != Some(SessionPhase::Failed) => {
                        let dedup = format!("{id}:failed:{turn_anchor}");
                        if guard.notified_keys.insert(dedup) {
                            toast_to_emit = Some(ToastPayload {
                                id: Uuid::new_v4().to_string(),
                                session_id: id.clone(),
                                provider: m.provider,
                                project_name: project_name.clone(),
                                title: title.clone(),
                                kind: ToastKind::Failed,
                                message: last_summary
                                    .clone()
                                    .or_else(|| current_action.clone())
                                    .unwrap_or_else(|| "Agent 执行异常中断".to_string()),
                                duration_ms: computed_duration_ms,
                                created_at_ms: now_ms,
                                host_pid,
                                host_app: host_app.clone(),
                                cwd: cwd.clone(),
                            });
                        }
                    }
                    _ => {}
                }
            }

            let next_turn_started = if matches!(
                m.phase,
                SessionPhase::Completed | SessionPhase::Failed | SessionPhase::Idle
            ) {
                None
            } else {
                turn_started_at_ms
            };

            let session = AgentSession {
                id: id.clone(),
                raw_session_id: m.raw_session_id,
                provider: m.provider,
                project_name,
                cwd,
                git_branch,
                title: truncate_chars(&title, 90),
                phase: m.phase,
                current_action: current_action.map(|a| truncate_chars(&a, 85)),
                step_count,
                started_at_ms,
                turn_started_at_ms: next_turn_started,
                updated_at_ms: now_ms,
                last_duration_ms: computed_duration_ms,
                last_summary: last_summary.map(|s| truncate_chars(&s, 140)),
                host_pid,
                host_app,
                source_channel: m.source_channel,
            };

            guard.sessions.insert(id, session);
        }

        if let Some(app_handle) = app {
            let snap = self.snapshot();
            let _ = app_handle.emit("sessions-updated", &snap);
            if let Some(ref toast) = toast_to_emit {
                crate::platform::prepare_toast_window(app_handle, &snap.settings);
                let _ = app_handle.emit("toast-push", toast);
            }
        }

        toast_to_emit
    }
}

pub fn extract_project_name(cwd: &str) -> Option<String> {
    let trimmed = cwd.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .rsplit(['/', '\\'])
        .find(|seg| !seg.is_empty())
        .map(|s| s.to_string())
}

pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    let clean = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max_chars {
        clean
    } else {
        let mut out: String = clean.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_updates_do_not_reorder_sessions_in_the_same_phase() {
        let state = AppState::new();
        let update = |id: &str, time| {
            state.apply_mutation(
                None,
                SessionMutation {
                    raw_session_id: id.into(),
                    provider: AgentProvider::Codex,
                    cwd: None,
                    git_branch: None,
                    title: None,
                    phase: SessionPhase::Working,
                    current_action: None,
                    step_count: None,
                    last_summary: None,
                    host_pid: None,
                    host_app: None,
                    source_channel: SourceChannel::Passive,
                    updated_at_ms: Some(time),
                    suppress_toast: true,
                },
            );
        };
        update("first", 1);
        update("second", 2);
        update("first", 3);
        let sessions = state.snapshot().sessions;
        assert_eq!(sessions[0].raw_session_id, "second");
        assert_eq!(sessions[1].raw_session_id, "first");
        assert_eq!(sessions[1].updated_at_ms, 3);
    }

    #[test]
    fn state_machine_emits_approval_and_completion_toasts_once() {
        let state = AppState::new();
        let t0 = 1_700_000_000_000i64;

        let toast1 = state.apply_mutation(
            None,
            SessionMutation {
                raw_session_id: "sess-1".into(),
                provider: AgentProvider::ClaudeCode,
                cwd: Some("/Users/dev/my-app".into()),
                git_branch: Some("main".into()),
                title: Some("重构登录接口并补齐单测".into()),
                phase: SessionPhase::Working,
                current_action: Some("Bash: pnpm test".into()),
                step_count: Some(3),
                last_summary: None,
                host_pid: Some(1234),
                host_app: Some("Ghostty".into()),
                source_channel: SourceChannel::Hook,
                updated_at_ms: Some(t0),
                suppress_toast: false,
            },
        );
        assert!(toast1.is_none());

        // Transition to WaitingApproval -> immediate Toast
        let toast2 = state.apply_mutation(
            None,
            SessionMutation {
                raw_session_id: "sess-1".into(),
                provider: AgentProvider::ClaudeCode,
                cwd: None,
                git_branch: None,
                title: None,
                phase: SessionPhase::WaitingApproval,
                current_action: Some("等待确认: rm -rf dist".into()),
                step_count: Some(4),
                last_summary: None,
                host_pid: None,
                host_app: None,
                source_channel: SourceChannel::Hook,
                updated_at_ms: Some(t0 + 3_000),
                suppress_toast: false,
            },
        );
        assert!(toast2.is_some());
        assert_eq!(toast2.unwrap().kind, ToastKind::WaitingApproval);

        // Transition to Completed after 8s (>= 5s default threshold) -> Completion Toast
        let toast3 = state.apply_mutation(
            None,
            SessionMutation {
                raw_session_id: "sess-1".into(),
                provider: AgentProvider::ClaudeCode,
                cwd: None,
                git_branch: None,
                title: None,
                phase: SessionPhase::Completed,
                current_action: None,
                step_count: Some(6),
                last_summary: Some("全部 18 个单元测试通过，登录模块重构完成。".into()),
                host_pid: None,
                host_app: None,
                source_channel: SourceChannel::Hook,
                updated_at_ms: Some(t0 + 8_500),
                suppress_toast: false,
            },
        );
        assert!(toast3.is_some());
        let tp = toast3.unwrap();
        assert_eq!(tp.kind, ToastKind::Completed);
        assert_eq!(tp.project_name, "my-app");
        assert_eq!(tp.duration_ms, Some(8_500));
    }
}
