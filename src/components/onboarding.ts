import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  Check,
  CheckCircle,
  Download,
  Globe,
  Keyboard,
  Mic,
  PersonStanding,
  RefreshCw,
  Upload,
} from "lucide";
import { createIcon, escapeHtml } from "../lib/icons";
import {
  checkModelExists,
  completeSetup,
  disableOnboardingTestMode,
  enableOnboardingTestMode,
  getAudioDevices,
  getDownloadProgress,
  getMicTestLevel,
  getSettings,
  relaunchApp,
  requestAccessibilityPermission,
  requestMicrophonePermission,
  requestPermissions,
  selectModelFile,
  startMicTest,
  startModelDownload,
  stopMicTest,
  updateSettings,
} from "../lib/ipc";
import type {
  AudioDevice,
  DownloadProgress,
  MicrophoneTest,
  PermissionStatus,
  Settings,
} from "../lib/types";

type OnboardingStep = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8;

interface OnboardingState {
  step: OnboardingStep;
  downloadProgress: DownloadProgress | null;
  downloadError: string | null;
  permissions: PermissionStatus | null;
  audioDevices: AudioDevice[];
  selectedDeviceId: string | null;
  micTest: MicrophoneTest | null;
  audioDetected: boolean;
  selectedLanguages: string[];
  selectedHotkey: string;
  capturedHotkey: string | null;
  testText: string;
}

const SUPPORTED_LANGUAGES = [
  { code: "en", name: "English" },
  { code: "de", name: "German" },
  { code: "es", name: "Spanish" },
  { code: "fr", name: "French" },
];

let state: OnboardingState = {
  step: 1,
  downloadProgress: null,
  downloadError: null,
  permissions: null,
  audioDevices: [],
  selectedDeviceId: null,
  micTest: null,
  audioDetected: false,
  selectedLanguages: ["en"],
  selectedHotkey: "F8",
  capturedHotkey: null,
  testText: "",
};

let container: HTMLElement | null = null;
let downloadPollInterval: number | null = null;
let micTestInterval: number | null = null;
let hotkeyKeyHandler: ((e: KeyboardEvent) => void) | null = null;
let testResultUnlisten: UnlistenFn | null = null;

const TOTAL_STEPS = 8;

function renderStepIndicator(currentStep: OnboardingStep): string {
  if (currentStep === 8) {
    return ""; // Don't show on completion
  }

  const dots: string[] = [];
  for (let i = 1; i <= TOTAL_STEPS - 1; i++) {
    // 7 dots (exclude completion)
    const isActive = i === currentStep;
    const isPast = i < currentStep;
    const clickable = isPast;
    dots.push(`
      <button
        class="step-dot ${isActive ? "active" : ""} ${isPast ? "completed" : ""}"
        data-step="${i}"
        ${clickable ? "" : "disabled"}
        aria-label="Step ${i}"
      ></button>
    `);
  }
  return `<div class="step-indicator">${dots.join("")}</div>`;
}

function attachStepIndicatorListeners(): void {
  for (const dot of document.querySelectorAll(".step-dot[data-step]")) {
    dot.addEventListener("click", (e) => {
      const step = Number(
        (e.currentTarget as HTMLElement).dataset.step
      ) as OnboardingStep;
      if (step < state.step) {
        goToStep(step);
      }
    });
  }
}

function formatBytes(bytes: number): string {
  if (bytes === 0) {
    return "0 B";
  }
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / k ** i).toFixed(1)} ${sizes[i]}`;
}

async function persistSelectedDevice(deviceId: string | null): Promise<void> {
  try {
    const currentSettings = await getSettings();
    if (currentSettings.selectedMicrophoneId === deviceId) {
      return;
    }
    await updateSettings({
      ...currentSettings,
      selectedMicrophoneId: deviceId,
    });
  } catch (err) {
    console.error("Failed to save microphone selection:", err);
  }
}

function formatKeyForDisplay(hotkey: string): string {
  const parts = hotkey.split("+");
  const formatted = parts.map((part) => {
    const keyMap: Record<string, string> = {
      Ctrl: "Ctrl",
      Option: "Opt",
      Shift: "Shift",
      Cmd: "Cmd",
      " ": "Space",
    };
    return keyMap[part] || part;
  });
  return formatted.join(" + ");
}

const FUNCTION_KEY_REGEX = /^F\d+$/;

function keyEventToHotkey(e: KeyboardEvent): string | null {
  let key = e.key;

  if (["Control", "Alt", "Shift", "Meta"].includes(key)) {
    return null;
  }

  if (key === "Escape") {
    return null;
  }

  if (FUNCTION_KEY_REGEX.test(key)) {
    // Keep as-is
  } else if (key === " ") {
    key = "Space";
  } else if (key.length === 1) {
    key = key.toUpperCase();
  }

  const modifiers: string[] = [];
  if (e.ctrlKey) {
    modifiers.push("Ctrl");
  }
  if (e.altKey) {
    modifiers.push("Option");
  }
  if (e.shiftKey) {
    modifiers.push("Shift");
  }
  if (e.metaKey) {
    modifiers.push("Cmd");
  }

  if (modifiers.length === 0) {
    return key;
  }

  return [...modifiers, key].join("+");
}

function renderDownloadButton(
  isDownloading: boolean,
  isComplete: boolean,
  isFailed: boolean,
  statusText: string,
  progress: DownloadProgress | null
): string {
  if (isDownloading || isComplete) {
    return `
      <div class="download-progress-container">
        <div class="progress-bar">
          <div class="progress-bar-fill" style="width: ${progress?.percentage ?? 0}%"></div>
        </div>
        <div class="download-status ${isFailed ? "error" : ""}">${statusText}</div>
        ${isDownloading ? `<button class="btn btn-secondary" id="cancel-download-btn">Cancel</button>` : ""}
      </div>
    `;
  }
  if (isFailed) {
    return `
      <div class="download-progress-container">
        <div class="download-status error">${statusText}</div>
        <button class="btn btn-primary" id="retry-download-btn">Retry Download</button>
      </div>
    `;
  }
  return `
    <button class="btn btn-primary btn-lg" id="start-download-btn">
      Download Model
    </button>
  `;
}

function renderMicPermissionStatus(status: string | undefined): string {
  if (status === "granted") {
    return `<div class="permission-status granted">Granted</div>`;
  }
  if (status === "prompt") {
    return `<button class="btn btn-outline btn-sm" id="grant-microphone-btn">Allow</button>`;
  }
  return `<button class="btn btn-outline btn-sm" id="grant-microphone-btn">Grant</button>`;
}

function renderAccessibilityPermissionStatus(
  status: string | undefined
): string {
  if (status === "granted") {
    return `<div class="permission-status granted">Granted</div>`;
  }
  if (status === "not-applicable") {
    return `<div class="permission-status">N/A</div>`;
  }
  return `<button class="btn btn-outline btn-sm" id="grant-accessibility-btn">Grant</button>`;
}

function render(): void {
  if (!container) {
    return;
  }

  // Remove hotkey listener when leaving step 5
  if (state.step !== 5 && hotkeyKeyHandler) {
    document.removeEventListener("keydown", hotkeyKeyHandler);
    hotkeyKeyHandler = null;
  }

  switch (state.step) {
    case 1:
      renderWelcome();
      break;
    case 2:
      renderDownloadModel();
      break;
    case 3:
      renderLanguageSelection();
      break;
    case 4:
      renderPermissions();
      break;
    case 5:
      renderHotkeyStep();
      break;
    case 6:
      renderMicSelection();
      break;
    case 7:
      renderTestStep();
      break;
    case 8:
      renderCompletion();
      break;
    default:
      break;
  }
}

function renderWelcome(): void {
  if (!container) {
    return;
  }

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <img class="onboarding-logo" src="/icon.png" alt="Fing" />
        <h1 class="onboarding-title">Welcome to Fing</h1>
        <p class="onboarding-desc">Fast, private, local speech-to-text</p>
      </div>
      <div class="onboarding-body">
        <ul class="onboarding-features">
          <li>${createIcon(PersonStanding)} All processing happens locally on your device</li>
          <li>${createIcon(Mic)} Microphone is only active while you hold the hotkey</li>
          <li>${createIcon(Check)} Your audio never leaves your computer</li>
        </ul>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-primary btn-lg" id="get-started-btn">
          Get Started
        </button>
        ${renderStepIndicator(1)}
      </div>
    </div>
  `;

  document
    .getElementById("get-started-btn")
    ?.addEventListener("click", () => goToStep(2));
  attachStepIndicatorListeners();
}

function renderDownloadModel(): void {
  if (!container) {
    return;
  }

  const progress = state.downloadProgress;
  const isDownloading =
    progress?.status === "downloading" || progress?.status === "verifying";
  const isComplete = progress?.status === "complete";
  const isFailed = progress?.status === "failed";

  let statusText = "";
  if (progress) {
    switch (progress.status) {
      case "downloading":
        statusText = `Downloading... ${formatBytes(progress.bytesDownloaded)} / ${formatBytes(progress.totalBytes)}`;
        break;
      case "verifying":
        statusText = "Verifying download...";
        break;
      case "complete":
        statusText = "Download complete!";
        break;
      case "failed":
        statusText = progress.errorMessage || "Download failed";
        break;
      default:
        statusText = "";
    }
  }

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(Download)}
        </div>
        <h1 class="onboarding-title">Download Speech Model</h1>
        <p class="onboarding-desc">Fing needs a speech recognition model (~142 MB, one-time download)</p>
      </div>
      <div class="onboarding-body">
        ${state.downloadError ? `<div class="download-status error" style="margin-bottom: 16px;">${state.downloadError}</div>` : ""}
        ${renderDownloadButton(isDownloading, isComplete, isFailed, statusText, progress)}
        ${
          isDownloading || isComplete
            ? ""
            : `
          <div class="onboarding-divider">
            <span>OR</span>
          </div>
          <button class="btn btn-outline" id="select-file-btn">
            ${createIcon(Upload)}
            Already have the model file? Choose File...
          </button>
        `
        }
      </div>
      <div class="onboarding-footer">
        ${
          isComplete
            ? `<button class="btn btn-primary btn-lg" id="continue-btn">Continue</button>`
            : ""
        }
        ${renderStepIndicator(2)}
      </div>
    </div>
  `;

  document
    .getElementById("start-download-btn")
    ?.addEventListener("click", handleStartDownload);
  document
    .getElementById("retry-download-btn")
    ?.addEventListener("click", handleStartDownload);
  document
    .getElementById("cancel-download-btn")
    ?.addEventListener("click", handleCancelDownload);
  document
    .getElementById("select-file-btn")
    ?.addEventListener("click", handleSelectFile);
  document
    .getElementById("continue-btn")
    ?.addEventListener("click", () => goToStep(3));
  attachStepIndicatorListeners();
}

function renderLanguageSelection(): void {
  if (!container) {
    return;
  }

  const langCheckboxes = SUPPORTED_LANGUAGES.map(
    (lang) => `
      <label class="lang-checkbox onboarding-lang-checkbox">
        <input type="checkbox" class="lang-check" data-lang="${lang.code}" ${state.selectedLanguages.includes(lang.code) ? "checked" : ""}>
        <span>${lang.name}</span>
      </label>
    `
  ).join("");

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(Globe)}
        </div>
        <h1 class="onboarding-title">Select Languages</h1>
        <p class="onboarding-desc">Which languages do you speak?</p>
      </div>
      <div class="onboarding-body">
        <div class="lang-selection-container">
          ${langCheckboxes}
        </div>
        <p class="onboarding-hint">Select one for best accuracy, or multiple for auto-detection</p>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-primary btn-lg" id="continue-btn">
          Continue
        </button>
        ${renderStepIndicator(3)}
      </div>
    </div>
  `;

  for (const checkbox of document.querySelectorAll(".lang-check")) {
    checkbox.addEventListener("change", handleLanguageChange);
  }

  document
    .getElementById("continue-btn")
    ?.addEventListener("click", handleLanguageContinue);
  attachStepIndicatorListeners();
}

function handleLanguageChange(e: Event): void {
  const target = e.target as HTMLInputElement;
  const lang = target.dataset.lang as string;

  if (target.checked) {
    if (!state.selectedLanguages.includes(lang)) {
      state.selectedLanguages.push(lang);
    }
  } else {
    if (state.selectedLanguages.length <= 1) {
      target.checked = true;
      return;
    }
    state.selectedLanguages = state.selectedLanguages.filter((l) => l !== lang);
  }
}

async function handleLanguageContinue(): Promise<void> {
  try {
    const currentSettings = await getSettings();
    await updateSettings({
      ...currentSettings,
      languages: state.selectedLanguages,
    });
  } catch (err) {
    console.error("Failed to save language selection:", err);
  }

  goToStep(4);
}

function renderPermissions(): void {
  if (!container) {
    return;
  }

  const perms = state.permissions;
  const isMac = navigator.userAgent.includes("Mac");

  if (!isMac) {
    goToStep(5);
    return;
  }

  const allGranted =
    perms?.microphone === "granted" && perms?.accessibility === "granted";

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(PersonStanding)}
        </div>
        <h1 class="onboarding-title">Permissions</h1>
        <p class="onboarding-desc">Fing needs a few permissions to work properly</p>
      </div>
      <div class="onboarding-body">
        <div class="permissions-list">
          <div class="permission-row">
            <div class="permission-info">
              ${createIcon(Mic)}
              <div>
                <div class="permission-label">Microphone Access</div>
                <div class="permission-desc">Required to capture your voice</div>
              </div>
            </div>
            ${renderMicPermissionStatus(perms?.microphone)}
          </div>

          <div class="permission-row">
            <div class="permission-info">
              ${createIcon(PersonStanding)}
              <div>
                <div class="permission-label">Accessibility Access</div>
                <div class="permission-desc">Required for global hotkey and text pasting</div>
              </div>
            </div>
            ${renderAccessibilityPermissionStatus(perms?.accessibility)}
          </div>
        </div>

        ${
          allGranted
            ? ""
            : `
          <p class="onboarding-hint">After granting permissions in System Settings, click Restart to apply changes</p>
        `
        }
      </div>
      <div class="onboarding-footer">
        ${
          allGranted
            ? `<button class="btn btn-primary btn-lg" id="continue-btn">Continue</button>`
            : `<button class="btn btn-primary btn-lg" id="restart-btn">${createIcon(RefreshCw)} Restart to Apply</button>`
        }
        ${renderStepIndicator(4)}
      </div>
    </div>
  `;

  document
    .getElementById("grant-microphone-btn")
    ?.addEventListener("click", handleGrantMicrophone);
  document
    .getElementById("grant-accessibility-btn")
    ?.addEventListener("click", handleGrantAccessibility);
  document
    .getElementById("continue-btn")
    ?.addEventListener("click", handlePermissionsContinue);
  document
    .getElementById("restart-btn")
    ?.addEventListener("click", handleRestartForPermissions);
  attachStepIndicatorListeners();
}

async function handlePermissionsContinue(): Promise<void> {
  try {
    const currentSettings = await getSettings();
    if (currentSettings.onboardingStep !== null) {
      await updateSettings({
        ...currentSettings,
        onboardingStep: null,
      });
    }
  } catch (err) {
    console.error("Failed to clear onboarding step:", err);
  }
  goToStep(5);
}

async function handleRestartForPermissions(): Promise<void> {
  try {
    const currentSettings = await getSettings();
    await updateSettings({
      ...currentSettings,
      onboardingStep: 4,
    });
  } catch (err) {
    console.error("Failed to save onboarding step:", err);
  }

  await relaunchApp();
}

function renderHotkeyStep(): void {
  if (!container) {
    return;
  }

  const displayKey = state.capturedHotkey ?? state.selectedHotkey;
  const hasNewKey =
    state.capturedHotkey && state.capturedHotkey !== state.selectedHotkey;

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(Keyboard)}
        </div>
        <h1 class="onboarding-title">Set Recording Hotkey</h1>
        <p class="onboarding-desc">Press a key to set your recording hotkey</p>
      </div>
      <div class="onboarding-body">
        <div class="hotkey-capture-area">
          <div class="hotkey-preview">
            <span class="hotkey-key ${hasNewKey ? "captured" : ""}">${formatKeyForDisplay(displayKey)}</span>
            ${hasNewKey ? '<span class="hotkey-new-badge">New</span>' : ""}
          </div>
          <p class="hotkey-hint">Press any key or key combination</p>
          <button class="hotkey-fn-link" id="use-fn-btn">Use Fn key instead</button>
        </div>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-primary btn-lg" id="continue-btn">
          Continue
        </button>
        ${renderStepIndicator(5)}
      </div>
    </div>
  `;

  // Set up hotkey capture
  if (!hotkeyKeyHandler) {
    hotkeyKeyHandler = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      if (e.key === "Escape") {
        state.capturedHotkey = null;
        render();
        return;
      }

      const hotkey = keyEventToHotkey(e);
      if (hotkey) {
        state.capturedHotkey = hotkey;
        render();
      }
    };
    document.addEventListener("keydown", hotkeyKeyHandler);
  }

  document.getElementById("use-fn-btn")?.addEventListener("click", () => {
    state.capturedHotkey = "Fn";
    render();
  });

  document
    .getElementById("continue-btn")
    ?.addEventListener("click", handleHotkeyContinue);
  attachStepIndicatorListeners();
}

async function handleHotkeyContinue(): Promise<void> {
  const finalHotkey = state.capturedHotkey ?? state.selectedHotkey;

  try {
    const currentSettings = await getSettings();
    await updateSettings({
      ...currentSettings,
      hotkey: finalHotkey,
    });
    state.selectedHotkey = finalHotkey;
  } catch (err) {
    console.error("Failed to save hotkey:", err);
  }

  if (hotkeyKeyHandler) {
    document.removeEventListener("keydown", hotkeyKeyHandler);
    hotkeyKeyHandler = null;
  }

  goToStep(6);
}

function renderMicSelection(): void {
  if (!container) {
    return;
  }

  const micTest = state.micTest;
  const level = micTest?.peakLevel ?? 0;
  const levelPercent = Math.min(level * 100, 100);

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(Mic)}
        </div>
        <h1 class="onboarding-title">Select Microphone</h1>
        <p class="onboarding-desc">Choose which microphone to use for recording</p>
      </div>
      <div class="onboarding-body">
        <div class="mic-test-container">
          <div class="mic-select-row">
            <label for="mic-select">Microphone:</label>
            <select id="mic-select" class="settings-select">
              ${state.audioDevices
                .map(
                  (d) => `
                <option value="${escapeHtml(d.id)}" ${d.id === state.selectedDeviceId || (state.selectedDeviceId === null && d.isDefault) ? "selected" : ""}>
                  ${escapeHtml(d.name)}${d.isDefault ? " (Default)" : ""}
                </option>
              `
                )
                .join("")}
            </select>
          </div>

          <div class="audio-level-container">
            <div class="audio-level-label">Audio Level</div>
            <div class="audio-level-bar">
              <div class="audio-level-fill ${levelPercent > 10 ? "active" : ""}" style="width: ${levelPercent}%"></div>
            </div>
          </div>

          <div class="mic-test-prompt ${state.audioDetected ? "success" : ""}">
            ${
              state.audioDetected
                ? `${createIcon(CheckCircle)} Audio detected! Your microphone is working.`
                : `${createIcon(Mic)} Say something to test...`
            }
          </div>
        </div>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-primary btn-lg" id="continue-btn">
          Continue
        </button>
        ${renderStepIndicator(6)}
      </div>
    </div>
  `;

  document
    .getElementById("mic-select")
    ?.addEventListener("change", handleMicChange);
  document
    .getElementById("continue-btn")
    ?.addEventListener("click", () => goToStep(7));
  attachStepIndicatorListeners();
}

function renderTestStep(): void {
  if (!container) {
    return;
  }

  const hotkey = state.selectedHotkey;
  const hasText = state.testText.trim().length > 0;

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(Mic)}
        </div>
        <h1 class="onboarding-title">Test Transcription</h1>
        <p class="onboarding-desc">Hold <span class="hotkey-key-inline">${formatKeyForDisplay(hotkey)}</span> and speak</p>
      </div>
      <div class="onboarding-body">
        <input
          type="text"
          id="test-input"
          class="test-input"
          placeholder="Your transcription will appear here..."
          value="${escapeHtml(state.testText)}"
        />
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-primary btn-lg" id="finish-btn" ${hasText ? "" : "disabled"}>
          Finish Setup
        </button>
        ${hasText ? "" : '<p class="onboarding-hint">Complete a test transcription to continue</p>'}
        ${renderStepIndicator(7)}
      </div>
    </div>
  `;

  // Auto-focus the input
  const input = document.getElementById("test-input") as HTMLInputElement;
  input?.focus();

  // Watch for input changes (text gets pasted by the hotkey)
  input?.addEventListener("input", (e) => {
    state.testText = (e.target as HTMLInputElement).value;
    const finishBtn = document.getElementById(
      "finish-btn"
    ) as HTMLButtonElement;
    const hint = document.querySelector(".onboarding-hint");
    if (state.testText.trim().length > 0) {
      finishBtn?.removeAttribute("disabled");
      hint?.remove();
    }
  });

  document
    .getElementById("finish-btn")
    ?.addEventListener("click", () => goToStep(8));
  attachStepIndicatorListeners();
}

function renderCompletion(): void {
  if (!container) {
    return;
  }

  const hotkey = state.selectedHotkey;

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon success">
          ${createIcon(CheckCircle)}
        </div>
        <h1 class="onboarding-title">You're all set!</h1>
        <p class="onboarding-desc">Start using Fing to transcribe your speech</p>
      </div>
      <div class="onboarding-body">
        <div class="completion-instructions">
          <div class="instruction-item">
            <span class="instruction-key">${formatKeyForDisplay(hotkey)}</span>
            <span>Press and hold to start recording</span>
          </div>
          <div class="instruction-item">
            <span class="instruction-icon">${createIcon(Check)}</span>
            <span>Release to transcribe and paste</span>
          </div>
        </div>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-primary btn-lg" id="start-btn">
          Start Using Fing
        </button>
      </div>
    </div>
  `;

  document
    .getElementById("start-btn")
    ?.addEventListener("click", handleComplete);
}

async function goToStep(step: OnboardingStep): Promise<void> {
  await stopPolling();
  state.step = step;

  if (step === 4) {
    handleRequestPermissions();
  }

  if (step === 5) {
    state.capturedHotkey = null;
  }

  if (step === 6) {
    await loadAudioDevices();
    await initMicTest();
    startMicTestPolling();
  }

  if (step === 7) {
    state.testText = "";
    // Ensure mic test from step 6 is stopped (extra safety)
    try {
      await stopMicTest();
    } catch {
      // Ignore - might not be running
    }
    // Enable test mode so the hotkey works during onboarding
    try {
      await enableOnboardingTestMode();
      console.log("[onboarding] Test mode enabled");
    } catch (err) {
      console.error("Failed to enable test mode:", err);
    }
    // Listen for transcription results
    testResultUnlisten = await listen<string>(
      "test-transcription-result",
      (event) => {
        state.testText = event.payload;
        render();
      }
    );
  }

  render();
}

function handleStartDownload(): void {
  state.downloadError = null;
  state.downloadProgress = {
    bytesDownloaded: 0,
    totalBytes: 0,
    percentage: 0,
    status: "downloading",
  };
  render();

  startModelDownload().catch((err) => {
    console.error("Download error:", err);
  });

  startDownloadPolling();
}

function handleCancelDownload(): void {
  stopPolling();
  state.downloadProgress = null;
  render();
}

async function handleSelectFile(): Promise<void> {
  const path = await selectModelFile();
  if (path) {
    state.downloadProgress = {
      bytesDownloaded: 0,
      totalBytes: 0,
      percentage: 100,
      status: "complete",
    };
    render();
  }
}

async function handleRequestPermissions(): Promise<void> {
  state.permissions = await requestPermissions();
  render();
}

async function handleGrantAccessibility(): Promise<void> {
  await requestAccessibilityPermission();
  setTimeout(async () => {
    state.permissions = await requestPermissions();
    render();
  }, 1000);
}

async function handleGrantMicrophone(): Promise<void> {
  await requestMicrophonePermission();
  setTimeout(async () => {
    state.permissions = await requestPermissions();
    render();
  }, 1000);
}

async function loadAudioDevices(): Promise<void> {
  const [devices, currentSettings] = await Promise.all([
    getAudioDevices(),
    getSettings().catch(() => null),
  ]);
  state.audioDevices = devices;

  if (currentSettings?.selectedMicrophoneId) {
    state.selectedDeviceId = currentSettings.selectedMicrophoneId;
  } else if (!state.selectedDeviceId) {
    const defaultDevice = state.audioDevices.find((d) => d.isDefault);
    state.selectedDeviceId = defaultDevice?.id ?? null;
  }
  render();
}

async function handleMicChange(e: Event): Promise<void> {
  const select = e.target as HTMLSelectElement;
  state.selectedDeviceId = select.value || null;
  state.audioDetected = false;

  try {
    await stopMicTest();
    await startMicTest(state.selectedDeviceId);
    console.log("[onboarding] Switched to device:", state.selectedDeviceId);
  } catch (err) {
    console.error("Failed to switch mic:", err);
  }

  await persistSelectedDevice(state.selectedDeviceId);
}

function startDownloadPolling(): void {
  console.log("[onboarding] Starting download polling");
  downloadPollInterval = window.setInterval(async () => {
    const progress = await getDownloadProgress();
    console.log("[onboarding] Download progress:", progress);
    state.downloadProgress = progress;

    if (progress.status === "complete" || progress.status === "failed") {
      console.log(
        "[onboarding] Download finished with status:",
        progress.status
      );
      stopPolling();
    }

    render();
  }, 500);
}

async function initMicTest(): Promise<void> {
  try {
    await startMicTest(state.selectedDeviceId);
    console.log("[onboarding] Mic test started");
  } catch (e) {
    console.error("Failed to start mic test:", e);
  }
}

function startMicTestPolling(): void {
  micTestInterval = window.setInterval(async () => {
    try {
      const test = await getMicTestLevel();
      state.micTest = test;

      if (test.isReceivingAudio && test.peakLevel > 0.1) {
        state.audioDetected = true;
      }

      updateAudioLevel();
    } catch (e) {
      console.error("Mic test error:", e);
    }
  }, 100);
}

function updateAudioLevel(): void {
  const level = state.micTest?.peakLevel ?? 0;
  const levelPercent = Math.min(level * 100, 100);

  const levelFill = document.querySelector(".audio-level-fill") as HTMLElement;
  if (levelFill) {
    levelFill.style.width = `${levelPercent}%`;
    if (levelPercent > 10) {
      levelFill.classList.add("active");
    } else {
      levelFill.classList.remove("active");
    }
  }

  const prompt = document.querySelector(".mic-test-prompt") as HTMLElement;
  if (prompt && state.audioDetected) {
    prompt.classList.add("success");
    prompt.innerHTML = `${createIcon(CheckCircle)} Audio detected! Your microphone is working.`;
  }
}

async function stopPolling(): Promise<void> {
  if (downloadPollInterval) {
    clearInterval(downloadPollInterval);
    downloadPollInterval = null;
  }
  if (micTestInterval) {
    clearInterval(micTestInterval);
    micTestInterval = null;
    try {
      await stopMicTest();
    } catch (e) {
      console.error("Error stopping mic test:", e);
    }
  }
  if (testResultUnlisten) {
    testResultUnlisten();
    testResultUnlisten = null;
  }
  // Disable test mode when leaving test step
  try {
    await disableOnboardingTestMode();
  } catch {
    // Ignore errors - test mode might not have been enabled
  }
}

async function handleComplete(): Promise<void> {
  await stopPolling();
  try {
    await completeSetup();
    window.dispatchEvent(new CustomEvent("setup-complete"));
  } catch (err) {
    console.error("Failed to complete setup:", err);
    state.step = 2;
    state.downloadProgress = null;
    state.downloadError = String(err);
    render();
  }
}

export async function renderOnboarding(el: HTMLElement): Promise<void> {
  container = el;

  // Load saved settings first
  let savedSettings: Settings | null = null;
  try {
    savedSettings = await getSettings();
  } catch (e) {
    console.error("Failed to load settings:", e);
  }

  state = {
    step: 1,
    downloadProgress: null,
    downloadError: null,
    permissions: null,
    audioDevices: [],
    selectedDeviceId: null,
    micTest: null,
    audioDetected: false,
    selectedLanguages: savedSettings?.languages ?? ["en"],
    selectedHotkey: savedSettings?.hotkey ?? "F8",
    capturedHotkey: null,
    testText: "",
  };

  // Check if this is a manual reset
  const isReset = sessionStorage.getItem("onboarding-reset") === "true";
  if (isReset) {
    sessionStorage.removeItem("onboarding-reset");
    render();
    return;
  }

  // Check if resuming from a saved step (after restart)
  if (savedSettings?.onboardingStep) {
    const savedStep = savedSettings.onboardingStep as OnboardingStep;
    console.log("[onboarding] Resuming at step:", savedStep);

    // If resuming at permissions step, refresh permissions
    if (savedStep === 4) {
      state.step = savedStep;
      state.permissions = await requestPermissions();
      render();
      return;
    }

    state.step = savedStep;
    render();
    return;
  }

  // Check if model already exists
  try {
    const modelStatus = await checkModelExists();
    if (modelStatus.isValid) {
      state.downloadProgress = {
        bytesDownloaded: 0,
        totalBytes: 0,
        percentage: 100,
        status: "complete",
      };
      state.step = 3;
    }
  } catch (e) {
    console.error("Failed to check model status:", e);
  }

  render();
}

export function cleanupOnboarding(): void {
  stopPolling();
  if (hotkeyKeyHandler) {
    document.removeEventListener("keydown", hotkeyKeyHandler);
    hotkeyKeyHandler = null;
  }
  container = null;
}
