export type AgentProvider = "claude_code" | "codex" | "antigravity";

export type SessionPhase =
  | "working"
  | "waiting_approval"
  | "completed"
  | "failed"
  | "idle";

export type SourceChannel = "hook" | "passive";

export interface AgentSession {
  id: string;
  rawSessionId: string;
  provider: AgentProvider;
  projectName: string;
  cwd: string;
  gitBranch: string | null;
  title: string;
  phase: SessionPhase;
  currentAction: string | null;
  stepCount: number;
  startedAtMs: number;
  turnStartedAtMs: number | null;
  updatedAtMs: number;
  lastDurationMs: number | null;
  lastSummary: string | null;
  hostPid: number | null;
  hostApp: string | null;
  sourceChannel: SourceChannel;
}

export type ToastKind = "completed" | "waiting_approval" | "failed";

export interface ToastPayload {
  id: string;
  sessionId: string;
  provider: AgentProvider;
  projectName: string;
  title: string;
  kind: ToastKind;
  message: string;
  durationMs: number | null;
  createdAtMs: number;
  hostPid: number | null;
  hostApp: string | null;
  cwd: string;
}

export type HudMode = "capsule" | "top_island";
export type ToastCorner = "top_right" | "bottom_right";

export interface AppSettings {
  hudMode: HudMode;
  toastCorner: ToastCorner;
  soundEnabled: boolean;
  minCompletionSeconds: number;
  capsulePos: [number, number] | null;
}

export interface HookProviderStatus {
  provider: AgentProvider;
  installed: boolean;
  configPath: string;
  configExists: boolean;
  backupExists: boolean;
  note: string | null;
}

export interface DashboardSnapshot {
  sessions: AgentSession[];
  settings: AppSettings;
  hooks: HookProviderStatus[];
  workingCount: number;
  attentionCount: number;
}
