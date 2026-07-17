import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Check,
  CheckCircle,
  Download,
  Keyboard,
  LoaderCircle,
  Mic,
  Monitor,
  PersonStanding,
  Shield,
  Sliders,
} from "lucide";
import {
  eventToHotkeyToken,
  matchesHotkey,
  matchesHotkeyRelease,
  normalizeStoredHotkey,
  parseHotkeyString,
} from "../lib/hotkey";
import { renderHotkeyChips } from "../lib/hotkey-display";
import { t } from "../lib/i18n";
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
  BootstrapReason,
  DownloadProgress,
  HistoryMode,
  ModelInfo,
  ModelVariant,
  PermissionStatus,
  Settings,
} from "../lib/types";
import { mountLanguageSelect } from "./language-select";

type OnboardingStep = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8;

interface OnboardingState {
  audioDevices: AudioDevice[];
  capturedHotkey: string | null;
  completeError: string | null;
  downloadError: string | null;
  downloadProgress: DownloadProgress | null;
  isCompleting: boolean;
  micError: string | null;
  models: ModelInfo[];
  permissions: PermissionStatus | null;
  selectedDeviceId: string | null;
  selectedHistoryMode: HistoryMode;
  selectedHotkey: string;
  selectedLanguages: string[];
  selectedModelVariant: ModelVariant;
  step: OnboardingStep;
  testText: string;
  modelRepairReason: BootstrapReason | null;
  invalidModelVariant: ModelVariant | null;
}

interface RenderOnboardingOptions {
  modelRepairReason?: BootstrapReason;
}

let state: OnboardingState = {
  step: 1,
  downloadProgress: null,
  downloadError: null,
  completeError: null,
  isCompleting: false,
  micError: null,
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
  modelRepairReason: null,
  invalidModelVariant: null,
};

let container: HTMLElement | null = null;
let downloadPollInterval: number | null = null;
let hotkeyKeyHandler: ((e: KeyboardEvent) => void) | null = null;
let hotkeyKeyupHandler: ((e: KeyboardEvent) => void) | null = null;
let hotkeyBlurHandler: (() => void) | null = null;
const hotkeyCaptureTokens = new Set<string>();
const hotkeyCaptureOrder: string[] = [];
let testResultUnlisten: UnlistenFn | null = null;
// Frontend hotkey handling for test step (WebView2 workaround)
let testHotkeyPressed = false;
let testHotkeyKeydown: ((e: KeyboardEvent) => void) | null = null;
let testHotkeyKeyup: ((e: KeyboardEvent) => void) | null = null;
let pendingEnterClass: string | null = null;

const TOTAL_STEPS = 8;

function cleanupHotkeyCaptureListeners(): void {
  if (hotkeyKeyHandler) {
    document.removeEventListener("keydown", hotkeyKeyHandler);
    hotkeyKeyHandler = null;
  }
  if (hotkeyKeyupHandler) {
    document.removeEventListener("keyup", hotkeyKeyupHandler);
    hotkeyKeyupHandler = null;
  }
  if (hotkeyBlurHandler) {
    window.removeEventListener("blur", hotkeyBlurHandler);
    hotkeyBlurHandler = null;
  }
  hotkeyCaptureTokens.clear();
  hotkeyCaptureOrder.length = 0;
}

function renderStepIndicator(currentStep: OnboardingStep): string {
  if (currentStep === 8) {
    return ""; // Don't show on completion
  }

  const dots: string[] = [];
  for (let i = 1; i <= TOTAL_STEPS - 1; i++) {
    const isActive = i === currentStep;
    const isPast = i < currentStep;
    const clickable = isPast;
    dots.push(`
      <button
        class="step-dot ${isActive ? "active" : ""} ${isPast ? "completed" : ""}"
        data-step="${i}"
        ${clickable ? "" : "disabled"}
        aria-label="${t("onboarding.step", { number: i })}"
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

async function persistSelectedDevice(deviceId: string | null): Promise<void> {
  const currentSettings = await getSettings();
  if (currentSettings.selectedMicrophoneId === deviceId) {
    return;
  }
  await updateSettings({
    ...currentSettings,
    selectedMicrophoneId: deviceId,
  });
}

function formatMb(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

function getDownloadLeftText(progress: DownloadProgress | null): string {
  const status = progress?.status;
  if (status === "verifying") {
    return t("onboarding.verifyingModel");
  }
  if (status === "downloading") {
    return `${Math.round(progress?.percentage ?? 0)}%`;
  }
  if (status === "complete") {
    return t("onboarding.downloadComplete");
  }
  if (status === "failed") {
    return progress?.errorMessage || t("onboarding.downloadFailed");
  }
  return "";
}

function getDownloadRightText(progress: DownloadProgress | null): string {
  const status = progress?.status;
  if (status === "downloading") {
    return formatMb(progress?.bytesDownloaded ?? 0);
  }
  if (status === "complete") {
    return formatMb(progress?.totalBytes ?? 0);
  }
  return "";
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
    status === "verifying" || status === "complete"
      ? 100
      : (progress?.percentage ?? 0);
  const leftText = getDownloadLeftText(progress);
  const rightText = getDownloadRightText(progress);
  const isComplete = status === "complete";
  const isFailed = status === "failed";

  const leftClass = isComplete
    ? "download-meta-left download-meta-ok"
    : isFailed
      ? "download-meta-left download-meta-error"
      : "download-meta-left";

  const leftIcon = isComplete
    ? `<span class="checkpill" aria-hidden="true">${createIcon(Check)}</span>`
    : status === "verifying"
      ? `<span class="loading-spinner" aria-hidden="true">${createIcon(LoaderCircle)}</span>`
      : "";

  return `
    <div class="download-progress-container">
      <div class="progress-bar">
        <div class="progress-bar-fill" style="width: ${progressWidth}%"></div>
      </div>
      <div class="download-meta">
        <span class="${leftClass}">${leftIcon}<span class="download-status-text">${leftText}</span></span>
        <span class="download-meta-right">${rightText}</span>
      </div>
    </div>
  `;
}

function renderMicPermissionStatus(status: string | undefined): string {
  if (status === "granted") {
    return `<div class="permission-status granted">${t("common.granted")}</div>`;
  }
  if (status === "prompt") {
    return `<button class="btn btn-info btn-sm" id="grant-microphone-btn">${t("common.allow")}</button>`;
  }
  return `<button class="btn btn-info btn-sm" id="grant-microphone-btn">${t("common.allow")}</button>`;
}

function renderAccessibilityPermissionStatus(
  status: string | undefined
): string {
  if (status === "granted") {
    return `<div class="permission-status granted">${t("common.granted")}</div>`;
  }
  if (status === "not-applicable") {
    return `<div class="permission-status">${t("common.na")}</div>`;
  }
  return `<button class="btn btn-info btn-sm" id="grant-accessibility-btn">${t("common.allow")}</button>`;
}

function render(): void {
  if (!container) {
    return;
  }

  // Remove hotkey listener when leaving the hotkey step
  if (state.step !== 5 && (hotkeyKeyHandler || hotkeyKeyupHandler)) {
    cleanupHotkeyCaptureListeners();
  }

  switch (state.step) {
    case 1:
      renderWelcome();
      break;
    case 2:
      renderDownloadModel();
      break;
    case 3:
      renderPreferences();
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

  if (pendingEnterClass && container) {
    const header = container.querySelector(".onboarding-header");
    const body = container.querySelector(".onboarding-body");
    addEnterClass(header, pendingEnterClass);
    addEnterClass(body, pendingEnterClass);
    // First load animates the footer too (action button + step dots).
    // Step-to-step transitions keep the footer static.
    if (pendingEnterClass === "onb-enter-up") {
      const footer = container.querySelector(".onboarding-footer");
      addEnterClass(footer, pendingEnterClass);
    }
    pendingEnterClass = null;
  }
}

/** Adds an enter-animation class and removes it once the animation ends.
 *  The animations fill `both`, so leaving the class on would keep a transform
 *  on the element, turning it into the containing block for the fixed-position
 *  language popover and breaking its viewport coordinates. */
function addEnterClass(el: Element | null, cls: string): void {
  if (!el) {
    return;
  }
  el.classList.add(cls);
  el.addEventListener("animationend", (e) => {
    if (e.target === el) {
      el.classList.remove(cls);
    }
  });
}

function renderWelcome(): void {
  if (!container) {
    return;
  }

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <img class="onboarding-logo" src="/icon.png" alt="Fing" />
        <h1 class="onboarding-title">${t("onboarding.welcomeTitle")}</h1>
        <p class="onboarding-desc">${t("onboarding.welcomeSubtitle")}</p>
      </div>
      <div class="onboarding-body">
        <ul class="onboarding-features">
          <li><span class="onb-ibox">${createIcon(Monitor)}</span>${t("onboarding.featureDevice")}</li>
          <li><span class="onb-ibox">${createIcon(Mic)}</span>${t("onboarding.featureMic")}</li>
          <li><span class="onb-ibox">${createIcon(Shield)}</span>${t("onboarding.featurePrivacy")}</li>
        </ul>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-accent btn-lg btn-block" id="get-started-btn">
          ${t("onboarding.getStarted")}
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
    { variant: "small", badge: t("onboarding.recommended") },
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
          <div class="variant-desc">${getModelQualityLabel(model.variant)}</div>
          <div class="variant-stats">
            <span>${t("onboarding.disk", { size: formatModelSize(model.sizeBytes) })}</span>
            <span>${t("onboarding.memory", { size: model.memoryEstimateMb })}</span>
          </div>
        </button>
      `;
    })
    .join("");
}

function getModelQualityLabel(variant: ModelVariant): string {
  switch (variant) {
    case "small_q5":
      return t("models.quality.small_q5");
    case "small":
      return t("models.quality.small");
    case "large_turbo_q5":
      return t("models.quality.large_turbo_q5");
  }
}

function renderDownloadFooterButton(
  isComplete: boolean,
  isVerifying: boolean,
  isDownloading: boolean,
  isFailed: boolean
): string {
  if (isComplete) {
    const label = state.modelRepairReason
      ? t("onboarding.finishRepair")
      : t("common.continue");
    return `<button class="btn btn-accent btn-lg btn-block" id="continue-btn">${label}</button>`;
  }
  if (isVerifying) {
    return `<button class="btn btn-accent btn-lg btn-block" disabled>${t("onboarding.verifying")}</button>`;
  }
  if (isDownloading) {
    return `<button class="btn btn-accent btn-lg btn-block" disabled>${t("onboarding.downloading")}</button>`;
  }
  if (isFailed) {
    return `<button class="btn btn-accent btn-lg btn-block" id="retry-btn">${t("onboarding.retryDownload")}</button>`;
  }
  return `<button class="btn btn-accent btn-lg btn-block" id="download-btn">${t("onboarding.downloadModel")}</button>`;
}

function getDownloadHeading(
  selectedModel: ModelInfo | undefined,
  isDownloading: boolean,
  isVerifying: boolean,
  isComplete: boolean,
  isFailed: boolean
): { title: string; subtitle: string } {
  if (isDownloading || isVerifying) {
    return {
      title: t("onboarding.downloadingTitle"),
      subtitle: t("onboarding.downloadingSubtitle"),
    };
  }
  if (isComplete) {
    return {
      title: t("onboarding.modelReady"),
      subtitle: t("onboarding.modelReadySubtitle", {
        model: selectedModel?.displayName ?? t("onboarding.modelFallback"),
      }),
    };
  }
  if (isFailed) {
    return {
      title: t("onboarding.downloadFailed"),
      subtitle: t("onboarding.downloadFailedSubtitle"),
    };
  }
  if (state.modelRepairReason === "model_missing") {
    return {
      title: t("onboarding.restoreTitle"),
      subtitle: t("onboarding.restoreSubtitle"),
    };
  }
  if (state.modelRepairReason === "model_invalid") {
    return {
      title: t("onboarding.repairTitle"),
      subtitle: t("onboarding.repairSubtitle"),
    };
  }
  return {
    title: t("onboarding.chooseModel"),
    subtitle: t("onboarding.chooseModelSubtitle"),
  };
}

function renderDownloadModel(): void {
  if (!container) {
    return;
  }

  const selectedModel = state.models.find(
    (model) => model.variant === state.selectedModelVariant
  );
  const progress =
    state.downloadProgress?.variant === state.selectedModelVariant
      ? state.downloadProgress
      : null;
  const isDownloading = progress?.status === "downloading";
  const isVerifying = progress?.status === "verifying";
  const isRepairingInvalidSelection =
    state.modelRepairReason !== null &&
    state.invalidModelVariant === state.selectedModelVariant;
  const isDownloaded =
    selectedModel?.isDownloaded === true && !isRepairingInvalidSelection;
  const isComplete = progress?.status === "complete" || isDownloaded;
  const isFailed = progress?.status === "failed";
  const footerButton = renderDownloadFooterButton(
    isComplete,
    isVerifying,
    isDownloading,
    isFailed
  );
  const heading = getDownloadHeading(
    selectedModel,
    isDownloading,
    isVerifying,
    isComplete,
    isFailed
  );

  // Body content - either selection or progress
  let bodyContent: string;
  if (
    isDownloading ||
    isVerifying ||
    progress?.status === "complete" ||
    isFailed
  ) {
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
        <h1 class="onboarding-title">${heading.title}</h1>
        <p class="onboarding-desc">${heading.subtitle}</p>
      </div>
      <div class="onboarding-body">
        ${state.downloadError ? `<div class="download-status error">${escapeHtml(state.downloadError)}</div>` : ""}
        ${bodyContent}
      </div>
      <div class="onboarding-footer">
        ${footerButton}
        ${state.modelRepairReason ? "" : renderStepIndicator(2)}
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
    state.downloadError = t("onboarding.saveModelFailed");
    render();
    return;
  }

  if (state.modelRepairReason) {
    await finishModelRepair();
    return;
  }

  goToStep(3);
}

async function finishModelRepair(): Promise<void> {
  if (state.isCompleting) {
    return;
  }

  state.isCompleting = true;
  state.downloadError = null;
  window.dispatchEvent(new CustomEvent("setup-completion-started"));

  try {
    await completeSetup();
    window.dispatchEvent(new CustomEvent("setup-complete"));
  } catch {
    window.dispatchEvent(new CustomEvent("setup-completion-failed"));
    state.isCompleting = false;
    state.invalidModelVariant = state.selectedModelVariant;
    state.downloadProgress = null;
    state.downloadError = t("onboarding.repairFailed");
    try {
      state.models = await getModels();
    } catch {
      // Keep the existing model list so the recovery screen remains usable.
    }
    render();
  }
}

function renderPreferences(): void {
  if (!container) {
    return;
  }

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(Sliders)}
        </div>
        <h1 class="onboarding-title">${t("onboarding.preferencesTitle")}</h1>
        <p class="onboarding-desc">${t("onboarding.preferencesSubtitle")}</p>
      </div>
      <div class="onboarding-body prefs-body">
        <div class="onb-section">
          <h3>${t("onboarding.languages")} <span class="meta">${t("onboarding.pickOne")}</span></h3>
          <div id="onb-langs"></div>
        </div>
        <div class="onb-section">
          <h3>${t("onboarding.history")} <span class="meta">${t("onboarding.storedLocally")}</span></h3>
          <div class="appearance-selector">
            <button class="appearance-option ${state.selectedHistoryMode === "off" ? "selected" : ""}" data-history-mode="off">${t("common.off")}</button>
            <button class="appearance-option ${state.selectedHistoryMode === "30d" ? "selected" : ""}" data-history-mode="30d">${t("settings.last30Days")}</button>
          </div>
        </div>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-accent btn-lg btn-block" id="continue-btn">
          ${t("common.continue")}
        </button>
        ${renderStepIndicator(3)}
      </div>
    </div>
  `;

  const langMount = document.getElementById("onb-langs");
  if (langMount) {
    mountLanguageSelect(langMount, {
      selected: state.selectedLanguages,
      onChange: (next) => {
        state.selectedLanguages = next;
      },
    });
  }

  for (const btn of document.querySelectorAll("[data-history-mode]")) {
    btn.addEventListener("click", (e) => {
      state.selectedHistoryMode = (e.currentTarget as HTMLElement).dataset
        .historyMode as HistoryMode;
      render();
    });
  }

  document
    .getElementById("continue-btn")
    ?.addEventListener("click", handlePreferencesContinue);
  attachStepIndicatorListeners();
}

async function handlePreferencesContinue(): Promise<void> {
  try {
    const currentSettings = await getSettings();
    await updateSettings({
      ...currentSettings,
      languages: state.selectedLanguages,
      historyMode: state.selectedHistoryMode,
    });
  } catch (err) {
    console.error("Failed to save preferences:", err);
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
        <h1 class="onboarding-title">${t("onboarding.permissionsTitle")}</h1>
        <p class="onboarding-desc">${t("onboarding.permissionsSubtitle")}</p>
      </div>
      <div class="onboarding-body">
        <div class="permissions-list">
          <div class="permission-row">
            <div class="permission-info">
              <span class="onb-ibox onb-ibox-lg">${createIcon(Mic)}</span>
              <div>
                <div class="permission-label">${t("onboarding.microphoneAccess")}</div>
                <div class="permission-desc">${t("onboarding.microphoneRequired")}</div>
              </div>
            </div>
            ${renderMicPermissionStatus(perms?.microphone)}
          </div>

          <div class="permission-row">
            <div class="permission-info">
              <span class="onb-ibox onb-ibox-lg">${createIcon(PersonStanding)}</span>
              <div>
                <div class="permission-label">${t("onboarding.accessibilityAccess")}</div>
                <div class="permission-desc">${t("onboarding.accessibilityRequired")}</div>
              </div>
            </div>
            ${renderAccessibilityPermissionStatus(perms?.accessibility)}
          </div>
        </div>
      </div>
      <div class="onboarding-footer">
        ${
          allGranted
            ? ""
            : `<p class="onboarding-foot-hint">${t("onboarding.grantRemaining")}</p>`
        }
        ${
          allGranted
            ? `<button class="btn btn-accent btn-lg btn-block" id="continue-btn">${t("common.continue")}</button>`
            : `<button class="btn btn-accent btn-lg btn-block" id="restart-btn">${t("onboarding.restartApply")}</button>`
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

function handlePermissionsContinue(): void {
  goToStep(5);
}

async function handleRestartForPermissions(): Promise<void> {
  await relaunchApp();
}

function renderHotkeyStep(): void {
  if (!container) {
    return;
  }

  const syncCapturedHotkey = () => {
    if (hotkeyCaptureOrder.length > 0) {
      state.capturedHotkey = hotkeyCaptureOrder.join("+");
    } else {
      state.capturedHotkey = null;
    }
    render();
  };

  const displayKey = state.capturedHotkey ?? state.selectedHotkey;
  const hasNewKey =
    state.capturedHotkey && state.capturedHotkey !== state.selectedHotkey;

  container.innerHTML = `
    <div class="onboarding">
      <div class="onboarding-header">
        <div class="onboarding-icon">
          ${createIcon(Keyboard)}
        </div>
        <h1 class="onboarding-title">${t("onboarding.hotkeyTitle")}</h1>
        <p class="onboarding-desc">${t("onboarding.hotkeySubtitle")}</p>
      </div>
      <div class="onboarding-body">
        <div class="hotkey-capture-area">
          <div class="hotkey-preview">
            ${renderHotkeyChips(displayKey, { chipClass: "hotkey-key", extraChipClass: hasNewKey ? "captured" : "" })}
          </div>
          <div class="hotkey-fn-inline">
            ${t("onboarding.orUse")}
            <button class="hotkey-modal-key hotkey-fn-chip" type="button" id="use-fn-btn" aria-label="${t("dialogs.useFn")}">fn</button>
            ${t("onboarding.onMacLaptops")}
          </div>
        </div>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-accent btn-lg btn-block" id="continue-btn">
          ${t("common.continue")}
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
        hotkeyCaptureTokens.clear();
        hotkeyCaptureOrder.length = 0;
        render();
        return;
      }
      if (e.repeat) {
        return;
      }

      const token = eventToHotkeyToken(e);
      if (!token) {
        return;
      }

      // Starting a fresh press session — clear the previously captured chord
      // so this new keypress begins a new binding attempt.
      if (hotkeyCaptureTokens.size === 0) {
        hotkeyCaptureOrder.length = 0;
      }
      hotkeyCaptureTokens.add(token);
      if (!hotkeyCaptureOrder.includes(token)) {
        hotkeyCaptureOrder.push(token);
      }
      syncCapturedHotkey();
    };
    document.addEventListener("keydown", hotkeyKeyHandler);
  }
  if (!hotkeyKeyupHandler) {
    hotkeyKeyupHandler = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      const token = eventToHotkeyToken(e);
      if (!token) {
        return;
      }

      // Release order doesn't matter; the captured chord stays put until the
      // next fresh press session (when no keys are held).
      hotkeyCaptureTokens.delete(token);
    };
    document.addEventListener("keyup", hotkeyKeyupHandler);
  }
  if (!hotkeyBlurHandler) {
    hotkeyBlurHandler = () => {
      hotkeyCaptureTokens.clear();
    };
    window.addEventListener("blur", hotkeyBlurHandler);
  }

  document.getElementById("use-fn-btn")?.addEventListener("click", () => {
    state.capturedHotkey = "Function";
    hotkeyCaptureTokens.clear();
    hotkeyCaptureOrder.length = 0;
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

  cleanupHotkeyCaptureListeners();

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
        <h1 class="onboarding-title">${t("onboarding.microphoneTitle")}</h1>
        <p class="onboarding-desc">${t("onboarding.microphoneSubtitle")}</p>
      </div>
      <div class="onboarding-body">
        ${
          state.micError
            ? `<div class="download-status error">${escapeHtml(state.micError)}</div>`
            : ""
        }
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
        <p class="onboarding-foot-hint">${t("onboarding.microphoneHint")}</p>
        <button class="btn btn-accent btn-lg btn-block" id="continue-btn">
          ${t("common.continue")}
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
    ?.addEventListener("click", handleMicContinue);
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
      <div class="onboarding-header onboarding-header-try">
        <div class="onboarding-icon">
          ${createIcon(Mic)}
        </div>
        <h1 class="onboarding-title">${t("onboarding.tryTitle")}</h1>
        <p class="onboarding-desc">${t("onboarding.trySubtitle", {
          hotkey: renderHotkeyChips(hotkey, {
            chipClass: "hotkey-key-inline",
          }),
        })}</p>
      </div>
      <div class="onboarding-body">
        <div
          id="test-input"
          class="test-input test-input-readonly ${hasText ? "has-text" : ""}"
        >${state.testText ? `<span class="test-input-content">${escapeHtml(state.testText)}</span>` : `<span class="test-input-placeholder">${t("onboarding.transcriptionPlaceholder")}</span>`}</div>
      </div>
      <div class="onboarding-footer">
        <p class="onboarding-foot-hint ${hasText ? "invisible" : ""}">${t("onboarding.firstTranscription")}</p>
        <button class="btn btn-accent btn-lg btn-block" id="finish-btn" ${hasText ? "" : "disabled"}>
          ${t("onboarding.finishSetup")}
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
        <h1 class="onboarding-title">${t("onboarding.completeTitle")}</h1>
        <p class="onboarding-desc">${t("onboarding.completeSubtitle")}</p>
      </div>
      <div class="onboarding-body">
        ${
          completeError
            ? `<div class="download-status error">${escapeHtml(completeError)}</div>`
            : ""
        }
        <div class="completion-instructions">
          <div class="done-row">
            <span class="onb-ibox">${createIcon(Keyboard)}</span>
            <span>${t("onboarding.holdToRecord", {
              hotkey: renderHotkeyChips(hotkey, {
                chipClass: "hotkey-key-inline",
              }),
            })}</span>
          </div>
          <div class="done-row">
            <span class="onb-ibox">${createIcon(Check)}</span>
            <span>${t("onboarding.releaseToPaste")}</span>
          </div>
          <div class="done-row">
            <span class="onb-ibox">${createIcon(Sliders)}</span>
            <span>${t("onboarding.tweakLater")}</span>
          </div>
        </div>
      </div>
      <div class="onboarding-footer">
        <button class="btn btn-accent btn-lg btn-block" id="start-btn" type="button" ${
          isCompleting ? "disabled" : ""
        }>
          ${isCompleting ? t("onboarding.finishing") : t("onboarding.start")}
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
  const prevStep = state.step;
  if (container && step !== prevStep) {
    const direction = step > prevStep ? "forward" : "back";
    const exitClass =
      direction === "forward" ? "onb-exit-left" : "onb-exit-right";
    const header = container.querySelector(".onboarding-header");
    const body = container.querySelector(".onboarding-body");
    header?.classList.add(exitClass);
    body?.classList.add(exitClass);
    await new Promise((resolve) => setTimeout(resolve, 180));
    pendingEnterClass =
      direction === "forward" ? "onb-enter-right" : "onb-enter-left";
  }

  await stopPolling();
  state.step = step;

  if (step === 4) {
    await handleRequestPermissions();
  }

  if (step === 5) {
    state.capturedHotkey = null;
  }

  if (step === 6) {
    state.micError = null;
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
      const pressedTestTokens = new Set<string>();

      testHotkeyKeydown = (e: KeyboardEvent) => {
        if (e.repeat) {
          return;
        }
        const token = eventToHotkeyToken(e);
        if (!token) {
          return;
        }
        pressedTestTokens.add(token);
        if (testHotkeyPressed) {
          return;
        }
        if (matchesHotkey(pressedTestTokens, parsedHotkey)) {
          e.preventDefault();
          e.stopPropagation();
          testHotkeyPressed = true;
          hotkeyPress().catch(console.error);
        }
      };

      testHotkeyKeyup = (e: KeyboardEvent) => {
        const token = eventToHotkeyToken(e);
        if (!testHotkeyPressed) {
          if (token) {
            pressedTestTokens.delete(token);
          }
          return;
        }
        if (matchesHotkeyRelease(e, parsedHotkey)) {
          e.preventDefault();
          e.stopPropagation();
          testHotkeyPressed = false;
          hotkeyRelease().catch(console.error);
        }
        if (token) {
          pressedTestTokens.delete(token);
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

function handleMicChange(e: Event): void {
  const select = e.target as HTMLSelectElement;
  state.micError = null;
  state.selectedDeviceId = select.value || null;
}

async function handleMicContinue(): Promise<void> {
  state.micError = null;

  try {
    await persistSelectedDevice(state.selectedDeviceId);
  } catch (err) {
    console.error("Failed to save microphone selection:", err);
    state.micError = t("onboarding.saveMicrophoneFailed");
    render();
    return;
  }

  goToStep(7);
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
  const right = container.querySelector(
    ".download-progress-container .download-meta-right"
  );

  if (!(fill instanceof HTMLElement && text instanceof HTMLElement)) {
    return false;
  }

  fill.style.width = `${progress.percentage}%`;
  text.textContent = getDownloadLeftText(progress);
  if (right instanceof HTMLElement) {
    right.textContent = getDownloadRightText(progress);
  }
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

function waitForAnimationFrame(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => resolve());
  });
}

async function waitForPaintFrames(count = 2): Promise<void> {
  for (let i = 0; i < count; i += 1) {
    await waitForAnimationFrame();
  }
}

async function showOnboardingCompletionError(): Promise<void> {
  document.documentElement.classList.add("window-route-preparing");
  render();

  try {
    const window = getCurrentWindow();
    await window.show();
    await window.setFocus();
    await waitForPaintFrames();
  } catch (err) {
    console.error("Failed to show onboarding completion error:", err);
  } finally {
    document.documentElement.classList.remove("window-route-preparing");
  }
}

async function handleComplete(): Promise<void> {
  if (state.isCompleting) {
    return;
  }

  state.isCompleting = true;
  state.completeError = null;

  stopPolling().catch(() => undefined);
  window.dispatchEvent(new CustomEvent("setup-completion-started"));

  try {
    await getCurrentWindow().hide();
  } catch (err) {
    console.error("Failed to hide onboarding window:", err);
  }

  try {
    await completeSetup();
    window.dispatchEvent(new CustomEvent("setup-complete"));
  } catch {
    window.dispatchEvent(new CustomEvent("setup-completion-failed"));
    state.isCompleting = false;
    state.completeError = t("onboarding.setupFailed");
    await showOnboardingCompletionError();
  }
}

export async function renderOnboarding(
  el: HTMLElement,
  options: RenderOnboardingOptions = {}
): Promise<void> {
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
    step: options.modelRepairReason ? 2 : 1,
    downloadProgress: null,
    downloadError: null,
    completeError: null,
    isCompleting: false,
    micError: null,
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
    modelRepairReason: options.modelRepairReason ?? null,
    invalidModelVariant: options.modelRepairReason
      ? (savedSettings?.activeModelVariant ?? "small")
      : null,
  };

  pendingEnterClass = "onb-enter-up";
  render();
}

export function cleanupOnboarding(): void {
  stopPolling();
  cleanupHotkeyCaptureListeners();
  container = null;
}
