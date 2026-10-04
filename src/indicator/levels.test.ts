import { describe, expect, it } from "bun:test";
import {
  ATTACK_PER_FRAME,
  clampLevel,
  frameRate,
  levelToSize,
  livelyLevel,
  RELEASE_PER_FRAME,
  smoothToward,
} from "./levels";

const FRAME_MS = 1000 / 60;

function near(actual: number, expected: number, epsilon = 1e-9): boolean {
  return Math.abs(actual - expected) < epsilon;
}

describe("clampLevel", () => {
  it("keeps levels within 0..1", () => {
    expect(clampLevel(0.4)).toBe(0.4);
    expect(clampLevel(5)).toBe(1);
    expect(clampLevel(-0.2)).toBe(0);
    expect(clampLevel(Number.POSITIVE_INFINITY)).toBe(0);
    expect(clampLevel(undefined)).toBe(0);
  });
});

describe("smoothToward", () => {
  it("attacks fast and releases slowly per 60 Hz frame", () => {
    expect(near(smoothToward(0, 1, FRAME_MS), ATTACK_PER_FRAME)).toBe(true);
    expect(near(smoothToward(1, 0, FRAME_MS), 1 - RELEASE_PER_FRAME)).toBe(
      true
    );
  });

  it("is frame-rate independent", () => {
    const twoFrames = smoothToward(smoothToward(0, 1, FRAME_MS), 1, FRAME_MS);
    expect(near(smoothToward(0, 1, FRAME_MS * 2), twoFrames)).toBe(true);
  });

  it("does not move without elapsed time and never overshoots", () => {
    expect(smoothToward(0.3, 1, 0)).toBe(0.3);
    expect(smoothToward(0.3, 1, 10_000) <= 1).toBe(true);
    expect(frameRate(0.5, -5)).toBe(0);
  });

  it("caps catch-up after long frame gaps", () => {
    expect(near(frameRate(0.2, 10_000), frameRate(0.2, FRAME_MS * 6))).toBe(
      true
    );
  });
});

describe("livelyLevel", () => {
  it("stays within 0..1", () => {
    for (let time = 0; time < 5000; time += 37) {
      for (let dot = 0; dot < 6; dot += 1) {
        const silent = livelyLevel(0, time, dot);
        const loud = livelyLevel(1, time, dot);
        expect(silent).toBe(0);
        expect(loud >= 0.9 && loud <= 1).toBe(true);
      }
    }
  });
});

describe("levelToSize", () => {
  it("maps levels onto rest..max px in quarter pixels", () => {
    expect(levelToSize(0, 4, 16)).toBe(4);
    expect(levelToSize(1, 4, 16)).toBe(16);
    expect(levelToSize(0.5, 4, 16)).toBe(10);
    expect(levelToSize(0.52, 4, 16)).toBe(10.25);
    expect(levelToSize(3, 4, 16)).toBe(16);
  });
});
