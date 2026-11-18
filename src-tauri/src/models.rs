use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentProvider {
    ClaudeCode,
    Codex,
    Antigravity,
}

impl AgentProvider {
    pub fn as_key(&self) -> &'static str {
        match self {
            AgentProvider::ClaudeCode => "claude_code",
            AgentProvider::Codex => "codex",
            AgentProvider::Antigravity => "antigravity",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            AgentProvider::ClaudeCode => "Claude Code",
            AgentProvider::Codex => "Codex",
            AgentProvider::Antigravity => "Antigravity",
        }
    }

    pub fn from_cli_arg(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude_code" | "claude-code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "antigravity" | "agy" => Some(Self::Antigravity),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionPhase {
    Working,
    WaitingApproval,
    Completed,
    Failed,
    Idle,
}

impl SessionPhase {
    pub fn sort_rank(&self) -> u8 {
        match self {
            SessionPhase::WaitingApproval => 0,
            SessionPhase::Working => 1,
            SessionPhase::Failed => 2,
            SessionPhase::Completed => 3,
            SessionPhase::Idle => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceChannel {
    Hook,
    Passive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub raw_session_id: String,
    pub provider: AgentProvider,
    pub project_name: String,
    pub cwd: String,
    pub git_branch: Option<String>,
    pub title: String,
    pub phase: SessionPhase,
    pub current_action: Option<String>,
    pub step_count: u32,
    pub started_at_ms: i64,
    pub turn_started_at_ms: Option<i64>,
    pub updated_at_ms: i64,
    pub last_duration_ms: Option<u64>,
    pub last_summary: Option<String>,
    pub host_pid: Option<u32>,
    pub host_app: Option<String>,
    pub source_channel: SourceChannel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToastKind {
    Completed,
    WaitingApproval,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastPayload {
    pub id: String,
    pub session_id: String,
    pub provider: AgentProvider,
    pub project_name: String,
    pub title: String,
    pub kind: ToastKind,
    pub message: String,
    pub duration_ms: Option<u64>,
    pub created_at_ms: i64,
    pub host_pid: Option<u32>,
    pub host_app: Option<String>,
    pub cwd: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HudMode {
    Capsule,
    TopIsland,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToastCorner {
    TopRight,
    BottomRight,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub hud_mode: HudMode,
    pub toast_corner: ToastCorner,
    pub sound_enabled: bool,
    pub min_completion_seconds: u64,
    pub capsule_pos: Option<(f64, f64)>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            hud_mode: HudMode::Capsule,
            toast_corner: ToastCorner::TopRight,
            sound_enabled: true,
            min_completion_seconds: 5,
            capsule_pos: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookProviderStatus {
    pub provider: AgentProvider,
    pub installed: bool,
    pub config_path: String,
    pub config_exists: bool,
    pub backup_exists: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSnapshot {
    pub sessions: Vec<AgentSession>,
    pub settings: AppSettings,
    pub hooks: Vec<HookProviderStatus>,
    pub working_count: usize,
    pub attention_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookWireEnvelope {
    pub agent: String,
    pub event_hint: Option<String>,
    pub client_pid: u32,
    pub parent_pid: Option<u32>,
    pub env_hints: HashMap<String, String>,
    pub payload: serde_json::Value,
}
