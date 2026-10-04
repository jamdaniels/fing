import { listen } from "@tauri-apps/api/event";
import { BAND_MAP, DOT_REACH } from "./indicator/levels";
import { NoticeView } from "./indicator/notice";
import {
  deriveView,
  INITIAL_MODEL,
  type IndicatorAction,
  type IndicatorModel,
  type IndicatorView,
  reduceIndicator,
} from "./indicator/state";
import { DotVisualizer } from "./indicator/visualizer";
import { setUiLanguage, t } from "./lib/i18n";
import { getSettings } from "./lib/ipc";
import type {
  IndicatorLevelsPayload,
  IndicatorNoticePayload,
  IndicatorStatePayload,
  UiLanguage,
} from "./lib/types";

/** Dot diameter at rest; also the fixed dot width. */
const DOT_REST_PX = 4;
/** Longest dot: 30px pill - 2px border - 2 x 6px vertical padding. */
const DOT_MAX_PX = 16;

const pill = document.getElementById("indicator");
const dotsElement = document.getElementById("dots");
const noticeElement = document.getElementById("notice");
const noticeIcon = document.getElementById("notice-icon");
const noticeText = document.getElementById("notice-text");

const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

const visualizer = dotsElement
  ? new DotVisualizer({
      container: dotsElement,
      bandMap: BAND_MAP,
      dotReach: DOT_REACH,
      restSizePx: DOT_REST_PX,
      maxSizePx: DOT_MAX_PX,
      reducedMotion: () => reducedMotion.matches,
    })
  : null;

const noticeView =
  noticeElement && noticeIcon && noticeText
    ? new NoticeView({
        root: noticeElement,
        icon: noticeIcon,
        text: noticeText,
      })
    : null;

let model: IndicatorModel = INITIAL_MODEL;
let noticeSequence = 0;

function updateLabels(view: IndicatorView): void {
  dotsElement?.setAttribute(
    "aria-label",
    t(
      view.dotMode === "processing"
        ? "indicator.processing"
        : "indicator.recording"
    )
  );
}

// Once the shrink-out animation has finished, the dots stop animating entirely.
pill?.addEventListener("animationend", (event) => {
  if (event.target === pill && pill.classList.contains("is-out")) {
    visualizer?.stop();
    visualizer?.setMode("idle");
    visualizer?.reset();
  }
});

function render(previous: IndicatorModel, next: IndicatorModel): void {
  if (!(pill && visualizer && noticeView)) {
    return;
  }
  const was = deriveView(previous);
  const view = deriveView(next);
  if (!view.visible) {
    // Keep the last face (e.g. an expiring notice) while shrinking out.
    if (was.visible) {
      pill.classList.add("is-out");
    }
    return;
  }

  const appearing = !was.visible;
  if (appearing) {
    // Snap width/faces while still hidden so only the appear motion animates.
    pill.classList.add("is-instant");
    visualizer.reset();
  }

  pill.dataset.face = view.face;
  if (view.face === "notice" && next.notice) {
    noticeView.render(next.notice);
    pill.style.setProperty("--notice-width", `${noticeView.measureWidth()}px`);
  }

  visualizer.setMode(view.dotMode);
  visualizer.start();
  updateLabels(view);

  if (appearing) {
    // Flush the snapped styles once before transitions come back.
    pill.getBoundingClientRect();
    pill.classList.remove("is-instant", "is-out");
  }
}

function dispatch(action: IndicatorAction): void {
  const previous = model;
  model = reduceIndicator(model, action);
  if (model === previous) {
    return;
  }

  if (!model.notice) {
    noticeView?.cancelExpiry();
  } else if (model.notice !== previous.notice) {
    noticeView?.scheduleExpiry(model.notice, (id) => {
      dispatch({ type: "noticeExpired", id });
    });
  }

  render(previous, model);
}

function applyLanguage(language: UiLanguage): void {
  setUiLanguage(language);
  updateLabels(deriveView(model));
}

listen<IndicatorStatePayload>("indicator-state-changed", (event) => {
  dispatch({ type: "state", state: event.payload.state });
});

listen<IndicatorLevelsPayload>("indicator-levels", (event) => {
  visualizer?.setLevels(event.payload.levels);
});

listen<IndicatorNoticePayload>("indicator-notice", (event) => {
  noticeSequence += 1;
  dispatch({
    type: "notice",
    notice: { ...event.payload, id: noticeSequence },
  });
});

listen<{ language: UiLanguage }>("ui-language-changed", (event) => {
  applyLanguage(event.payload.language === "de" ? "de" : "en");
});

getSettings()
  .then((settings) => applyLanguage(settings.uiLanguage))
  .catch(() => applyLanguage("en"));
