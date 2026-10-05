import { describe, expect, it } from "bun:test";
import {
  type ActiveNotice,
  deriveView,
  INITIAL_MODEL,
  type IndicatorModel,
  reduceIndicator,
} from "./state";

function notice(id: number, kind: ActiveNotice["kind"] = "info"): ActiveNotice {
  return { id, kind, message: `notice ${id}`, durationMs: 3000 };
}

describe("reduceIndicator", () => {
  it("recording clears an active notice", () => {
    const model: IndicatorModel = { base: "hidden", notice: notice(1) };
    expect(
      reduceIndicator(model, { type: "state", state: "recording" })
    ).toEqual({ base: "recording", notice: null });
  });

  it("processing and hidden keep the notice until it expires", () => {
    const active = notice(1, "error");
    const recording: IndicatorModel = { base: "recording", notice: active };
    expect(
      reduceIndicator(recording, { type: "state", state: "processing" })
    ).toEqual({ base: "processing", notice: active });
    expect(
      reduceIndicator(recording, { type: "state", state: "hidden" })
    ).toEqual({ base: "hidden", notice: active });
  });

  it("only the current notice's expiry clears it", () => {
    const model: IndicatorModel = { base: "processing", notice: notice(2) };
    expect(reduceIndicator(model, { type: "noticeExpired", id: 1 })).toBe(
      model
    );
    expect(reduceIndicator(model, { type: "noticeExpired", id: 2 })).toEqual({
      base: "processing",
      notice: null,
    });
  });
});

describe("deriveView", () => {
  it("shows error notices over any base state", () => {
    for (const base of ["recording", "processing", "hidden"] as const) {
      expect(deriveView({ base, notice: notice(1, "error") })).toEqual({
        visible: true,
        face: "notice",
        dotMode: "idle",
      });
    }
  });

  it("keeps the dots running beside info notices", () => {
    expect(deriveView({ base: "recording", notice: notice(1) })).toEqual({
      visible: true,
      face: "dots-notice",
      dotMode: "live",
    });
    expect(deriveView({ base: "processing", notice: notice(1) })).toEqual({
      visible: true,
      face: "dots-notice",
      dotMode: "processing",
    });
    expect(deriveView({ base: "hidden", notice: notice(1) }).face).toBe(
      "notice"
    );
  });

  it("hides after a notice expires on a hidden base", () => {
    const shown = reduceIndicator(INITIAL_MODEL, {
      type: "notice",
      notice: notice(7),
    });
    expect(deriveView(shown).visible).toBe(true);
    const expired = reduceIndicator(shown, { type: "noticeExpired", id: 7 });
    expect(deriveView(expired).visible).toBe(false);
  });
});
