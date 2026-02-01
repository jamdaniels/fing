import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Check,
  CheckCircle,
  Download,
  Globe,
  Keyboard,
  Mic,
  PersonStanding,
} from "lucide";
import { createIcon, escapeHtml } from "../lib/icons";
import {
  checkModelExists,
  completeSetup,
  disableOnboardingTestMode,
  enableOnboardingTestMode,
  getAudioDevices,
  getDownloadProgress,
  getSettings,
  hotkeyPress,
  hotkeyRelease,
  relaunchApp,
  requestAccessibilityPermission,
  requestMicrophonePermission,
  requestPermissions,
  startModelDownload,
  updateHotkey,
  updateSettings,
} from "../lib/ipc";
import type {
  AudioDevice,
  DownloadProgress,
  PermissionStatus,
  Settings,
} from "../lib/types";

type OnboardingStep = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8;

interface OnboardingState {
  step: OnboardingStep;
  downloadProgress: DownloadProgress | null;
  downloadError: string | null;
  completeError: string | null;
  isCompleting: boolean;
  permissions: PermissionStatus | null;
  audioDevices: AudioDevice[];
  selectedDeviceId: string | null;
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
  completeError: null,
  isCompleting: false,
  permissions: null,
  audioDevices: [],
  selectedDeviceId: null,
  selectedLanguages: ["en"],
  selectedHotkey: "F8",
  capturedHotkey: null,
  testText: "",
};

let container: HTMLElement | null = null;
let downloadPollInterval: number | null = null;
let hotkeyKeyHandler: ((e: KeyboardEvent) => void) | null = null;
let testResultUnlisten: UnlistenFn | null = null;
// Frontend hotkey handling for test step (WebView2 workaround)
let testHotkeyPressed = false;
let testHotkeyKeydown: ((e: KeyboardEvent) => void) | null = null;
let testHotkeyKeyup: ((e: KeyboardEvent) => void) | null = null;

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

function renderDownloadBody(
  isDownloading: boolean,
  isComplete: boolean,
  isFailed: boolean,
  progress: DownloadProgress | null
): string {
  if (isDownloading || isComplete || isFailed) {
    let statusText = "";
    if (isDownloading) {
      statusText = `${formatBytes(progress?.bytesDownloaded ?? 0)} / ${formatBytes(progress?.totalBytes ?? 0)}`;
    } else if (isComplete) {
      statusText = "Download complete!";
    } else if (isFailed) {
      statusText = progress?.errorMessage || "Download failed";
    }

    return `
      <div class="download-progress-container">
        <div class="progress-bar">
          <div class="progress-bar-fill" style="width: ${progress?.percentage ?? 0}%"></div>
        </div>
        <div class="download-status ${isFailed ? "error" : ""}${isComplete ? "success centered-status" : ""}${isDownloading ? "centered-status" : ""}">${statusText}</div>
        ${isFailed ? `<button class="btn btn-primary" id="retry-download-btn">Retry Download</button>` : ""}
      </div>
    `;
  }
  return "";
}

function renderDownloadFooterButton(isComplete: boolean): string {
  if (isComplete) {
    return `<button class="btn btn-primary btn-lg" id="continue-btn">Continue</button>`;
  }
  return `<button class="btn btn-primary btn-lg" id="continue-btn" disabled>Continue</button>`;
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

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(Download)}
        </div>
        <h1 class="onboarding-title">Download Speech Model</h1>
        <p class="onboarding-desc">Fing needs a speech recognition model</p>
      </div>
      <div class="onboarding-body">
        ${state.downloadError ? `<div class="download-status error" style="margin-bottom: 16px;">${state.downloadError}</div>` : ""}
        ${renderDownloadBody(isDownloading, isComplete, isFailed, progress)}
        ${
          isDownloading || isComplete || isFailed
            ? ""
            : `
          <button class="btn btn-outline btn-lg" id="start-download-btn">Download Model</button>
        `
        }
      </div>
      <div class="onboarding-footer">
        ${renderDownloadFooterButton(isComplete)}
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
            : `<button class="btn btn-primary btn-lg" id="restart-btn">Restart to Apply</button>`
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
        <p class="onboarding-desc">Press a key or key combination to set your hotkey</p>
      </div>
      <div class="onboarding-body">
        <div class="hotkey-capture-area">
          <div class="hotkey-preview">
            <span class="hotkey-key ${hasNewKey ? "captured" : ""}">${formatKeyForDisplay(displayKey)}</span>
            ${hasNewKey ? '<span class="hotkey-new-badge">New</span>' : ""}
          </div>
          <button class="hotkey-fn-option" id="use-fn-btn">
            <span class="hotkey-fn-key">fn</span>
            <span>Use Fn key instead</span>
          </button>
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

    // Update the backend's runtime hotkey config so it takes effect immediately
    await updateHotkey(finalHotkey);
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
        <select id="mic-select" class="settings-select mic-select-full">
          ${state.audioDevices
            .map(
              (d) => `
            <option value="${escapeHtml(d.id)}" ${d.id === state.selectedDeviceId || (state.selectedDeviceId === null && d.isDefault) ? "selected" : ""}>
              ${escapeHtml(d.name)}
            </option>
          `
            )
            .join("")}
        </select>
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
        <div
          id="test-input"
          class="test-input test-input-readonly"
        >${state.testText ? escapeHtml(state.testText) : '<span class="test-input-placeholder">Your transcription will appear here...</span>'}</div>
        <p class="onboarding-hint ${hasText ? "invisible" : ""}">Complete a test transcription to continue</p>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-primary btn-lg" id="finish-btn" ${hasText ? "" : "disabled"}>
          Finish Setup
        </button>
        ${renderStepIndicator(7)}
      </div>
    </div>
  `;

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
  const completeError = state.completeError;
  const isCompleting = state.isCompleting;

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(CheckCircle)}
        </div>
        <h1 class="onboarding-title">You're all set!</h1>
        <p class="onboarding-desc">Start using Fing to transcribe your speech</p>
      </div>
      <div class="onboarding-body">
        ${
          completeError
            ? `<div class="download-status error">${escapeHtml(completeError)}</div>`
            : ""
        }
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
        <button class="btn btn-primary btn-lg" id="start-btn" type="button" ${
          isCompleting ? "disabled" : ""
        }>
          ${isCompleting ? "Finishing setup..." : "Start Using Fing"}
        </button>
        <div class="step-indicator-placeholder"></div>
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
    await handleRequestPermissions();
  }

  if (step === 5) {
    state.capturedHotkey = null;
  }

  if (step === 6) {
    await loadAudioDevices();
  }

  if (step === 7) {
    state.testText = "";
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

    // Set up frontend hotkey listener (WebView2 workaround for Windows)
    const hotkey = state.selectedHotkey;
    const hotkeyLower = hotkey.toLowerCase();
    console.log("[onboarding] Setting up frontend hotkey listener for:", hotkey);

    testHotkeyKeydown = (e: KeyboardEvent) => {
      if (testHotkeyPressed) return;
      const keyLower = e.key.toLowerCase();
      // Simple match for function keys and single keys
      if (keyLower === hotkeyLower || (hotkeyLower.startsWith("f") && keyLower === hotkeyLower)) {
        e.preventDefault();
        e.stopPropagation();
        testHotkeyPressed = true;
        hotkeyPress().catch(console.error);
      }
    };

    testHotkeyKeyup = (e: KeyboardEvent) => {
      if (!testHotkeyPressed) return;
      const keyLower = e.key.toLowerCase();
      if (keyLower === hotkeyLower || (hotkeyLower.startsWith("f") && keyLower === hotkeyLower)) {
        e.preventDefault();
        e.stopPropagation();
        testHotkeyPressed = false;
        hotkeyRelease().catch(console.error);
      }
    };

    document.addEventListener("keydown", testHotkeyKeydown);
    document.addEventListener("keyup", testHotkeyKeyup);
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

async function stopPolling(): Promise<void> {
  if (downloadPollInterval) {
    clearInterval(downloadPollInterval);
    downloadPollInterval = null;
  }
  if (testResultUnlisten) {
    testResultUnlisten();
    testResultUnlisten = null;
  }
  // Clean up frontend hotkey listeners
  if (testHotkeyKeydown) {
    document.removeEventListener("keydown", testHotkeyKeydown);
    testHotkeyKeydown = null;
  }
  if (testHotkeyKeyup) {
    document.removeEventListener("keyup", testHotkeyKeyup);
    testHotkeyKeyup = null;
  }
  testHotkeyPressed = false;
  // Disable test mode when leaving test step
  try {
    await disableOnboardingTestMode();
  } catch {
    // Ignore errors - test mode might not have been enabled
  }
}

async function handleComplete(): Promise<void> {
  if (state.isCompleting) {
    return;
  }

  state.isCompleting = true;
  state.completeError = null;
  render();

  stopPolling().catch(() => undefined);

  try {
    await completeSetup();
    window.dispatchEvent(new CustomEvent("setup-complete"));
    await getCurrentWindow().hide();
  } catch {
    state.isCompleting = false;
    state.completeError =
      "Setup did not finish. Please confirm the model download and try again.";
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
    completeError: null,
    isCompleting: false,
    permissions: null,
    audioDevices: [],
    selectedDeviceId: null,
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
      try {
        state.permissions = await requestPermissions();
      } catch (e) {
        console.error("Failed to get permissions:", e);
      }
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
