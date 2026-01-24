import { listen } from "@tauri-apps/api/event";

type IndicatorState = "recording" | "processing" | "hidden";

const indicator = document.getElementById("indicator");
const recording = document.getElementById("recording");
const processing = document.getElementById("processing");

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
