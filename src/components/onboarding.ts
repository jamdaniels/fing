import { Check, CheckCircle, ChevronRight, Mic, Shield, Upload } from "lucide";
import { createIcon } from "../lib/icons";
import {
  checkModelExists,
  completeSetup,
  getAudioDevices,
  getDownloadProgress,
  getMicTestLevel,
  getSettings,
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
} from "../lib/types";

type OnboardingStep = 1 | 2 | 3 | 4 | 5;

interface OnboardingState {
  step: OnboardingStep;
  downloadProgress: DownloadProgress | null;
  downloadError: string | null;
  permissions: PermissionStatus | null;
  audioDevices: AudioDevice[];
  selectedDeviceId: string | null;
  micTest: MicrophoneTest | null;
  audioDetected: boolean;
}

let state: OnboardingState = {
  step: 1,
  downloadProgress: null,
  downloadError: null,
  permissions: null,
  audioDevices: [],
  selectedDeviceId: null,
  micTest: null,
  audioDetected: false,
};

let container: HTMLElement | null = null;
let downloadPollInterval: number | null = null;
let micTestInterval: number | null = null;

const TOTAL_STEPS = 5;

function renderStepIndicator(currentStep: OnboardingStep): string {
  if (currentStep === 5) {
    return ""; // Don't show on completion
  }

  const dots: string[] = [];
  for (let i = 1; i <= TOTAL_STEPS - 1; i++) {
    // 4 dots (exclude completion)
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

// Use shared createIcon from lib/icons.ts

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

  switch (state.step) {
    case 1:
      renderWelcome();
      break;
    case 2:
      renderDownloadModel();
      break;
    case 3:
      renderPermissions();
      break;
    case 4:
      renderTestMicrophone();
      break;
    case 5:
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
      <div class="onboarding-skip">
        <button class="btn-link" id="skip-btn">Skip Setup</button>
      </div>
      <div class="onboarding-content">
        <div class="onboarding-icon">
          <svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M2 10v3"/>
            <path d="M6 6v11"/>
            <path d="M10 3v18"/>
            <path d="M14 8v7"/>
            <path d="M18 5v13"/>
            <path d="M22 10v3"/>
          </svg>
        </div>
        <h1 class="onboarding-title">Welcome to Fing</h1>
        <p class="onboarding-desc">Fast, private, local speech-to-text</p>
        <ul class="onboarding-features">
          <li>${createIcon(Shield)} All processing happens locally on your device</li>
          <li>${createIcon(Mic)} Microphone is only active while you hold the hotkey</li>
          <li>${createIcon(Check)} Your audio never leaves your computer</li>
        </ul>
        <button class="btn btn-primary btn-lg" id="get-started-btn">
          Get Started
          ${createIcon(ChevronRight)}
        </button>
      </div>
      ${renderStepIndicator(1)}
    </div>
  `;

  document
    .getElementById("skip-btn")
    ?.addEventListener("click", handleSkipSetup);
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
      <div class="onboarding-skip">
        <button class="btn-link" id="skip-btn">Skip Setup</button>
      </div>
      <div class="onboarding-content">
        <h1 class="onboarding-title">Download Speech Model</h1>
        <p class="onboarding-desc">Fing needs a speech recognition model (~75 MB, one-time download)</p>

        ${state.downloadError ? `<div class="download-status error" style="margin-bottom: 16px;">${state.downloadError}</div>` : ""}

        ${renderDownloadButton(isDownloading, isComplete, isFailed, statusText, progress)}

        ${
          isComplete
            ? `
          <button class="btn btn-primary btn-lg" id="continue-btn" style="margin-top: 16px;">
            Continue
            ${createIcon(ChevronRight)}
          </button>
        `
            : ""
        }

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
      ${renderStepIndicator(2)}
    </div>
  `;

  document
    .getElementById("skip-btn")
    ?.addEventListener("click", handleSkipSetup);
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

function renderPermissions(): void {
  if (!container) {
    return;
  }

  const perms = state.permissions;
  const isMac = navigator.userAgent.includes("Mac");

  if (!isMac) {
    goToStep(4);
    return;
  }

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-skip">
        <button class="btn-link" id="skip-btn">Skip Setup</button>
      </div>
      <div class="onboarding-content">
        <h1 class="onboarding-title">Permissions</h1>
        <p class="onboarding-desc">Fing needs a few permissions to work properly</p>

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
              ${createIcon(Shield)}
              <div>
                <div class="permission-label">Accessibility Access</div>
                <div class="permission-desc">Required for global hotkey and text pasting</div>
              </div>
            </div>
            ${renderAccessibilityPermissionStatus(perms?.accessibility)}
          </div>
        </div>

        <button class="btn btn-primary btn-lg" id="continue-btn">
          Continue
          ${createIcon(ChevronRight)}
        </button>
      </div>
      ${renderStepIndicator(3)}
    </div>
  `;

  document
    .getElementById("skip-btn")
    ?.addEventListener("click", handleSkipSetup);
  document
    .getElementById("grant-microphone-btn")
    ?.addEventListener("click", handleGrantMicrophone);
  document
    .getElementById("grant-accessibility-btn")
    ?.addEventListener("click", handleGrantAccessibility);
  document
    .getElementById("continue-btn")
    ?.addEventListener("click", () => goToStep(4));
  attachStepIndicatorListeners();
}

function renderTestMicrophone(): void {
  if (!container) {
    return;
  }

  const micTest = state.micTest;
  const level = micTest?.peakLevel ?? 0;
  const levelPercent = Math.min(level * 100, 100);

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-skip">
        <button class="btn-link" id="skip-btn">Skip Setup</button>
      </div>
      <div class="onboarding-content">
        <h1 class="onboarding-title">Test Your Microphone</h1>
        <p class="onboarding-desc">Make sure your microphone is working properly</p>

        <div class="mic-test-container">
          <div class="mic-select-row">
            <label for="mic-select">Microphone:</label>
            <select id="mic-select" class="btn btn-outline">
              ${state.audioDevices
                .map(
                  (d) => `
                <option value="${d.id}" ${d.id === state.selectedDeviceId || (state.selectedDeviceId === null && d.isDefault) ? "selected" : ""}>
                  ${d.name}${d.isDefault ? " (Default)" : ""}
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

        <button class="btn btn-primary btn-lg" id="finish-btn">
          Finish Setup
          ${createIcon(ChevronRight)}
        </button>
      </div>
      ${renderStepIndicator(4)}
    </div>
  `;

  document
    .getElementById("skip-btn")
    ?.addEventListener("click", handleSkipSetup);
  document
    .getElementById("mic-select")
    ?.addEventListener("change", handleMicChange);
  document
    .getElementById("finish-btn")
    ?.addEventListener("click", () => goToStep(5));
  attachStepIndicatorListeners();
}

function renderCompletion(): void {
  if (!container) {
    return;
  }

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-content">
        <div class="onboarding-icon success">
          ${createIcon(CheckCircle)}
        </div>
        <h1 class="onboarding-title">You're all set!</h1>

        <div class="completion-instructions">
          <div class="instruction-item">
            <span class="instruction-key">F8</span>
            <span>Press and hold to start recording</span>
          </div>
          <div class="instruction-item">
            <span class="instruction-icon">${createIcon(Check)}</span>
            <span>Release to transcribe and paste</span>
          </div>
        </div>

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

  if (step === 3) {
    handleRequestPermissions();
  }

  if (step === 4) {
    await loadAudioDevices();
    await initMicTest();
    startMicTestPolling();
  }

  render();
}

async function handleSkipSetup(): Promise<void> {
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

function handleStartDownload(): void {
  state.downloadError = null;
  state.downloadProgress = {
    bytesDownloaded: 0,
    totalBytes: 0,
    percentage: 0,
    status: "downloading",
  };
  render();

  // Start download in background (don't await - let polling handle progress)
  startModelDownload().catch((err) => {
    console.error("Download error:", err);
  });

  // Start polling immediately to track progress
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
  // This opens System Preferences
  await requestAccessibilityPermission();
  // Wait a moment then refresh status
  setTimeout(async () => {
    state.permissions = await requestPermissions();
    render();
  }, 1000);
}

async function handleGrantMicrophone(): Promise<void> {
  // This opens System Preferences to Microphone
  await requestMicrophonePermission();
  // Wait a moment then refresh status
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

  // Restart mic test with new device
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

      // Update only the audio level elements, not the whole page
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
    // Stop the backend mic test
    try {
      await stopMicTest();
    } catch (e) {
      console.error("Error stopping mic test:", e);
    }
  }
}

async function handleComplete(): Promise<void> {
  await stopPolling();
  try {
    await completeSetup();
    window.dispatchEvent(new CustomEvent("setup-complete"));
  } catch (err) {
    console.error("Failed to complete setup:", err);
    // Show error to user - go back to step 2 (download) if model issue
    state.step = 2;
    state.downloadProgress = null;
    state.downloadError = String(err);
    render();
  }
}

export async function renderOnboarding(el: HTMLElement): Promise<void> {
  container = el;
  state = {
    step: 1,
    downloadProgress: null,
    downloadError: null,
    permissions: null,
    audioDevices: [],
    selectedDeviceId: null,
    micTest: null,
    audioDetected: false,
  };

  // Check if model already exists - skip to permissions step if so
  try {
    const modelStatus = await checkModelExists();
    if (modelStatus.isValid) {
      // Model exists, mark download as complete and skip to permissions
      state.downloadProgress = {
        bytesDownloaded: 0,
        totalBytes: 0,
        percentage: 100,
        status: "complete",
      };
      state.step = 3; // Go to permissions step
      handleRequestPermissions(); // Load permission status
    }
  } catch (e) {
    console.error("Failed to check model status:", e);
  }

  render();
}

export function cleanupOnboarding(): void {
  stopPolling();
  container = null;
}
