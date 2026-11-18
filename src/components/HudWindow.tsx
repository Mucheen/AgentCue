import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { AnimatePresence, motion } from "framer-motion";
import {
  CheckCircle2,
  ChevronDown,
  ExternalLink,
  FolderOpen,
  GitBranch,
  Play,
  Settings,
  ShieldAlert,
  Sparkles,
  Volume2,
  VolumeX,
  X,
  XCircle,
} from "lucide-react";
import type {
  AgentProvider,
  AgentSession,
  AppSettings,
  DashboardSnapshot,
  HudMode,
  ToastCorner,
} from "../types";
import {
  formatDuration,
  formatRelativeAgo,
  PROVIDER_META,
  ProviderIcon,
} from "./ProviderBadge";
import { playToastChime } from "../utils/sound";

type FilterTab = "all" | "working" | "attention" | "done";

const DEFAULT_SNAPSHOT: DashboardSnapshot = {
  sessions: [],
  settings: {
    hudMode: "capsule",
    toastCorner: "top_right",
    soundEnabled: true,
    minCompletionSeconds: 5,
    capsulePos: null,
  },
  hooks: [],
  workingCount: 0,
  attentionCount: 0,
};

export const HudWindow: React.FC = () => {
  const [snapshot, setSnapshot] = useState<DashboardSnapshot>(DEFAULT_SNAPSHOT);
  const [expanded, setExpanded] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [filter, setFilter] = useState<FilterTab>("all");
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [busyHookProvider, setBusyHookProvider] = useState<AgentProvider | null>(
    null
  );
  const [statusBanner, setStatusBanner] = useState<string | null>(null);

  const dragOriginRef = useRef<{ x: number; y: number; dragged: boolean } | null>(
    null
  );

  // Initial snapshot + Tauri event subscriptions
  useEffect(() => {
    invoke<DashboardSnapshot>("get_snapshot")
      .then((snap) => setSnapshot(snap))
      .catch(() => {});

    const unlistenPromises = [
      listen<DashboardSnapshot>("sessions-updated", (ev) => {
        setSnapshot(ev.payload);
      }),
      listen("hud-toggle-expand", () => {
        setExpanded((prev) => {
          const next = !prev;
          void invoke("set_hud_expanded", { expanded: next });
          return next;
        });
      }),
      listen("hud-force-collapse", () => {
        setExpanded(false);
        setShowSettings(false);
      }),
    ];

    return () => {
      unlistenPromises.forEach((p) => void p.then((fn) => fn()));
    };
  }, []);

  // 1-second ticker for live elapsed timers
  useEffect(() => {
    const id = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  // Persist dragged capsule position when window moves in collapsed capsule mode
  useEffect(() => {
    const win = getCurrentWebviewWindow();
    let saveTimer: number | undefined;
    const unlistenPromise = win.onMoved(({ payload }) => {
      if (expanded || snapshot.settings.hudMode !== "capsule") return;
      window.clearTimeout(saveTimer);
      saveTimer = window.setTimeout(async () => {
        try {
          const scale = await win.scaleFactor();
          await invoke("save_capsule_pos", {
            x: payload.x / scale,
            y: payload.y / scale,
          });
        } catch {
          // ignore outside Tauri
        }
      }, 260);
    });

    return () => {
      window.clearTimeout(saveTimer);
      void unlistenPromise.then((fn) => fn());
    };
  }, [expanded, snapshot.settings.hudMode]);

  const toggleExpand = useCallback((nextState: boolean) => {
    setExpanded(nextState);
    if (!nextState) setShowSettings(false);
    void invoke("set_hud_expanded", { expanded: nextState });
  }, []);

  // Reliable cross-platform click-vs-drag handler for collapsed capsule
  const handleCapsulePointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    dragOriginRef.current = { x: e.clientX, y: e.clientY, dragged: false };
  };

  const handleCapsulePointerMove = (e: React.PointerEvent) => {
    const orig = dragOriginRef.current;
    if (!orig || orig.dragged) return;
    if (snapshot.settings.hudMode === "top_island") return;
    const dist = Math.hypot(e.clientX - orig.x, e.clientY - orig.y);
    if (dist > 5) {
      orig.dragged = true;
      void getCurrentWebviewWindow().startDragging();
    }
  };

  const handleCapsuleClick = (event: React.MouseEvent) => {
    const orig = dragOriginRef.current;
    dragOriginRef.current = null;
    if (event.detail === 0 || !orig?.dragged) {
      toggleExpand(true);
    }
  };

  const handleHeaderDragDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest("button")) return;
    if (snapshot.settings.hudMode === "capsule") {
      void getCurrentWebviewWindow().startDragging();
    }
  };

  const handleUpdateSettings = async (patch: Partial<AppSettings>) => {
    const next: AppSettings = { ...snapshot.settings, ...patch };
    try {
      const updated = await invoke<DashboardSnapshot>("update_settings", {
        settings: next,
        isExpanded: expanded,
      });
      setSnapshot(updated);
    } catch {
      setSnapshot((s) => ({ ...s, settings: next }));
    }
  };

  const handleToggleHook = async (provider: AgentProvider, installed: boolean) => {
    setBusyHookProvider(provider);
    setStatusBanner(null);
    try {
      const cmd = installed ? "uninstall_hooks" : "install_hooks";
      const updated = await invoke<DashboardSnapshot>(cmd, { provider });
      setSnapshot(updated);
      setStatusBanner(
        installed
          ? `已从 ${PROVIDER_META[provider].label} 安全移除钩子`
          : `已接入 ${PROVIDER_META[provider].label} 实时钩子（原配置已自动备份）`
      );
    } catch (err) {
      setStatusBanner(`操作失败: ${String(err)}`);
    } finally {
      setBusyHookProvider(null);
    }
  };

  const filteredSessions = useMemo(() => {
    return snapshot.sessions.filter((s) => {
      if (filter === "working") return s.phase === "working";
      if (filter === "attention")
        return s.phase === "waiting_approval" || s.phase === "failed";
      if (filter === "done")
        return s.phase === "completed" || s.phase === "idle";
      return true;
    });
  }, [snapshot.sessions, filter]);

  const activeLeadSession = useMemo(() => {
    return (
      snapshot.sessions.find((s) => s.phase === "waiting_approval") ||
      snapshot.sessions.find((s) => s.phase === "working") ||
      snapshot.sessions[0] ||
      null
    );
  }, [snapshot.sessions]);

  const isTopIsland = snapshot.settings.hudMode === "top_island";

  // ---------------------------------------------------------------------------
  // Collapsed Floating Capsule / Top Island View
  // ---------------------------------------------------------------------------
  if (!expanded) {
    const hasAttention = snapshot.attentionCount > 0;
    const hasWorking = snapshot.workingCount > 0;

    return (
      <div
        className={`w-full h-full flex items-center justify-center ${
          isTopIsland ? "px-2 pt-0.5 pb-2" : "p-2"
        }`}
      >
        <motion.button
          type="button"
          aria-label="展开进度面板"
          aria-expanded={false}
          initial={{ scale: 0.94, opacity: 0 }}
          animate={{ scale: 1, opacity: 1 }}
          transition={{ type: "spring", stiffness: 420, damping: 28 }}
          onPointerDown={handleCapsulePointerDown}
          onPointerMove={handleCapsulePointerMove}
          onClick={handleCapsuleClick}
          className={`w-full h-full text-left cursor-pointer flex items-center justify-between gap-2.5 px-3.5 backdrop-blur-xl transition-colors ${
            isTopIsland
              ? "rounded-b-2xl rounded-t-md bg-[#0D0F14]/96 border border-white/12 shadow-[0_8px_24px_rgba(0,0,0,0.55)]"
              : hasAttention
              ? "rounded-full bg-[#16130E]/95 border border-amber-400/45 shadow-[0_8px_26px_rgba(245,158,11,0.28)]"
              : hasWorking
              ? "rounded-full bg-[#10141C]/94 border border-emerald-400/35 shadow-[0_10px_28px_rgba(0,0,0,0.5)]"
              : "rounded-full bg-[#11131A]/92 border border-white/12 hover:border-white/25 shadow-[0_10px_26px_rgba(0,0,0,0.48)]"
          }`}
        >
          {/* Left live radar indicator */}
          <div className="flex items-center gap-2 min-w-0">
            <span className="relative flex h-2.5 w-2.5 shrink-0">
              {(hasAttention || hasWorking) && (
                <span
                  className={`animate-ping absolute inline-flex h-full w-full rounded-full opacity-75 ${
                    hasAttention ? "bg-amber-400" : "bg-emerald-400"
                  }`}
                />
              )}
              <span
                className={`relative inline-flex rounded-full h-2.5 w-2.5 ${
                  hasAttention
                    ? "bg-amber-400"
                    : hasWorking
                    ? "bg-emerald-400"
                    : "bg-slate-400/70"
                }`}
              />
            </span>

            <div className="flex flex-col min-w-0 leading-tight">
              <span className="text-[12px] font-semibold text-white/95 truncate">
                {hasAttention
                  ? `${snapshot.attentionCount} 个待你确认`
                  : hasWorking
                  ? `${snapshot.workingCount} 个运行中`
                  : snapshot.sessions.length > 0
                  ? `就绪 · ${snapshot.sessions.length} 会话`
                  : "AgentCue 就绪"}
              </span>
              {activeLeadSession && (
                <span className="text-[10.5px] text-white/55 truncate">
                  {activeLeadSession.currentAction ||
                    activeLeadSession.projectName}
                </span>
              )}
            </div>
          </div>

          {/* Right active provider badges */}
          <div className="flex items-center gap-1 shrink-0">
            {(["claude_code", "codex", "antigravity"] as AgentProvider[]).map(
              (prov) => {
                const activeForProv = snapshot.sessions.some(
                  (s) =>
                    s.provider === prov &&
                    (s.phase === "working" || s.phase === "waiting_approval")
                );
                return (
                  <div
                    key={prov}
                    className={`p-1 rounded-full transition-opacity ${
                      activeForProv
                        ? "bg-white/10 opacity-100"
                        : "opacity-35"
                    }`}
                    title={PROVIDER_META[prov].label}
                  >
                    <ProviderIcon provider={prov} size={12} />
                  </div>
                );
              }
            )}
          </div>
        </motion.button>
      </div>
    );
  }

  // ---------------------------------------------------------------------------
  // Expanded Progress & Settings Panel View
  // ---------------------------------------------------------------------------
  return (
    <div className="w-full h-full p-2.5">
      <motion.div
        initial={{ opacity: 0, scale: 0.96, y: -6 }}
        animate={{ opacity: 1, scale: 1, y: 0 }}
        transition={{ type: "spring", stiffness: 420, damping: 30 }}
        className="w-full h-full flex flex-col rounded-2xl bg-[#10131B]/96 backdrop-blur-2xl border border-white/12 shadow-[0_16px_40px_rgba(0,0,0,0.62)] overflow-hidden"
      >
        {/* Top Header */}
        <div
          onPointerDown={handleHeaderDragDown}
          className="flex items-center justify-between px-3.5 py-2.5 border-b border-white/8 bg-white/[0.02] cursor-grab active:cursor-grabbing"
        >
          <div className="flex items-center gap-2">
            <div className="h-2.5 w-2.5 rounded-full bg-emerald-400 shadow-[0_0_10px_rgba(52,211,153,0.8)]" />
            <span className="text-[13px] font-semibold tracking-tight text-white">
              AgentCue
            </span>
            <span className="text-[11px] px-1.5 py-0.5 rounded-md bg-white/8 text-white/65">
              {snapshot.workingCount > 0
                ? `${snapshot.workingCount} 运行中`
                : "空闲监控中"}
            </span>
          </div>

          <div className="flex items-center gap-1">
            <button
              onClick={() => void invoke("trigger_demo")}
              className="flex items-center gap-1 px-2 py-1 rounded-lg text-[11px] font-medium bg-emerald-500/15 hover:bg-emerald-500/25 text-emerald-300 border border-emerald-500/25 transition-colors cursor-pointer"
              title="模拟多 Agent 进度与完成弹窗"
            >
              <Play className="w-3 h-3 fill-current" />
              演示
            </button>
            <button
              onClick={() => setShowSettings((v) => !v)}
              className={`p-1.5 rounded-lg transition-colors cursor-pointer ${
                showSettings
                  ? "bg-white/16 text-white"
                  : "text-white/60 hover:text-white hover:bg-white/8"
              }`}
              title="Hook 接入与偏好设置"
            >
              <Settings className="w-3.5 h-3.5" />
            </button>
            <button
              onClick={() => toggleExpand(false)}
              className="p-1.5 rounded-lg text-white/60 hover:text-white hover:bg-white/8 transition-colors cursor-pointer"
              title="收起为悬浮胶囊"
            >
              <ChevronDown className="w-4 h-4" />
            </button>
          </div>
        </div>

        {/* Body: Either Settings View or Live Session List */}
        {showSettings ? (
          <div className="flex-1 overflow-y-auto px-3.5 py-3 space-y-3.5">
            {statusBanner && (
              <div className="px-2.5 py-2 rounded-xl bg-emerald-500/12 border border-emerald-500/25 text-[11.5px] text-emerald-200 flex items-center justify-between gap-2">
                <span>{statusBanner}</span>
                <button
                  onClick={() => setStatusBanner(null)}
                  className="text-white/50 hover:text-white cursor-pointer"
                >
                  <X className="w-3.5 h-3.5" />
                </button>
              </div>
            )}

            {/* Section 1: Safe Hook Installer */}
            <div className="space-y-2">
              <div className="text-[11px] font-semibold uppercase tracking-wider text-white/45">
                三大 Agent 增强 Hook（非破坏性合并）
              </div>
              {snapshot.hooks.map((h) => {
                const meta = PROVIDER_META[h.provider];
                const isBusy = busyHookProvider === h.provider;
                return (
                  <div
                    key={h.provider}
                    className="p-2.5 rounded-xl bg-white/[0.035] border border-white/8 space-y-1.5"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <div className="flex items-center gap-2">
                        <ProviderIcon provider={h.provider} size={15} />
                        <span className="text-[12.5px] font-medium text-white">
                          {meta.label}
                        </span>
                        <span
                          className={`text-[10px] px-1.5 py-0.5 rounded-full ${
                            h.installed
                              ? "bg-emerald-500/15 text-emerald-300 border border-emerald-500/30"
                              : "bg-white/8 text-white/55"
                          }`}
                        >
                          {h.installed ? "Hook 已启用" : "只读日志监听"}
                        </span>
                      </div>
                      <button
                        disabled={isBusy}
                        onClick={() =>
                          void handleToggleHook(h.provider, h.installed)
                        }
                        className={`px-2.5 py-1 rounded-lg text-[11px] font-medium transition-colors cursor-pointer disabled:opacity-50 ${
                          h.installed
                            ? "bg-white/8 hover:bg-rose-500/20 text-white/75 hover:text-rose-200"
                            : "bg-indigo-500/25 hover:bg-indigo-500/35 text-indigo-200 border border-indigo-400/30"
                        }`}
                      >
                        {isBusy
                          ? "处理中…"
                          : h.installed
                          ? "安全移除"
                          : "一键接入"}
                      </button>
                    </div>
                    {h.note && (
                      <p className="text-[10.5px] text-white/48 leading-relaxed">
                        {h.note}
                      </p>
                    )}
                  </div>
                );
              })}
            </div>

            {/* Section 2: Appearance & Notification Preferences */}
            <div className="space-y-2.5 pt-1 border-t border-white/8">
              <div className="text-[11px] font-semibold uppercase tracking-wider text-white/45">
                悬浮形态与弹窗设置
              </div>

              <div className="flex items-center justify-between text-[12px]">
                <span className="text-white/75">常驻形态</span>
                <div className="flex bg-white/6 rounded-lg p-0.5">
                  {(
                    [
                      ["capsule", "自由悬浮胶囊"],
                      ["top_island", "顶部居中灵动岛"],
                    ] as [HudMode, string][]
                  ).map(([mode, label]) => (
                    <button
                      key={mode}
                      onClick={() => void handleUpdateSettings({ hudMode: mode })}
                      className={`px-2.5 py-1 rounded-md text-[11px] transition-colors cursor-pointer ${
                        snapshot.settings.hudMode === mode
                          ? "bg-white/16 text-white font-medium"
                          : "text-white/55 hover:text-white/85"
                      }`}
                    >
                      {label}
                    </button>
                  ))}
                </div>
              </div>

              <div className="flex items-center justify-between text-[12px]">
                <span className="text-white/75">完成弹窗位置</span>
                <div className="flex bg-white/6 rounded-lg p-0.5">
                  {(
                    [
                      ["top_right", "右上角"],
                      ["bottom_right", "右下角"],
                    ] as [ToastCorner, string][]
                  ).map(([corner, label]) => (
                    <button
                      key={corner}
                      onClick={() =>
                        void handleUpdateSettings({ toastCorner: corner })
                      }
                      className={`px-2.5 py-1 rounded-md text-[11px] transition-colors cursor-pointer ${
                        snapshot.settings.toastCorner === corner
                          ? "bg-white/16 text-white font-medium"
                          : "text-white/55 hover:text-white/85"
                      }`}
                    >
                      {label}
                    </button>
                  ))}
                </div>
              </div>

              <div className="flex items-center justify-between text-[12px]">
                <span className="text-white/75">完成弹窗最小耗时阈值</span>
                <div className="flex bg-white/6 rounded-lg p-0.5">
                  {[0, 5, 15, 30].map((sec) => (
                    <button
                      key={sec}
                      onClick={() =>
                        void handleUpdateSettings({ minCompletionSeconds: sec })
                      }
                      className={`px-2 py-1 rounded-md text-[11px] transition-colors cursor-pointer ${
                        snapshot.settings.minCompletionSeconds === sec
                          ? "bg-white/16 text-white font-medium"
                          : "text-white/55 hover:text-white/85"
                      }`}
                    >
                      {sec === 0 ? "每次" : `${sec}s`}
                    </button>
                  ))}
                </div>
              </div>

              <div className="flex items-center justify-between text-[12px]">
                <span className="text-white/75">弹窗提示音效</span>
                <div className="flex items-center gap-1.5">
                  <button
                    onClick={() => playToastChime("completed")}
                    className="px-2 py-1 rounded-md text-[11px] bg-white/6 hover:bg-white/12 text-white/70 cursor-pointer"
                  >
                    试听
                  </button>
                  <button
                    onClick={() =>
                      void handleUpdateSettings({
                        soundEnabled: !snapshot.settings.soundEnabled,
                      })
                    }
                    className={`flex items-center gap-1 px-2.5 py-1 rounded-md text-[11px] cursor-pointer ${
                      snapshot.settings.soundEnabled
                        ? "bg-emerald-500/20 text-emerald-300 border border-emerald-500/30"
                        : "bg-white/6 text-white/50"
                    }`}
                  >
                    {snapshot.settings.soundEnabled ? (
                      <>
                        <Volume2 className="w-3.5 h-3.5" /> 开启
                      </>
                    ) : (
                      <>
                        <VolumeX className="w-3.5 h-3.5" /> 静音
                      </>
                    )}
                  </button>
                </div>
              </div>
            </div>
          </div>
        ) : (
          <>
            {/* Filter Bar */}
            <div className="flex items-center justify-between px-3.5 py-2 border-b border-white/6">
              <div className="flex items-center gap-1">
                {(
                  [
                    ["all", `全部 (${snapshot.sessions.length})`],
                    ["working", `运行中 (${snapshot.workingCount})`],
                    ["attention", `需关注 (${snapshot.attentionCount})`],
                    ["done", "已完成"],
                  ] as [FilterTab, string][]
                ).map(([key, label]) => (
                  <button
                    key={key}
                    onClick={() => setFilter(key)}
                    className={`px-2.5 py-1 rounded-lg text-[11px] font-medium transition-colors cursor-pointer ${
                      filter === key
                        ? "bg-white/14 text-white"
                        : "text-white/50 hover:text-white/80 hover:bg-white/5"
                    }`}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>

            {/* Session Cards */}
            <div className="flex-1 overflow-y-auto px-3 py-2.5 space-y-2">
              <AnimatePresence initial={false}>
                {filteredSessions.length === 0 ? (
                  <div className="h-48 flex flex-col items-center justify-center text-center px-6 gap-2 text-white/45">
                    <Sparkles className="w-6 h-6 text-white/25" />
                    <p className="text-[12.5px]">当前分类下暂无活跃 Agent 会话</p>
                    <p className="text-[11px] text-white/35">
                      启动 Codex、Claude Code 或 Antigravity 后将自动识别，也可点击右上角「演示」预览完整效果
                    </p>
                  </div>
                ) : (
                  filteredSessions.map((session) => (
                    <SessionCard
                      key={session.id}
                      session={session}
                      nowMs={nowMs}
                    />
                  ))
                )}
              </AnimatePresence>
            </div>
          </>
        )}
      </motion.div>
    </div>
  );
};

const SessionCard: React.FC<{ session: AgentSession; nowMs: number }> = ({
  session,
  nowMs,
}) => {
  const meta = PROVIDER_META[session.provider];
  const isWorking = session.phase === "working";
  const isWaiting = session.phase === "waiting_approval";
  const isFailed = session.phase === "failed";

  const liveElapsedMs =
    (isWorking || isWaiting) && session.turnStartedAtMs
      ? Math.max(0, nowMs - session.turnStartedAtMs)
      : session.lastDurationMs;

  const handleJump = () => {
    void invoke("jump_to_session", {
      provider: session.provider,
      hostPid: session.hostPid,
      hostApp: session.hostApp,
      cwd: session.cwd,
    });
  };

  const handleOpenFolder = () => {
    if (session.cwd) {
      void invoke("open_session_folder", { cwd: session.cwd });
    }
  };

  const handleDismiss = () => {
    void invoke("dismiss_session", { sessionId: session.id });
  };

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.96 }}
      className={`rounded-xl p-2.5 border transition-colors ${
        isWaiting
          ? "bg-amber-500/[0.08] border-amber-400/40"
          : isWorking
          ? "bg-emerald-500/[0.05] border-emerald-400/28"
          : isFailed
          ? "bg-rose-500/[0.07] border-rose-400/35"
          : "bg-white/[0.03] border-white/8 hover:border-white/16"
      }`}
    >
      {/* Top Row: Provider + Project + Branch + Elapsed/Time */}
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 min-w-0">
          <ProviderIcon provider={session.provider} size={14} />
          <span
            className={`text-[11px] font-semibold px-1.5 py-0.5 rounded ${meta.bgClass} ${meta.textClass}`}
          >
            {meta.short}
          </span>
          <span className="text-[12px] font-semibold text-white/92 truncate">
            {session.projectName}
          </span>
          {session.gitBranch && (
            <span className="inline-flex items-center gap-0.5 text-[10px] text-white/45 bg-white/6 px-1.5 py-0.5 rounded truncate max-w-[90px]">
              <GitBranch className="w-2.5 h-2.5 shrink-0" />
              {session.gitBranch}
            </span>
          )}
        </div>

        <div className="flex items-center gap-1.5 shrink-0">
          {isWorking || isWaiting ? (
            <span
              className={`text-[10.5px] font-mono px-1.5 py-0.5 rounded ${
                isWaiting
                  ? "bg-amber-400/20 text-amber-200"
                  : "bg-emerald-400/15 text-emerald-300"
              }`}
            >
              {formatDuration(liveElapsedMs)}
            </span>
          ) : (
            <span className="text-[10.5px] text-white/40">
              {formatRelativeAgo(session.updatedAtMs, nowMs)}
            </span>
          )}
          {!isWorking && !isWaiting && (
            <button
              onClick={handleDismiss}
              className="p-0.5 rounded text-white/30 hover:text-white/75 cursor-pointer"
              title="从列表移除"
            >
              <X className="w-3 h-3" />
            </button>
          )}
        </div>
      </div>

      {/* Middle Row: User task title */}
      <div className="mt-1.5 text-[12px] text-white/82 line-clamp-1">
        {session.title}
      </div>

      {/* Step / Status Pill */}
      <div className="mt-2 flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 min-w-0 flex-1">
          {isWaiting ? (
            <ShieldAlert className="w-3.5 h-3.5 text-amber-400 shrink-0 animate-pulse" />
          ) : isWorking ? (
            <span className="h-2 w-2 rounded-full bg-emerald-400 animate-pulse shrink-0" />
          ) : isFailed ? (
            <XCircle className="w-3.5 h-3.5 text-rose-400 shrink-0" />
          ) : (
            <CheckCircle2 className="w-3.5 h-3.5 text-emerald-400/80 shrink-0" />
          )}

          <span
            className={`text-[11px] truncate ${
              isWaiting
                ? "text-amber-200 font-medium"
                : isWorking
                ? "text-emerald-200/90 font-mono"
                : isFailed
                ? "text-rose-200"
                : "text-white/50"
            }`}
          >
            {session.currentAction ||
              session.lastSummary ||
              (isWorking ? "正在执行…" : "已完成")}
          </span>
        </div>

        {/* Action Buttons */}
        <div className="flex items-center gap-1 shrink-0">
          {session.stepCount > 0 && (
            <span className="text-[10px] text-white/35 mr-1">
              #{session.stepCount}
            </span>
          )}
          {session.cwd && (
            <button
              onClick={handleOpenFolder}
              className="p-1 rounded-md bg-white/5 hover:bg-white/12 text-white/55 hover:text-white transition-colors cursor-pointer"
              title={`打开目录: ${session.cwd}`}
            >
              <FolderOpen className="w-3 h-3" />
            </button>
          )}
          <button
            onClick={handleJump}
            className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-[10.5px] font-medium bg-white/8 hover:bg-white/16 text-white/85 transition-colors cursor-pointer"
            title={
              session.hostApp
                ? `跳转至 ${session.hostApp}`
                : "跳转至对应终端 / IDE"
            }
          >
            跳回
            <ExternalLink className="w-2.5 h-2.5" />
          </button>
        </div>
      </div>
    </motion.div>
  );
};
