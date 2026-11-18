import type { ToastKind } from "../types";

let sharedCtx: AudioContext | null = null;

function getAudioContext(): AudioContext | null {
  if (typeof window === "undefined") return null;
  const Ctx =
    window.AudioContext ||
    (window as unknown as { webkitAudioContext?: typeof AudioContext })
      .webkitAudioContext;
  if (!Ctx) return null;
  if (!sharedCtx) {
    sharedCtx = new Ctx();
  }
  if (sharedCtx.state === "suspended") {
    void sharedCtx.resume();
  }
  return sharedCtx;
}

function playTone(
  ctx: AudioContext,
  freq: number,
  startOffset: number,
  duration: number,
  type: OscillatorType = "sine",
  peakGain = 0.11
) {
  const now = ctx.currentTime + startOffset;
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();

  osc.type = type;
  osc.frequency.setValueAtTime(freq, now);

  gain.gain.setValueAtTime(0.0001, now);
  gain.gain.exponentialRampToValueAtTime(peakGain, now + 0.018);
  gain.gain.exponentialRampToValueAtTime(0.0001, now + duration);

  osc.connect(gain);
  gain.connect(ctx.destination);

  osc.start(now);
  osc.stop(now + duration + 0.02);
}

export function playToastChime(kind: ToastKind) {
  try {
    const ctx = getAudioContext();
    if (!ctx) return;

    if (kind === "completed") {
      // Crisp ascending major fifth glass chime (D5 -> A5)
      playTone(ctx, 587.33, 0, 0.24, "sine", 0.1);
      playTone(ctx, 880.0, 0.11, 0.36, "sine", 0.12);
    } else if (kind === "waiting_approval") {
      // Gentle double marimba tap for attention (C5 -> E5 -> G5)
      playTone(ctx, 523.25, 0, 0.16, "triangle", 0.12);
      playTone(ctx, 659.25, 0.12, 0.22, "triangle", 0.13);
    } else {
      // Soft descending alert (A4 -> F4)
      playTone(ctx, 440.0, 0, 0.2, "triangle", 0.11);
      playTone(ctx, 349.23, 0.14, 0.28, "triangle", 0.11);
    }
  } catch {
    // Ignore audio errors if blocked by OS autoplay policy
  }
}
