import {
  clampLevel,
  LEVELS_STALE_MS,
  levelToSize,
  livelyLevel,
  relativeLevel,
  SETTLED_EPSILON,
  smoothToward,
  trackPeak,
} from "./levels";
import type { DotMode } from "./state";

export interface DotVisualizerOptions {
  /** Band index for every dot; its length is the dot count. */
  bandMap: readonly number[];
  container: HTMLElement;
  /** Share of the full height each dot can reach (defaults to 1). */
  dotReach?: readonly number[];
  /** Longest dot length in px (pill height minus vertical padding). */
  maxSizePx: number;
  /** When true: no per-dot variation (levels still render). */
  reducedMotion: () => boolean;
  /** Dot diameter at rest, also the fixed dot width. */
  restSizePx: number;
}

const FALLBACK_FRAME_MS = 1000 / 60;
// Ignore long gaps (window hidden, throttled frames) instead of jumping.
const MAX_FRAME_GAP_MS = 100;
/** Remembered band peak before any levels arrive. */
const INITIAL_PEAK = 0;

/**
 * Renders N dots that grow vertically into rounded pills with their levels.
 * Each dot keeps a fixed width and a border-radius of half that width while its
 * height animates, so the round caps never distort. All DOM writes happen in a
 * single rAF callback and nothing is read back, so there is no layout thrash.
 */
export class DotVisualizer {
  private readonly bandMap: readonly number[];
  private readonly container: HTMLElement;
  private readonly current: number[];
  private readonly dotReach: readonly number[];
  private readonly dots: HTMLElement[];
  private frame: number | null = null;
  private lastFrameAt: number | null = null;
  private lastLevelsAt = Number.NEGATIVE_INFINITY;
  private readonly maxSizePx: number;
  private mode: DotMode = "idle";
  private readonly peaks: number[];
  private readonly reducedMotion: () => boolean;
  private readonly restSizePx: number;
  private readonly targets: number[];
  private readonly written: number[];

  constructor(options: DotVisualizerOptions) {
    this.bandMap = options.bandMap;
    this.container = options.container;
    this.dotReach = options.dotReach ?? [];
    this.maxSizePx = options.maxSizePx;
    this.restSizePx = options.restSizePx;
    this.reducedMotion = options.reducedMotion;

    const count = this.bandMap.length;
    this.current = new Array<number>(count).fill(0);
    this.targets = new Array<number>(count).fill(0);
    this.written = new Array<number>(count).fill(this.restSizePx);
    this.peaks = new Array<number>(Math.max(0, ...this.bandMap) + 1).fill(
      INITIAL_PEAK
    );

    this.container.style.setProperty("--dot-size", `${this.restSizePx}px`);
    this.container.style.setProperty("--dot-max", `${this.maxSizePx}px`);
    this.container.dataset.mode = this.mode;
    this.dots = Array.from({ length: count }, (_, index) => {
      const dot = document.createElement("span");
      dot.className = "dot";
      dot.style.setProperty("--dot-index", String(index));
      return dot;
    });
    this.container.replaceChildren(...this.dots);
  }

  /** Stores the latest band levels; they become dot targets in live mode. */
  setLevels(levels: readonly number[]): void {
    const now = performance.now();
    const dtMs = Math.min(MAX_FRAME_GAP_MS, now - this.lastLevelsAt);
    const relative = this.peaks.map((peak, band) => {
      const level = clampLevel(levels[band]);
      this.peaks[band] = trackPeak(peak, level, dtMs);
      return relativeLevel(level, this.peaks[band] ?? 0);
    });
    let audible = false;
    this.bandMap.forEach((band, index) => {
      const target = (relative[band] ?? 0) * (this.dotReach[index] ?? 1);
      this.targets[index] = target;
      audible ||= target > 0;
    });
    this.lastLevelsAt = now;
    // The frame loop sleeps through silence; wake it for audible levels.
    if (audible && this.mode === "live") {
      this.start();
    }
  }

  setMode(mode: DotMode): void {
    if (mode === this.mode) {
      return;
    }
    this.mode = mode;
    this.container.dataset.mode = mode;
    if (mode !== "live") {
      this.targets.fill(0);
    }
  }

  /** Starts the frame loop; it stops itself once every dot is at rest. */
  start(): void {
    if (this.frame !== null) {
      return;
    }
    this.lastFrameAt = null;
    this.frame = requestAnimationFrame(this.tick);
  }

  stop(): void {
    if (this.frame !== null) {
      cancelAnimationFrame(this.frame);
      this.frame = null;
    }
  }

  /** Snaps every dot back to rest and forgets old levels. */
  reset(): void {
    this.current.fill(0);
    this.targets.fill(0);
    this.peaks.fill(INITIAL_PEAK);
    this.lastLevelsAt = Number.NEGATIVE_INFINITY;
    this.dots.forEach((_, index) => {
      this.write(index, this.restSizePx);
    });
  }

  private readonly tick = (now: number): void => {
    const dtMs =
      this.lastFrameAt === null
        ? FALLBACK_FRAME_MS
        : Math.min(MAX_FRAME_GAP_MS, now - this.lastFrameAt);
    this.lastFrameAt = now;

    const live = this.mode === "live";
    const fresh = live && now - this.lastLevelsAt <= LEVELS_STALE_MS;
    const lively = live && !this.reducedMotion();
    let settled = true;

    for (let index = 0; index < this.dots.length; index += 1) {
      const target = fresh ? (this.targets[index] ?? 0) : 0;
      const level = smoothToward(this.current[index] ?? 0, target, dtMs);
      this.current[index] = level < SETTLED_EPSILON && target === 0 ? 0 : level;
      if (this.current[index] !== 0) {
        settled = false;
      }
      const display = lively ? livelyLevel(level, now, index) : level;
      this.write(index, levelToSize(display, this.restSizePx, this.maxSizePx));
    }

    this.frame = settled ? null : requestAnimationFrame(this.tick);
  };

  private write(index: number, sizePx: number): void {
    const dot = this.dots[index];
    if (!dot || this.written[index] === sizePx) {
      return;
    }
    this.written[index] = sizePx;
    dot.style.height = `${sizePx}px`;
  }
}
