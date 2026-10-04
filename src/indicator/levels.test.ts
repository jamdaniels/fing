import { describe, expect, it } from "bun:test";
import { frameRate, smoothToward } from "./levels";

const FRAME_MS = 1000 / 60;

function near(actual: number, expected: number, epsilon = 1e-9): boolean {
  return Math.abs(actual - expected) < epsilon;
}

describe("smoothToward", () => {
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
