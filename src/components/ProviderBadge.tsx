import React from "react";
import type { AgentProvider } from "../types";

export const PROVIDER_META: Record<
  AgentProvider,
  {
    label: string;
    short: string;
    accent: string;
    bgClass: string;
    textClass: string;
    borderClass: string;
  }
> = {
  claude_code: {
    label: "Claude Code",
    short: "Claude",
    accent: "#E07A5F",
    bgClass: "bg-[#E07A5F]/15",
    textClass: "text-[#F29E86]",
    borderClass: "border-[#E07A5F]/30",
  },
  codex: {
    label: "Codex",
    short: "Codex",
    accent: "#10B981",
    bgClass: "bg-emerald-500/15",
    textClass: "text-emerald-300",
    borderClass: "border-emerald-500/30",
  },
  antigravity: {
    label: "Antigravity",
    short: "AGY",
    accent: "#8B5CF6",
    bgClass: "bg-violet-500/15",
    textClass: "text-violet-300",
    borderClass: "border-violet-500/30",
  },
};

export const ProviderIcon: React.FC<{
  provider: AgentProvider;
  size?: number;
}> = ({ provider, size = 14 }) => {
  if (provider === "claude_code") {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        fill="none"
        className="shrink-0"
      >
        <circle cx="12" cy="12" r="9.5" stroke="#E07A5F" strokeWidth="2" />
        <path
          d="M12 6.5V17.5M6.5 12H17.5M8.2 8.2L15.8 15.8M15.8 8.2L8.2 15.8"
          stroke="#F29E86"
          strokeWidth="1.8"
          strokeLinecap="round"
        />
      </svg>
    );
  }
  if (provider === "codex") {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        fill="none"
        className="shrink-0"
      >
        <rect
          x="3"
          y="4"
          width="18"
          height="16"
          rx="4.5"
          stroke="#10B981"
          strokeWidth="2"
        />
        <path
          d="M8 9.5L11 12L8 14.5M12.5 14.5H16"
          stroke="#6EE7B7"
          strokeWidth="1.9"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      className="shrink-0"
    >
      <path
        d="M12 3L19.5 18.5H4.5L12 3Z"
        stroke="#8B5CF6"
        strokeWidth="2"
        strokeLinejoin="round"
      />
      <circle cx="12" cy="13.5" r="2.3" fill="#C4B5FD" />
    </svg>
  );
};

export function formatDuration(ms: number | null | undefined): string {
  if (!ms || ms < 0) return "0s";
  const totalSec = Math.max(1, Math.floor(ms / 1000));
  if (totalSec < 60) return `${totalSec}s`;
  const mins = Math.floor(totalSec / 60);
  const secs = totalSec % 60;
  if (mins < 60) return secs > 0 ? `${mins}m ${secs}s` : `${mins}m`;
  const hrs = Math.floor(mins / 60);
  const remMins = mins % 60;
  return `${hrs}h ${remMins}m`;
}

export function formatRelativeAgo(updatedAtMs: number, nowMs: number): string {
  const diffSec = Math.max(0, Math.floor((nowMs - updatedAtMs) / 1000));
  if (diffSec < 10) return "刚刚";
  if (diffSec < 60) return `${diffSec}秒前`;
  const mins = Math.floor(diffSec / 60);
  if (mins < 60) return `${mins}分钟前`;
  const hrs = Math.floor(mins / 60);
  return `${hrs}小时前`;
}
