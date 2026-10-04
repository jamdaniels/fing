// Pure indicator state machine: a base state from Rust plus an optional notice.

import type { IndicatorNoticePayload, IndicatorState } from "../lib/types";

export interface ActiveNotice extends IndicatorNoticePayload {
  id: number;
}

export interface IndicatorModel {
  base: IndicatorState;
  notice: ActiveNotice | null;
}

export type IndicatorAction =
  | { type: "state"; state: IndicatorState }
  | { type: "notice"; notice: ActiveNotice }
  | { type: "noticeExpired"; id: number };

export type DotMode = "live" | "processing" | "idle";
export type IndicatorFace = "dots" | "notice";

export interface IndicatorView {
  dotMode: DotMode;
  face: IndicatorFace;
  visible: boolean;
}

export const INITIAL_MODEL: IndicatorModel = { base: "hidden", notice: null };

export function reduceIndicator(
  model: IndicatorModel,
  action: IndicatorAction
): IndicatorModel {
  switch (action.type) {
    case "state":
      // A new recording always wins over a lingering notice; processing and
      // hidden keep the notice until it expires.
      return {
        base: action.state,
        notice: action.state === "recording" ? null : model.notice,
      };
    case "notice":
      return { ...model, notice: action.notice };
    case "noticeExpired":
      return model.notice?.id === action.id
        ? { ...model, notice: null }
        : model;
    default:
      return model;
  }
}

export function deriveView(model: IndicatorModel): IndicatorView {
  if (model.notice) {
    return { visible: true, face: "notice", dotMode: "idle" };
  }
  switch (model.base) {
    case "recording":
      return { visible: true, face: "dots", dotMode: "live" };
    case "processing":
      return { visible: true, face: "dots", dotMode: "processing" };
    default:
      return { visible: false, face: "dots", dotMode: "idle" };
  }
}
