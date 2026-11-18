import React, { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AnimatePresence, motion } from "framer-motion";
import {
  CheckCircle2,
  ExternalLink,
  ShieldAlert,
  X,
  XCircle,
} from "lucide-react";
import type { DashboardSnapshot, ToastPayload } from "../types";
import {
  formatDuration,
  PROVIDER_META,
  ProviderIcon,
} from "./ProviderBadge";
import { playToastChime } from "../utils/sound";

interface ActiveToast extends ToastPayload {
  expiresAt: number;
  totalMs: number;
}

export const ToastWindow: React.FC = () => {
  const [toasts, setToasts] = useState<ActiveToast[]>([]);
  const [hovered, setHovered] = useState(false);
  const soundEnabledRef = useRef(true);

  useEffect(() => {
    invoke<DashboardSnapshot>("get_snapshot")
      .then((snap) => {
        soundEnabledRef.current = snap.settings.soundEnabled;
      })
      .catch(() => {});

    const unlistenSnap = listen<DashboardSnapshot>("sessions-updated", (ev) => {
      soundEnabledRef.current = ev.payload.settings.soundEnabled;
    });

    const unlistenToast = listen<ToastPayload>("toast-push", (ev) => {
      const incoming = ev.payload;
      if (soundEnabledRef.current) {
        playToastChime(incoming.kind);
      }
      const ttl = incoming.kind === "completed" ? 8000 : 12000;
      const now = Date.now();
      setToasts((prev) => {
        const next: ActiveToast[] = [
          { ...incoming, expiresAt: now + ttl, totalMs: ttl },
          ...prev.filter((t) => t.sessionId !== incoming.sessionId),
        ].slice(0, 2);
        return next;
      });
    });

    return () => {
      void unlistenSnap.then((fn) => fn());
      void unlistenToast.then((fn) => fn());
    };
  }, []);

  // Toggle window click-through automatically when queue empties or fills
  useEffect(() => {
    void invoke("set_toast_interactive", { interactive: toasts.length > 0 });
  }, [toasts.length]);

  // Auto-dismiss timer (pauses countdown when user hovers card)
  useEffect(() => {
    if (toasts.length === 0) return;
    const id = window.setInterval(() => {
      if (hovered) {
        setToasts((prev) =>
          prev.map((t) => ({ ...t, expiresAt: t.expiresAt + 200 }))
        );
        return;
      }
      const now = Date.now();
      setToasts((prev) => prev.filter((t) => t.expiresAt > now));
    }, 200);
    return () => window.clearInterval(id);
  }, [toasts.length, hovered]);

  const dismissOne = useCallback((id: string) => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const jumpAndDismiss = useCallback((t: ActiveToast) => {
    void invoke("jump_to_session", {
      provider: t.provider,
      hostPid: t.hostPid,
      hostApp: t.hostApp,
      cwd: t.cwd,
    });
    setToasts((prev) => prev.filter((item) => item.id !== t.id));
  }, []);

  return (
    <div
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      className="w-full h-full p-2.5 flex flex-col gap-2 justify-start pointer-events-auto"
    >
      <AnimatePresence>
        {toasts.map((t) => {
          const meta = PROVIDER_META[t.provider];
          const isApproval = t.kind === "waiting_approval";
          const isFailed = t.kind === "failed";

          return (
            <motion.div
              key={t.id}
              initial={{ opacity: 0, x: 36, scale: 0.95 }}
              animate={{ opacity: 1, x: 0, scale: 1 }}
              exit={{ opacity: 0, x: 28, scale: 0.95 }}
              transition={{ type: "spring", stiffness: 430, damping: 29 }}
              className={`relative shrink-0 rounded-2xl p-3 backdrop-blur-2xl border shadow-[0_14px_34px_rgba(0,0,0,0.58)] overflow-hidden ${
                isApproval
                  ? "bg-[#19140C]/96 border-amber-400/50"
                  : isFailed
                  ? "bg-[#1B1014]/96 border-rose-400/45"
                  : "bg-[#0F1518]/96 border-emerald-400/40"
              }`}
            >
              <div className="flex items-start justify-between gap-2">
                <div className="flex items-center gap-1.5 min-w-0">
                  {isApproval ? (
                    <ShieldAlert className="w-4 h-4 text-amber-400 shrink-0" />
                  ) : isFailed ? (
                    <XCircle className="w-4 h-4 text-rose-400 shrink-0" />
                  ) : (
                    <CheckCircle2 className="w-4 h-4 text-emerald-400 shrink-0" />
                  )}
                  <ProviderIcon provider={t.provider} size={13} />
                  <span className={`text-[11.5px] font-semibold ${meta.textClass}`}>
                    {meta.label}
                  </span>
                  <span className="text-[11px] text-white/45 truncate">
                    · {t.projectName}
                  </span>
                </div>

                <div className="flex items-center gap-1.5 shrink-0">
                  {t.durationMs ? (
                    <span className="text-[10.5px] font-mono px-1.5 py-0.5 rounded bg-white/8 text-white/70">
                      {formatDuration(t.durationMs)}
                    </span>
                  ) : null}
                  <button
                    onClick={() => dismissOne(t.id)}
                    className="p-0.5 rounded-md text-white/45 hover:text-white hover:bg-white/10 cursor-pointer"
                  >
                    <X className="w-3.5 h-3.5" />
                  </button>
                </div>
              </div>

              <div className="mt-1.5 text-[12px] font-medium text-white/92 line-clamp-1">
                {isApproval
                  ? "等待你确认操作权限"
                  : isFailed
                  ? "任务执行报错中断"
                  : t.title}
              </div>

              <div className="mt-0.5 text-[11px] text-white/62 line-clamp-2 leading-snug">
                {t.message}
              </div>

              <div className="mt-2.5 flex items-center justify-end gap-2">
                <button
                  onClick={() => dismissOne(t.id)}
                  className="px-2.5 py-1 rounded-lg text-[11px] text-white/55 hover:text-white hover:bg-white/8 transition-colors cursor-pointer"
                >
                  知道了
                </button>
                <button
                  onClick={() => jumpAndDismiss(t)}
                  className={`inline-flex items-center gap-1 px-2.5 py-1 rounded-lg text-[11px] font-medium transition-colors cursor-pointer ${
                    isApproval
                      ? "bg-amber-400/20 hover:bg-amber-400/30 text-amber-200 border border-amber-400/35"
                      : "bg-emerald-500/20 hover:bg-emerald-500/30 text-emerald-200 border border-emerald-400/35"
                  }`}
                >
                  跳回窗口处理
                  <ExternalLink className="w-3 h-3" />
                </button>
              </div>
              <div
                aria-hidden="true"
                className={`toast-countdown absolute bottom-0 inset-x-0 h-0.5 ${
                  isApproval ? "bg-amber-400/70" : isFailed ? "bg-rose-400/70" : "bg-emerald-400/70"
                }`}
                style={{
                  animationDuration: `${t.totalMs}ms`,
                  animationPlayState: hovered ? "paused" : "running",
                }}
              />
            </motion.div>
          );
        })}
      </AnimatePresence>
    </div>
  );
};
