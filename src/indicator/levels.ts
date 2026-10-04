// Pure level-processing helpers for the indicator dot visualizer (no DOM).

/**
 * Band index for every dot, mirrored around the center with the lowest band
 * in the middle: high, mid, low, low, mid, high. Rust sends 3 bands.
 */
export const BAND_MAP: readonly number[] = [2, 1, 0, 0, 1, 2];

/** Share of the full height each dot can reach: center dots swing the most. */
export const DOT_REACH: readonly number[] = [0.7, 0.88, 1, 1, 0.88, 0.7];

// Adaptive gain: each band is shown relative to its own recent peak, so loud
// speech keeps moving instead of pinning every dot at full height.
/** How fast a band's remembered peak sinks, in level units per second. */
const PEAK_DECAY_PER_SECOND = 0.25;
/** Lowest remembered peak, so quiet rooms are never boosted into motion. */
const PEAK_FLOOR = 0.4;
/** Level span below the peak that maps onto 0..1 (~14 dB with Rust's 48 dB range). */
const DYNAMIC_SPAN = 0.3;

/** Fraction of the remaining distance covered per 60 Hz frame when rising. */
const ATTACK_PER_FRAME = 0.7;
/** Fraction of the remaining distance covered per 60 Hz frame when falling. */
const RELEASE_PER_FRAME = 0.2;
/** Without fresh levels for this long, the dots decay back to rest. */
export const LEVELS_STALE_MS = 150;
/** Below this every dot counts as resting, so an idle loop may stop. */
export const SETTLED_EPSILON = 0.002;

const REFERENCE_FRAME_MS = 1000 / 60;
const MAX_CATCH_UP_FRAMES = 6;
const TAU = Math.PI * 2;

// Per-dot variation: slow +-8% gain drift so mirrored pairs are not identical.
const VARIATION_AMOUNT = 0.08;
const VARIATION_PERIOD_MS = 900;
const DOT_PHASE_STEP = 1.3;

export function clampLevel(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return 0;
  }
  return Math.min(1, Math.max(0, value));
}

/** Converts a per-60Hz-frame rate into the equivalent rate for `dtMs`. */
export function frameRate(perFrame: number, dtMs: number): number {
  const frames = Math.min(
    MAX_CATCH_UP_FRAMES,
    Math.max(0, dtMs / REFERENCE_FRAME_MS)
  );
  return 1 - (1 - perFrame) ** frames;
}

/** One smoothing step: fast attack toward louder targets, slower release. */
export function smoothToward(
  current: number,
  target: number,
  dtMs: number
): number {
  const rate = target > current ? ATTACK_PER_FRAME : RELEASE_PER_FRAME;
  return current + (target - current) * frameRate(rate, dtMs);
}

/** Next remembered peak: jumps up instantly, sinks slowly, never below the floor. */
export function trackPeak(peak: number, level: number, dtMs: number): number {
  const decayed = peak - PEAK_DECAY_PER_SECOND * (Math.max(0, dtMs) / 1000);
  return Math.max(PEAK_FLOOR, clampLevel(level), decayed);
}

/** A level relative to its band's recent peak (peak -> 1, span below -> 0). */
export function relativeLevel(level: number, peak: number): number {
  return clampLevel((level - (peak - DYNAMIC_SPAN)) / DYNAMIC_SPAN);
}

/** Adds slow per-dot variation to a smoothed level (silence stays at 0). */
export function livelyLevel(
  level: number,
  timeMs: number,
  dotIndex: number
): number {
  const phase = dotIndex * DOT_PHASE_STEP * 1.7;
  const variation =
    1 +
    VARIATION_AMOUNT * Math.sin((timeMs / VARIATION_PERIOD_MS) * TAU + phase);
  return clampLevel(level * variation);
}

/** Maps a 0..1 level onto a dot length in px, quantized to quarter pixels. */
export function levelToSize(
  level: number,
  restPx: number,
  maxPx: number
): number {
  const size = restPx + (maxPx - restPx) * clampLevel(level);
  return Math.round(size * 4) / 4;
}
