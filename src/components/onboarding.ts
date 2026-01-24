import {
  startModelDownload,
  getDownloadProgress,
  selectModelFile,
  requestPermissions,
  getAudioDevices,
  testMicrophone,
  completeSetup,
  setAudioDevice,
} from "../lib/ipc";
import type {
  DownloadProgress,
  PermissionStatus,
  AudioDevice,
  MicrophoneTest,
} from "../lib/types";
import { createIcon } from "../lib/icons";
import { Check, ChevronRight, Upload, Mic, Shield, CheckCircle } from "lucide";

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

// Use shared createIcon from lib/icons.ts

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / Math.pow(k, i)).toFixed(1)} ${sizes[i]}`;
}

function render(): void {
  if (!container) return;

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
  }
}

function renderWelcome(): void {
  if (!container) return;

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
    </div>
  `;

  document.getElementById("skip-btn")?.addEventListener("click", handleSkipSetup);
  document.getElementById("get-started-btn")?.addEventListener("click", () => goToStep(2));
}

function renderDownloadModel(): void {
  if (!container) return;

  const progress = state.downloadProgress;
  const isDownloading = progress?.status === "downloading" || progress?.status === "verifying";
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

        ${isDownloading || isComplete ? `
          <div class="download-progress-container">
            <div class="progress-bar">
              <div class="progress-bar-fill" style="width: ${progress?.percentage ?? 0}%"></div>
            </div>
            <div class="download-status ${isFailed ? "error" : ""}">${statusText}</div>
            ${isDownloading ? `<button class="btn btn-secondary" id="cancel-download-btn">Cancel</button>` : ""}
          </div>
        ` : isFailed ? `
          <div class="download-progress-container">
            <div class="download-status error">${statusText}</div>
            <button class="btn btn-primary" id="retry-download-btn">Retry Download</button>
          </div>
        ` : `
          <button class="btn btn-primary btn-lg" id="start-download-btn">
            Download Model
          </button>
        `}

        ${isComplete ? `
          <button class="btn btn-primary btn-lg" id="continue-btn" style="margin-top: 16px;">
            Continue
            ${createIcon(ChevronRight)}
          </button>
        ` : ""}

        ${!isDownloading && !isComplete ? `
          <div class="onboarding-divider">
            <span>OR</span>
          </div>
          <button class="btn btn-outline" id="select-file-btn">
            ${createIcon(Upload)}
            Already have the model file? Choose File...
          </button>
        ` : ""}
      </div>
    </div>
  `;

  document.getElementById("skip-btn")?.addEventListener("click", handleSkipSetup);
  document.getElementById("start-download-btn")?.addEventListener("click", handleStartDownload);
  document.getElementById("retry-download-btn")?.addEventListener("click", handleStartDownload);
  document.getElementById("cancel-download-btn")?.addEventListener("click", handleCancelDownload);
  document.getElementById("select-file-btn")?.addEventListener("click", handleSelectFile);
  document.getElementById("continue-btn")?.addEventListener("click", () => goToStep(3));
}

function renderPermissions(): void {
  if (!container) return;

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
            <div class="permission-status ${perms?.microphone === "granted" ? "granted" : perms?.microphone === "denied" ? "denied" : ""}">
              ${perms?.microphone === "granted" ? "Granted" : perms?.microphone === "denied" ? "Denied" : "Unknown"}
            </div>
          </div>

          <div class="permission-row">
            <div class="permission-info">
              ${createIcon(Shield)}
              <div>
                <div class="permission-label">Accessibility Access</div>
                <div class="permission-desc">Required for global hotkey and text pasting</div>
              </div>
            </div>
            ${perms?.accessibility === "granted" ? `
              <div class="permission-status granted">Granted</div>
            ` : perms?.accessibility === "not-applicable" ? `
              <div class="permission-status">N/A</div>
            ` : `
              <button class="btn btn-outline btn-sm" id="grant-accessibility-btn">Grant</button>
            `}
          </div>
        </div>

        <button class="btn btn-primary btn-lg" id="continue-btn">
          Continue
          ${createIcon(ChevronRight)}
        </button>
      </div>
    </div>
  `;

  document.getElementById("skip-btn")?.addEventListener("click", handleSkipSetup);
  document.getElementById("grant-accessibility-btn")?.addEventListener("click", handleRequestPermissions);
  document.getElementById("continue-btn")?.addEventListener("click", () => goToStep(4));
}

function renderTestMicrophone(): void {
  if (!container) return;

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
              ${state.audioDevices.map(d => `
                <option value="${d.id}" ${d.id === state.selectedDeviceId || (state.selectedDeviceId === null && d.isDefault) ? "selected" : ""}>
                  ${d.name}${d.isDefault ? " (Default)" : ""}
                </option>
              `).join("")}
            </select>
          </div>

          <div class="audio-level-container">
            <div class="audio-level-label">Audio Level</div>
            <div class="audio-level-bar">
              <div class="audio-level-fill ${levelPercent > 10 ? "active" : ""}" style="width: ${levelPercent}%"></div>
            </div>
          </div>

          <div class="mic-test-prompt ${state.audioDetected ? "success" : ""}">
            ${state.audioDetected
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
    </div>
  `;

  document.getElementById("skip-btn")?.addEventListener("click", handleSkipSetup);
  document.getElementById("mic-select")?.addEventListener("change", handleMicChange);
  document.getElementById("finish-btn")?.addEventListener("click", () => goToStep(5));
}

function renderCompletion(): void {
  if (!container) return;

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

  document.getElementById("start-btn")?.addEventListener("click", handleComplete);
}

function goToStep(step: OnboardingStep): void {
  stopPolling();
  state.step = step;

  if (step === 3) {
    handleRequestPermissions();
  }

  if (step === 4) {
    loadAudioDevices();
    startMicTest();
  }

  render();
}

async function handleSkipSetup(): Promise<void> {
  stopPolling();
  await completeSetup();
  window.dispatchEvent(new CustomEvent("setup-complete"));
}

async function handleStartDownload(): Promise<void> {
  state.downloadError = null;
  state.downloadProgress = {
    bytesDownloaded: 0,
    totalBytes: 0,
    percentage: 0,
    status: "downloading",
  };
  render();

  await startModelDownload();
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

async function loadAudioDevices(): Promise<void> {
  state.audioDevices = await getAudioDevices();
  const defaultDevice = state.audioDevices.find(d => d.isDefault);
  if (defaultDevice && !state.selectedDeviceId) {
    state.selectedDeviceId = defaultDevice.id;
  }
  render();
}

async function handleMicChange(e: Event): Promise<void> {
  const select = e.target as HTMLSelectElement;
  state.selectedDeviceId = select.value;
  await setAudioDevice(state.selectedDeviceId);
  state.audioDetected = false;
  render();
}

function startDownloadPolling(): void {
  downloadPollInterval = window.setInterval(async () => {
    const progress = await getDownloadProgress();
    state.downloadProgress = progress;

    if (progress.status === "complete" || progress.status === "failed") {
      stopPolling();
    }

    render();
  }, 500);
}

function startMicTest(): void {
  micTestInterval = window.setInterval(async () => {
    const test = await testMicrophone();
    state.micTest = test;

    if (test.isReceivingAudio && test.peakLevel > 0.1) {
      state.audioDetected = true;
    }

    render();
  }, 100);
}

function stopPolling(): void {
  if (downloadPollInterval) {
    clearInterval(downloadPollInterval);
    downloadPollInterval = null;
  }
  if (micTestInterval) {
    clearInterval(micTestInterval);
    micTestInterval = null;
  }
}

async function handleComplete(): Promise<void> {
  stopPolling();
  await completeSetup();
  window.dispatchEvent(new CustomEvent("setup-complete"));
}

export function renderOnboarding(el: HTMLElement): void {
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
  render();
}

export function cleanupOnboarding(): void {
  stopPolling();
  container = null;
}
