import { listen } from "@tauri-apps/api/event";
import { setUiLanguage, t } from "./lib/i18n";
import { getSettings } from "./lib/ipc";
import type { UiLanguage } from "./lib/types";

type IndicatorState = "recording" | "processing" | "hidden";

const indicator = document.getElementById("indicator");
const recording = document.getElementById("recording");
const processing = document.getElementById("processing");
const processingTitle = document.querySelector(".processing title");

function applyLanguage(language: UiLanguage): void {
  setUiLanguage(language);
  if (processingTitle) {
    processingTitle.textContent = t("indicator.processing");
  }
}

function setState(state: IndicatorState): void {
  if (!(indicator && recording && processing)) {
    return;
  }

  switch (state) {
    case "recording":
      indicator.classList.remove("shrinking");
      recording.classList.remove("hidden");
      processing.classList.add("hidden");
      break;
    case "processing":
      indicator.classList.remove("shrinking");
      recording.classList.add("hidden");
      processing.classList.remove("hidden");
      break;
    case "hidden":
      indicator.classList.add("shrinking");
      break;
    default:
      break;
  }
}

listen<{ state: IndicatorState }>("indicator-state-changed", (event) => {
  setState(event.payload.state);
});

listen<{ language: UiLanguage }>("ui-language-changed", (event) => {
  applyLanguage(event.payload.language === "de" ? "de" : "en");
});

getSettings()
  .then((settings) => applyLanguage(settings.uiLanguage))
  .catch(() => applyLanguage("en"));
