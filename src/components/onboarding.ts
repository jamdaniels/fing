import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  BarChart3,
  Check,
  CheckCircle,
  ClipboardList,
  Download,
  Globe,
  Keyboard,
  LoaderCircle,
  Mic,
  Monitor,
  PersonStanding,
  Search,
  Shield,
} from "lucide";
import { createIcon, escapeHtml } from "../lib/icons";
import {
  completeSetup,
  disableOnboardingTestMode,
  downloadModel,
  enableOnboardingTestMode,
  getAudioDevices,
  getDownloadProgress,
  getModels,
  getSettings,
  hotkeyPress,
  hotkeyRelease,
  relaunchApp,
  requestAccessibilityPermission,
  requestMicrophonePermission,
  requestPermissions,
  updateHotkey,
  updateSettings,
} from "../lib/ipc";
import type {
  AudioDevice,
  DownloadProgress,
  HistoryMode,
  ModelInfo,
  ModelVariant,
  PermissionStatus,
  Settings,
} from "../lib/types";

type OnboardingStep = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9;

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
  selectedModelVariant: ModelVariant;
  selectedHistoryMode: HistoryMode;
  models: ModelInfo[];
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
  selectedHotkey: "F9",
  capturedHotkey: null,
  testText: "",
  selectedModelVariant: "small",
  selectedHistoryMode: "30d",
  models: [],
};

let container: HTMLElement | null = null;
let downloadPollInterval: number | null = null;
let hotkeyKeyHandler: ((e: KeyboardEvent) => void) | null = null;
let testResultUnlisten: UnlistenFn | null = null;
// Frontend hotkey handling for test step (WebView2 workaround)
let testHotkeyPressed = false;
let testHotkeyKeydown: ((e: KeyboardEvent) => void) | null = null;
let testHotkeyKeyup: ((e: KeyboardEvent) => void) | null = null;

const TOTAL_STEPS = 9;

function renderStepIndicator(currentStep: OnboardingStep): string {
  if (currentStep === 9) {
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

const FUNCTION_KEY_REGEX = /^F\d+$/i;
const SINGLE_KEY_REGEX = /^[A-Z0-9]$/;
const MODIFIER_EVENT_KEYS = ["Control", "Alt", "Shift", "Meta"];
const SPACE_EVENT_KEYS = [" ", "Space", "Spacebar"];

type ParsedHotkeyConfig = {
  key: string | null;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
};

function getHotkeyModifiers(e: KeyboardEvent): string[] {
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
  return modifiers;
}

function normalizeHotkeyBase(key: string): string | null {
  if (FUNCTION_KEY_REGEX.test(key)) {
    return key.toUpperCase();
  }
  if (key.length === 1 && SINGLE_KEY_REGEX.test(key.toUpperCase())) {
    return key.toUpperCase();
  }
  return null;
}

function isSpaceKeyEvent(e: KeyboardEvent): boolean {
  return e.code === "Space" || SPACE_EVENT_KEYS.includes(e.key);
}

function keyEventToHotkey(e: KeyboardEvent): string | null {
  if (e.key === "Escape") {
    return null;
  }

  const modifiers = getHotkeyModifiers(e);

  if (MODIFIER_EVENT_KEYS.includes(e.key)) {
    return modifiers.length === 2 ? modifiers.join("+") : null;
  }

  if (isSpaceKeyEvent(e)) {
    return modifiers.length === 1 ? [...modifiers, "Space"].join("+") : null;
  }

  const key = normalizeHotkeyBase(e.key);
  if (!key || modifiers.length !== 0) {
    return null;
  }

  return key;
}

function parseHotkeyString(hotkey: string): ParsedHotkeyConfig | null {
  const parts = hotkey.split("+");
  if (parts.length === 0 || parts.length > 2) {
    return null;
  }

  let key: string | null = null;
  let ctrl = false;
  let alt = false;
  let shift = false;
  let meta = false;

  for (const part of parts) {
    const trimmed = part.trim();
    if (!trimmed) {
      return null;
    }

    const lower = trimmed.toLowerCase();
    if (lower === "ctrl" || lower === "control") {
      ctrl = true;
    } else if (lower === "alt" || lower === "option") {
      alt = true;
    } else if (lower === "shift") {
      shift = true;
    } else if (lower === "meta" || lower === "cmd" || lower === "command") {
      meta = true;
    } else if (lower === "space") {
      if (key !== null) {
        return null;
      }
      key = "Space";
    } else if (lower === "fn") {
      if (key !== null) {
        return null;
      }
      key = "Fn";
    } else {
      const normalized = normalizeHotkeyBase(trimmed);
      if (!normalized || key !== null) {
        return null;
      }
      key = normalized;
    }
  }

  const config = { key, ctrl, alt, shift, meta };
  const modifierCount = getParsedModifierCount(config);

  if (parts.length === 1) {
    if (key === "Fn") {
      return modifierCount === 0 ? config : null;
    }
    if (key === "Space" || key === null) {
      return null;
    }
    return modifierCount === 0 ? config : null;
  }

  if (key === null) {
    return modifierCount === 2 ? config : null;
  }

  if (key === "Space") {
    return modifierCount === 1 ? config : null;
  }

  return null;
}

function getParsedModifierCount(config: ParsedHotkeyConfig): number {
  return Number(config.ctrl) + Number(config.alt) + Number(config.shift) + Number(config.meta);
}

function matchesModifierFlags(
  e: KeyboardEvent,
  config: ParsedHotkeyConfig
): boolean {
  return (
    e.ctrlKey === config.ctrl &&
    e.altKey === config.alt &&
    e.shiftKey === config.shift &&
    e.metaKey === config.meta
  );
}

function isConfiguredModifierKey(
  key: string,
  config: ParsedHotkeyConfig
): boolean {
  if (key === "Control") {
    return config.ctrl;
  }
  if (key === "Alt") {
    return config.alt;
  }
  if (key === "Shift") {
    return config.shift;
  }
  if (key === "Meta") {
    return config.meta;
  }
  return false;
}

function shouldReleaseOnKeydown(
  e: KeyboardEvent,
  config: ParsedHotkeyConfig
): boolean {
  if (!matchesModifierFlags(e, config)) {
    return true;
  }

  if (config.key === null) {
    return !isConfiguredModifierKey(e.key, config);
  }

  return false;
}

function matchesHotkey(
  e: KeyboardEvent,
  config: ParsedHotkeyConfig
): boolean {
  if (!matchesModifierFlags(e, config)) {
    return false;
  }

  if (config.key === null) {
    return isConfiguredModifierKey(e.key, config);
  }

  const key = config.key.toLowerCase();
  const eventKey = e.key.toLowerCase();

  if (key.startsWith("f") && key.length <= 3) {
    return eventKey === key;
  }

  if (key === "space") {
    return isSpaceKeyEvent(e);
  }

  return eventKey === key;
}

function matchesHotkeyRelease(
  e: KeyboardEvent,
  config: ParsedHotkeyConfig
): boolean {
  if (!matchesModifierFlags(e, config)) {
    return true;
  }

  if (config.key === null) {
    return false;
  }

  const key = config.key.toLowerCase();
  const eventKey = e.key.toLowerCase();

  if (key.startsWith("f") && key.length <= 3) {
    return eventKey === key;
  }

  if (key === "space") {
    return isSpaceKeyEvent(e);
  }

  return eventKey === key;
}

function normalizeStoredHotkey(hotkey?: string | null): string {
  if (hotkey) {
    const parsed = parseHotkeyString(hotkey);
    if (parsed) {
      return parsed.key === "Fn" ? "Fn" : hotkey;
    }
  }

  return "F9";
}

function getDownloadStatusText(progress: DownloadProgress | null): string {
  const status = progress?.status;
  if (status === "verifying") {
    return "Verifying model integrity...";
  }
  if (status === "downloading") {
    return `${formatBytes(progress?.bytesDownloaded ?? 0)} / ${formatBytes(progress?.totalBytes ?? 0)}`;
  }
  if (status === "complete") {
    return "Download complete!";
  }
  if (status === "failed") {
    return progress?.errorMessage || "Download failed";
  }
  return "";
}

function getDownloadStatusIcon(progress: DownloadProgress | null): string {
  const status = progress?.status;
  if (status === "downloading" || status === "verifying") {
    return `<span class="loading-spinner" aria-hidden="true">${createIcon(LoaderCircle)}</span>`;
  }
  if (status === "complete") {
    return `<span class="status-icon status-icon-complete" aria-hidden="true">${createIcon(Check)}</span>`;
  }
  return "";
}

function getDownloadStatusClasses(progress: DownloadProgress | null): string {
  const status = progress?.status;
  const classes = ["download-status"];

  if (status === "failed") {
    classes.push("error");
  }
  if (status === "complete") {
    classes.push("success", "centered-status");
  }
  if (status === "downloading" || status === "verifying") {
    classes.push("centered-status", "loading");
  }
  if (status === "verifying") {
    classes.push("verifying");
  }

  return classes.join(" ");
}

function renderDownloadBody(progress: DownloadProgress | null): string {
  const status = progress?.status;
  const isVisible =
    status === "downloading" ||
    status === "verifying" ||
    status === "complete" ||
    status === "failed";

  if (!isVisible) {
    return "";
  }

  const progressWidth =
    status === "verifying" ? 100 : (progress?.percentage ?? 0);
  const statusIcon = getDownloadStatusIcon(progress);
  const statusText = getDownloadStatusText(progress);
  const statusClasses = getDownloadStatusClasses(progress);

  return `
    <div class="download-progress-container">
      <div class="progress-bar">
        <div class="progress-bar-fill" style="width: ${progressWidth}%"></div>
      </div>
      <div class="${statusClasses}">${statusIcon}<span class="download-status-text">${statusText}</span></div>
    </div>
  `;
}

function renderMicPermissionStatus(status: string | undefined): string {
  if (status === "granted") {
    return `<div class="permission-status granted">Granted</div>`;
  }
  if (status === "prompt") {
    return `<button class="btn btn-secondary btn-sm" id="grant-microphone-btn">Allow</button>`;
  }
  return `<button class="btn btn-secondary btn-sm" id="grant-microphone-btn">Grant</button>`;
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
  return `<button class="btn btn-secondary btn-sm" id="grant-accessibility-btn">Grant</button>`;
}

function render(): void {
  if (!container) {
    return;
  }

  // Remove hotkey listener when leaving step 6
  if (state.step !== 6 && hotkeyKeyHandler) {
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
      renderHistoryStep();
      break;
    case 5:
      renderPermissions();
      break;
    case 6:
      renderHotkeyStep();
      break;
    case 7:
      renderMicSelection();
      break;
    case 8:
      renderTestStep();
      break;
    case 9:
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
          <li>${createIcon(Monitor)} All processing happens locally on your device</li>
          <li>${createIcon(Mic)} Microphone is only active while you hold the hotkey</li>
          <li>${createIcon(Shield)} Your audio never leaves your computer</li>
        </ul>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-accent btn-lg" id="get-started-btn">
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

function formatModelSize(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

function renderModelVariantCards(): string {
  const variants: { variant: ModelVariant; badge?: string }[] = [
    { variant: "small_q5" },
    { variant: "small", badge: "Recommended" },
    { variant: "large_turbo_q5" },
  ];

  return variants
    .map(({ variant, badge }) => {
      const model = state.models.find((m) => m.variant === variant);
      if (!model) {
        return "";
      }
      const isSelected = state.selectedModelVariant === variant;
      return `
        <button class="model-variant-card ${isSelected ? "selected" : ""}" data-variant="${variant}">
          ${badge ? `<div class="variant-badge">${badge}</div>` : ""}
          <div class="variant-name">${model.displayName}</div>
          <div class="variant-desc">${model.description}</div>
          <div class="variant-stats">
            <span>~${formatModelSize(model.sizeBytes)} disk</span>
            <span>~${model.memoryEstimateMb} MB memory</span>
          </div>
        </button>
      `;
    })
    .join("");
}

function renderDownloadModel(): void {
  if (!container) {
    return;
  }

  const progress = state.downloadProgress;
  const isDownloading = progress?.status === "downloading";
  const isVerifying = progress?.status === "verifying";
  const isComplete = progress?.status === "complete";
  const isFailed = progress?.status === "failed";
  const downloadBtnText = "Download Model";

  // Determine footer button state
  let footerButton: string;
  if (isComplete) {
    footerButton = `<button class="btn btn-accent btn-lg" id="continue-btn">Continue</button>`;
  } else if (isVerifying) {
    footerButton = `<button class="btn btn-accent btn-lg" disabled>Verifying...</button>`;
  } else if (isDownloading) {
    footerButton = `<button class="btn btn-accent btn-lg" disabled>Downloading...</button>`;
  } else if (isFailed) {
    footerButton = `<button class="btn btn-accent btn-lg" id="retry-btn">Retry Download</button>`;
  } else {
    footerButton = `<button class="btn btn-accent btn-lg" id="download-btn">${downloadBtnText}</button>`;
  }

  // Body content - either selection or progress
  let bodyContent: string;
  if (isDownloading || isVerifying || isComplete || isFailed) {
    bodyContent = renderDownloadBody(progress);
  } else {
    bodyContent = `
      <div class="model-variant-grid">
        ${renderModelVariantCards()}
      </div>
    `;
  }

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(Download)}
        </div>
        <h1 class="onboarding-title">Choose Speech Model</h1>
        <p class="onboarding-desc">Select a model based on your needs</p>
      </div>
      <div class="onboarding-body">
        ${state.downloadError ? `<div class="download-status error" style="margin-bottom: 16px;">${state.downloadError}</div>` : ""}
        ${bodyContent}
      </div>
      <div class="onboarding-footer">
        ${footerButton}
        ${renderStepIndicator(2)}
      </div>
    </div>
  `;

  // Attach variant card click handlers
  for (const card of document.querySelectorAll(".model-variant-card")) {
    card.addEventListener("click", (e) => {
      const variant = (e.currentTarget as HTMLElement).dataset
        .variant as ModelVariant;
      state.selectedModelVariant = variant;
      render();
    });
  }

  document
    .getElementById("download-btn")
    ?.addEventListener("click", handleStartDownload);
  document
    .getElementById("retry-btn")
    ?.addEventListener("click", handleStartDownload);
  document
    .getElementById("continue-btn")
    ?.addEventListener("click", handleDownloadContinue);
  attachStepIndicatorListeners();
}

async function handleDownloadContinue(): Promise<void> {
  // Save the selected model variant to settings before proceeding
  try {
    const currentSettings = await getSettings();
    await updateSettings({
      ...currentSettings,
      activeModelVariant: state.selectedModelVariant,
    });
  } catch (err) {
    console.error("Failed to save model variant selection:", err);
  }
  goToStep(3);
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
        <button class="btn btn-accent btn-lg" id="continue-btn">
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

function renderHistoryStep(): void {
  if (!container) {
    return;
  }

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(ClipboardList)}
        </div>
        <h1 class="onboarding-title">Transcript History</h1>
        <p class="onboarding-desc">Keep track of your transcriptions</p>
      </div>
      <div class="onboarding-body history-body">
        <ul class="onboarding-features">
          <li>${createIcon(BarChart3)} See daily usage stats on your dashboard</li>
          <li>${createIcon(Search)} Search and copy past transcriptions</li>
          <li>${createIcon(Shield)} Everything stays local on your device</li>
        </ul>
        <div class="history-toggle-onboarding">
          <div class="appearance-selector">
            <button class="appearance-option ${state.selectedHistoryMode === "off" ? "selected" : ""}" data-history-mode="off">Off</button>
            <button class="appearance-option ${state.selectedHistoryMode === "30d" ? "selected" : ""}" data-history-mode="30d">Last 30 days</button>
          </div>
        </div>
        <p class="onboarding-hint">You can change this later in settings</p>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-accent btn-lg" id="continue-btn">
          Continue
        </button>
        ${renderStepIndicator(4)}
      </div>
    </div>
  `;

  for (const btn of document.querySelectorAll("[data-history-mode]")) {
    btn.addEventListener("click", (e) => {
      state.selectedHistoryMode = (e.currentTarget as HTMLElement).dataset
        .historyMode as HistoryMode;
      render();
    });
  }

  document
    .getElementById("continue-btn")
    ?.addEventListener("click", handleHistoryStepContinue);
  attachStepIndicatorListeners();
}

async function handleHistoryStepContinue(): Promise<void> {
  try {
    const currentSettings = await getSettings();
    await updateSettings({
      ...currentSettings,
      historyMode: state.selectedHistoryMode,
    });
  } catch (err) {
    console.error("Failed to save history mode:", err);
  }

  goToStep(5);
}

function renderPermissions(): void {
  if (!container) {
    return;
  }

  const perms = state.permissions;
  const isMac = navigator.userAgent.includes("Mac");

  if (!isMac) {
    goToStep(6);
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
            ? `<button class="btn btn-accent btn-lg" id="continue-btn">Continue</button>`
            : `<button class="btn btn-accent btn-lg" id="restart-btn">Restart to Apply</button>`
        }
        ${renderStepIndicator(5)}
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
  goToStep(6);
}

async function handleRestartForPermissions(): Promise<void> {
  try {
    const currentSettings = await getSettings();
    await updateSettings({
      ...currentSettings,
      onboardingStep: 5,
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
        <button class="btn btn-accent btn-lg" id="continue-btn">
          Continue
        </button>
        ${renderStepIndicator(6)}
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

  goToStep(7);
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
        <button class="btn btn-accent btn-lg" id="continue-btn">
          Continue
        </button>
        ${renderStepIndicator(7)}
      </div>
    </div>
  `;

  document
    .getElementById("mic-select")
    ?.addEventListener("change", handleMicChange);
  document
    .getElementById("continue-btn")
    ?.addEventListener("click", () => goToStep(8));
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
        <p class="onboarding-hint ${hasText ? "invisible" : ""}">First transcription may take a few seconds while the model loads.</p>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-accent btn-lg" id="finish-btn" ${hasText ? "" : "disabled"}>
          Finish Setup
        </button>
        ${renderStepIndicator(8)}
      </div>
    </div>
  `;

  document
    .getElementById("finish-btn")
    ?.addEventListener("click", () => goToStep(9));
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
        <button class="btn btn-accent btn-lg" id="start-btn" type="button" ${
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

  if (step === 5) {
    await handleRequestPermissions();
  }

  if (step === 6) {
    state.capturedHotkey = null;
  }

  if (step === 7) {
    await loadAudioDevices();
  }

  if (step === 8) {
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

    if (!navigator.userAgent.includes("Mac")) {
      // WebView2 only: Windows needs a frontend fallback while the onboarding
      // window is focused. macOS uses the native listener directly.
      const hotkey = state.selectedHotkey;
      const parsedHotkey = parseHotkeyString(hotkey);
      if (!parsedHotkey) {
        return;
      }
      console.log(
        "[onboarding] Setting up frontend hotkey listener for:",
        hotkey
      );

      testHotkeyKeydown = (e: KeyboardEvent) => {
        if (testHotkeyPressed) {
          if (shouldReleaseOnKeydown(e, parsedHotkey)) {
            testHotkeyPressed = false;
            hotkeyRelease().catch(console.error);
          }
          return;
        }
        if (matchesHotkey(e, parsedHotkey)) {
          e.preventDefault();
          e.stopPropagation();
          testHotkeyPressed = true;
          hotkeyPress().catch(console.error);
        }
      };

      testHotkeyKeyup = (e: KeyboardEvent) => {
        if (!testHotkeyPressed) {
          return;
        }
        if (matchesHotkeyRelease(e, parsedHotkey)) {
          e.preventDefault();
          e.stopPropagation();
          testHotkeyPressed = false;
          hotkeyRelease().catch(console.error);
        }
      };

      document.addEventListener("keydown", testHotkeyKeydown);
      document.addEventListener("keyup", testHotkeyKeyup);
    }
  }

  render();
}

function handleStartDownload(): void {
  state.downloadError = null;
  state.downloadProgress = {
    variant: state.selectedModelVariant,
    bytesDownloaded: 0,
    totalBytes: 0,
    percentage: 0,
    status: "downloading",
  };
  render();

  downloadModel(state.selectedModelVariant).catch((err) => {
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

function hasDownloadProgressChanged(
  previous: DownloadProgress | null,
  next: DownloadProgress
): boolean {
  if (!previous) {
    return true;
  }

  return (
    previous.status !== next.status ||
    previous.percentage !== next.percentage ||
    previous.bytesDownloaded !== next.bytesDownloaded ||
    previous.totalBytes !== next.totalBytes ||
    previous.errorMessage !== next.errorMessage ||
    previous.variant !== next.variant
  );
}

function updateInlineDownloadProgress(progress: DownloadProgress): boolean {
  if (progress.status !== "downloading" || state.step !== 2 || !container) {
    return false;
  }

  const fill = container.querySelector(
    ".download-progress-container .progress-bar-fill"
  );
  const text = container.querySelector(
    ".download-progress-container .download-status-text"
  );

  if (!(fill instanceof HTMLElement && text instanceof HTMLElement)) {
    return false;
  }

  fill.style.width = `${progress.percentage}%`;
  text.textContent = getDownloadStatusText(progress);
  return true;
}

function startDownloadPolling(): void {
  console.log("[onboarding] Starting download polling");
  downloadPollInterval = window.setInterval(async () => {
    const progress = await getDownloadProgress();
    const changed = hasDownloadProgressChanged(
      state.downloadProgress,
      progress
    );

    if (!changed) {
      return;
    }

    console.log("[onboarding] Download progress:", progress);
    const previousStatus = state.downloadProgress?.status;
    state.downloadProgress = progress;

    if (
      previousStatus === "downloading" &&
      progress.status === "downloading" &&
      updateInlineDownloadProgress(progress)
    ) {
      return;
    }

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

  // Load saved settings and models first
  let savedSettings: Settings | null = null;
  let models: ModelInfo[] = [];
  try {
    [savedSettings, models] = await Promise.all([getSettings(), getModels()]);
  } catch (e) {
    console.error("Failed to load settings/models:", e);
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
    selectedHotkey: normalizeStoredHotkey(savedSettings?.hotkey),
    capturedHotkey: null,
    testText: "",
    selectedModelVariant: savedSettings?.activeModelVariant ?? "small",
    selectedHistoryMode: savedSettings?.historyMode ?? "30d",
    models,
  };

  // Check if resuming from a saved step (after restart)
  if (savedSettings?.onboardingStep) {
    const savedStep = savedSettings.onboardingStep as OnboardingStep;
    console.log("[onboarding] Resuming at step:", savedStep);

    // If resuming at permissions step, refresh permissions
    if (savedStep === 5) {
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

  const selectedModel = models.find(
    (model) => model.variant === state.selectedModelVariant
  );
  if (selectedModel?.isDownloaded) {
    state.downloadProgress = {
      variant: state.selectedModelVariant,
      bytesDownloaded: 0,
      totalBytes: 0,
      percentage: 100,
      status: "complete",
    };
    state.step = 3;
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
