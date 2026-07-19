import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ArrowUpRight,
  BookOpen,
  Check,
  CheckCircle,
  Copy,
  History,
  Home,
  type IconNode,
  Info,
  LoaderCircle,
  Mic,
  Monitor,
  Moon,
  Plus,
  Power,
  RefreshCw,
  Search,
  Settings,
  Sun,
  Trash2,
  X,
} from "lucide";
import { mountLanguageSelect } from "./components/language-select";
import { cleanupOnboarding, renderOnboarding } from "./components/onboarding";
import type { ParsedHotkeyConfig } from "./lib/hotkey";
import {
  eventToHotkeyToken,
  matchesHotkey,
  matchesHotkeyRelease,
  normalizeStoredHotkey,
  parseHotkeyString,
} from "./lib/hotkey";
import { renderHotkeyChips } from "./lib/hotkey-display";
import { formatDateTime, formatNumber, setUiLanguage, t, tp } from "./lib/i18n";
import { createIcon, escapeHtml } from "./lib/icons";
import {
  armPermissionRestart,
  checkForUpdatesNow,
  clearUpdateStatus,
  deleteAllTranscripts,
  deleteModel,
  deleteTranscript,
  downloadModel,
  finishMainWindowPresentation,
  getAppInfo,
  getAutoStart,
  getBootstrapStatus,
  getDownloadProgress,
  getInferenceRuntimeInfo,
  getMicTestLevel,
  getModels,
  getRecentTranscripts,
  getSettings,
  getStats,
  getUpdateStatus,
  hotkeyPress,
  hotkeyRelease,
  presentMainWindow,
  refreshAudioDevices,
  relaunchApp,
  requestAccessibilityPermission,
  requestMicrophonePermission,
  requestPermissions,
  searchTranscripts,
  setActiveModel,
  setAutoStart,
  setHotkeySuppressed,
  startMicTest,
  stopMicTest,
  updateSettings,
} from "./lib/ipc";
import type {
  AppInfo,
  AppState,
  AudioDevice,
  BootstrapReason,
  HistoryMode,
  InferenceDevicePreference,
  InferenceRuntimeInfo,
  MicrophoneTest,
  MicTestStartResult,
  ModelInfo,
  ModelVariant,
  PermissionStatus,
  Settings as SettingsType,
  SidebarItem,
  Stats,
  Theme,
  Transcript,
  UiLanguage,
  UpdateCheckResult,
  UpdateStatus,
  WindowPresentationRequest,
} from "./lib/types";

const scrollFadeObservers = new WeakMap<HTMLElement, ResizeObserver>();

function updateScrollFade(el: HTMLElement): void {
  const { scrollTop, scrollHeight, clientHeight } = el;
  const canScrollUp = scrollTop > 1;
  const canScrollDown = scrollTop + clientHeight < scrollHeight - 1;
  el.classList.toggle("fade-top", canScrollUp);
  el.classList.toggle("fade-bottom", canScrollDown);
}

function setupScrollFade(el: HTMLElement | null): void {
  if (!el) {
    return;
  }
  let ro = scrollFadeObservers.get(el);
  if (!ro) {
    el.classList.add("scroll-fade");
    el.addEventListener("scroll", () => updateScrollFade(el), {
      passive: true,
    });
    ro = new ResizeObserver(() => updateScrollFade(el));
    scrollFadeObservers.set(el, ro);
  }
  ro.disconnect();
  ro.observe(el);
  for (const child of Array.from(el.children)) {
    ro.observe(child);
  }
  updateScrollFade(el);
}

function applyTheme(theme: Theme): void {
  if (theme === "system") {
    document.documentElement.removeAttribute("data-theme");
    localStorage.removeItem("fing-theme");
  } else {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("fing-theme", theme);
  }
}

let currentView: SidebarItem = "home";
let appInfo: AppInfo | null = null;
let stats: Stats | null = null;
let currentAppState: AppState = "initializing";
let transcripts: Transcript[] = [];
let searchQuery = "";
let transcriptOffset = 0;
let hasMoreTranscripts = true;
const PAGE_SIZE = 25;
let settings: SettingsType | null = null;
let settingsLoadedAt = 0;
let audioDevices: AudioDevice[] = [];
let hotkeyModalCleanup: (() => void) | null = null;
let micTestModalCleanup: (() => void) | null = null;
let searchTimeout: ReturnType<typeof setTimeout> | null = null;
let sidebarListenerAttached = false;
let contentListenerAttached = false;
let models: ModelInfo[] = [];
let inferenceRuntimeInfo: InferenceRuntimeInfo | null = null;
let inferenceRuntimeLoading = false;
let inferenceRuntimeVariant: ModelVariant | null = null;
let inferenceRuntimeRequest = 0;
let modelDownloadPollInterval: number | null = null;
const MAX_DICTIONARY_TERMS = 100;
const MAX_DICTIONARY_WORDS_PER_TERM = 3;
let dictionaryError: string | null = null;
let lazyModelToggleBusy = false;
let updateCheckInProgress = false;
let updateStatus: UpdateStatus = {
  updateAvailable: false,
  checking: false,
};
type SettingsPermission = "microphone" | "accessibility";
const permissionRestartRequired = new Set<SettingsPermission>();
let lastPermissionStatus: PermissionStatus | null = null;
let onboardingCompletionInFlight = false;
let latestPresentationRequestId = 0;

interface ModelDownloadProgressState {
  percentage: number;
  status: "downloading" | "verifying";
  variant: ModelVariant;
}

let modelDownloadProgress: ModelDownloadProgressState | null = null;

function toModelDownloadProgress(
  variant: ModelVariant,
  progress: {
    status: string;
    percentage: number;
  }
): ModelDownloadProgressState | null {
  if (progress.status !== "downloading" && progress.status !== "verifying") {
    return null;
  }

  return {
    variant,
    percentage: progress.percentage,
    status: progress.status,
  };
}

function hasModelDownloadProgressChanged(
  previous: ModelDownloadProgressState | null,
  next: ModelDownloadProgressState
): boolean {
  if (!previous) {
    return true;
  }

  return (
    previous.variant !== next.variant ||
    previous.status !== next.status ||
    previous.percentage !== next.percentage
  );
}

function updateInlineModelDownloadProgress(
  progress: ModelDownloadProgressState
): boolean {
  if (progress.status !== "downloading" || currentView !== "settings") {
    return false;
  }

  const row = document.querySelector(
    `.model-row[data-variant="${progress.variant}"]`
  );
  if (!(row instanceof HTMLElement)) {
    return false;
  }

  const value = row.querySelector(".model-download-progress-value");
  if (!(value instanceof HTMLElement)) {
    return false;
  }

  value.textContent = `${Math.round(progress.percentage)}%`;
  return true;
}

function navigateToTab(tab: SidebarItem): void {
  if (!["home", "history", "dictionary", "settings", "about"].includes(tab)) {
    return;
  }

  // Clear content immediately to prevent flash of old view
  const content = document.getElementById("content");
  if (content) {
    content.innerHTML = "";
  }

  currentView = tab;
  renderSidebar();
  renderContent();
}

function waitForAnimationFrame(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => resolve());
  });
}

async function waitForPaintFrames(count = 1): Promise<void> {
  for (let i = 0; i < count; i += 1) {
    await waitForAnimationFrame();
  }
}

async function handleMainWindowPresentationRequest(
  request: WindowPresentationRequest
): Promise<void> {
  latestPresentationRequestId = request.requestId;

  const window = getCurrentWindow();
  const isVisible = await window.isVisible().catch(() => true);
  if (isVisible) {
    document.documentElement.classList.remove("window-route-preparing");
    navigateToTab(request.tab);
    try {
      await finishMainWindowPresentation(request.requestId);
    } catch (err) {
      console.error("Failed to finish main window presentation:", err);
    }
    return;
  }

  document.documentElement.classList.add("window-route-preparing");
  navigateToTab(request.tab);
  await waitForPaintFrames();

  try {
    await finishMainWindowPresentation(request.requestId);
    if (latestPresentationRequestId === request.requestId) {
      await waitForPaintFrames();
      document.documentElement.classList.remove("window-route-preparing");
    }
  } catch (err) {
    console.error("Failed to finish main window presentation:", err);
  }
}

function setupSidebarListener(): void {
  const sidebar = document.getElementById("sidebar");
  if (!sidebar || sidebarListenerAttached) {
    return;
  }
  sidebarListenerAttached = true;

  sidebar.addEventListener("click", (e) => {
    const target = (e.target as HTMLElement).closest(".sidebar-item");
    if (!target) {
      return;
    }
    const view = target.getAttribute("data-view") as SidebarItem | null;
    const action = target.getAttribute("data-action");
    if (view) {
      navigateToTab(view);
    } else if (action === "quit") {
      invoke("quit_app");
    }
  });
}

function renderSidebar(): void {
  const sidebar = document.getElementById("sidebar");
  if (!sidebar) {
    return;
  }

  const items: { id: SidebarItem; label: string; icon: IconNode }[] = [
    { id: "home", label: t("sidebar.home"), icon: Home },
    { id: "history", label: t("sidebar.history"), icon: History },
    { id: "dictionary", label: t("sidebar.dictionary"), icon: BookOpen },
    { id: "settings", label: t("sidebar.settings"), icon: Settings },
    { id: "about", label: t("sidebar.about"), icon: Info },
  ];

  sidebar.innerHTML = `
    ${items
      .map(
        (item) => `
      <button class="sidebar-item ${currentView === item.id ? "active" : ""}" data-view="${item.id}">
        ${createIcon(item.icon)}
        <span>${item.label}</span>
      </button>
    `
      )
      .join("")}
    <div class="sidebar-spacer"></div>
    <button class="sidebar-item" data-action="quit">
      ${createIcon(Power)}
      <span>${t("sidebar.quit")}</span>
    </button>
  `;

  setupSidebarListener();
}

function renderContent(): void {
  const content = document.getElementById("content");
  if (!content) {
    return;
  }

  switch (currentView) {
    case "home":
      renderHome(content);
      break;
    case "history":
      transcriptOffset = 0;
      renderHistory(content);
      break;
    case "dictionary":
      renderDictionary(content);
      break;
    case "settings":
      renderSettings(content);
      break;
    case "about":
      renderAbout(content);
      break;
    default:
      break;
  }

  applyContentScrollFade(content);
}

function applyContentScrollFade(content: HTMLElement): void {
  const scroller = content.querySelector<HTMLElement>(
    ".history-scrollable, .dictionary-scrollable"
  );
  setupScrollFade(scroller ?? content);
}

function renderHome(el: HTMLElement): void {
  if (settings?.historyMode === "off") {
    const mockWords = [
      { word: "meeting", pct: 100 },
      { word: "project", pct: 72 },
      { word: "update", pct: 55 },
      { word: "review", pct: 38 },
      { word: "schedule", pct: 24 },
    ];
    const mockWordHtml = mockWords
      .map(
        (w, i) => `
      <div class="word-item">
        <span class="word-rank">${i + 1}</span>
        <span class="word-text">${w.word}</span>
        <div class="word-bar-container"><div class="word-bar" style="width: ${w.pct}%"></div></div>
        <span class="word-count">${Math.round(w.pct * 0.4)}</span>
      </div>`
      )
      .join("");

    el.innerHTML = `
      <h1>${t("dashboard.title")}</h1>
      <div class="dashboard-disabled-wrapper">
        <div class="dashboard-disabled-ghost" aria-hidden="true">
          <div class="stats-grid">
            <div class="stat-card">
              <div class="stat-label">${t("dashboard.transcriptionsToday")}</div>
              <div class="stat-value">12</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">${t("dashboard.wordsToday")}</div>
              <div class="stat-value">847</div>
            </div>
          </div>
          <div class="stat-card" style="margin-bottom: 24px;">
            <div class="stat-label">${t("dashboard.mostUsedWords")}</div>
            <div class="word-list">${mockWordHtml}</div>
          </div>
          <div class="stats-grid">
            <div class="stat-card">
              <div class="stat-label">${t("dashboard.averageWords")}</div>
              <div class="stat-value">71</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">${t("dashboard.averageSpeed")}</div>
              <div class="stat-value">142 <span class="stat-unit">${t("dashboard.wpm")}</span></div>
            </div>
          </div>
        </div>
        <div class="dashboard-disabled-overlay">
          <div class="dashboard-disabled-card">
            <div class="dashboard-disabled-title">${t("dashboard.historyDisabled")}</div>
            <p>${t("dashboard.enableForStats")}</p>
          </div>
        </div>
      </div>
    `;
    return;
  }

  const topWords = stats?.topWords ?? [];
  const maxCount = topWords.length > 0 ? topWords[0].count : 1;

  const wordListHtml =
    topWords.length > 0
      ? topWords
          .map(
            (w, i) => `
      <div class="word-item">
        <span class="word-rank">${i + 1}</span>
        <span class="word-text">${escapeHtml(w.word)}</span>
        <div class="word-bar-container"><div class="word-bar" style="width: ${(w.count / maxCount) * 100}%"></div></div>
        <span class="word-count">${w.count}</span>
      </div>`
          )
          .join("")
      : '<div class="stat-value">0</div>';

  el.innerHTML = `
    <h1>${t("dashboard.title")}</h1>
    <div class="stats-grid">
      <div class="stat-card">
        <div class="stat-label">${t("dashboard.transcriptionsToday")}</div>
        <div class="stat-value">${formatNumber(stats?.transcriptionsToday ?? 0)}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">${t("dashboard.wordsToday")}</div>
        <div class="stat-value">${formatNumber(stats?.wordsToday ?? 0)}</div>
      </div>
    </div>
    <div class="stat-card" style="margin-bottom: 24px;">
      <div class="stat-label">${t("dashboard.mostUsedWords")}</div>
      <div class="word-list">${wordListHtml}</div>
    </div>
    <div class="stats-grid">
      <div class="stat-card">
        <div class="stat-label">${t("dashboard.averageWords")}</div>
        <div class="stat-value">${formatNumber(Math.round(stats?.averageWordsPerTranscription ?? 0))}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">${t("dashboard.averageSpeed")}</div>
        <div class="stat-value">${formatNumber(Math.round(stats?.averageWpm ?? 0))} <span class="stat-unit">${t("dashboard.wpm")}</span></div>
      </div>
    </div>
  `;
}

function parseCreatedAt(createdAt: string): Date {
  // SQLite datetime format: "YYYY-MM-DD HH:MM:SS"
  return new Date(`${createdAt.replace(" ", "T")}Z`);
}

function formatTime(createdAt: string): string {
  const date = parseCreatedAt(createdAt);
  return formatDateTime(date, { hour: "2-digit", minute: "2-digit" });
}

type DateGroup = "today" | "yesterday" | "thisWeek" | "older";

function getDateGroup(createdAt: string): DateGroup {
  const now = new Date();
  const date = parseCreatedAt(createdAt);
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterday = new Date(today.getTime() - 86_400_000);
  const weekAgo = new Date(today.getTime() - 7 * 86_400_000);

  if (date >= today) {
    return "today";
  }
  if (date >= yesterday) {
    return "yesterday";
  }
  if (date >= weekAgo) {
    return "thisWeek";
  }
  return "older";
}

function groupTranscriptsByDate(
  items: Transcript[]
): Map<DateGroup, Transcript[]> {
  const groups = new Map<DateGroup, Transcript[]>();
  const order: DateGroup[] = ["today", "yesterday", "thisWeek", "older"];

  for (const grp of order) {
    groups.set(grp, []);
  }

  for (const item of items) {
    const grp = getDateGroup(item.createdAt);
    groups.get(grp)?.push(item);
  }

  for (const [key, value] of groups) {
    if (value.length === 0) {
      groups.delete(key);
    }
  }

  return groups;
}

function getDateGroupLabel(group: DateGroup): string {
  switch (group) {
    case "today":
      return t("history.today");
    case "yesterday":
      return t("history.yesterday");
    case "thisWeek":
      return t("history.thisWeek");
    case "older":
      return t("history.older");
  }
}

function truncateText(text: string, maxLength: number): string {
  if (text.length <= maxLength) {
    return text;
  }
  return `${text.slice(0, maxLength)}...`;
}

async function loadTranscripts(append = false): Promise<void> {
  try {
    let newItems: Transcript[];
    if (searchQuery.trim()) {
      newItems = await searchTranscripts(
        searchQuery,
        PAGE_SIZE,
        transcriptOffset
      );
    } else {
      newItems = await getRecentTranscripts(PAGE_SIZE, transcriptOffset);
    }
    hasMoreTranscripts = newItems.length === PAGE_SIZE;
    transcripts = append ? [...transcripts, ...newItems] : newItems;
  } catch {
    transcripts = append ? transcripts : [];
    hasMoreTranscripts = false;
  }
}

async function handleDeleteTranscript(id: number): Promise<void> {
  try {
    await deleteTranscript(id);
    transcriptOffset = 0;
    await loadTranscripts();
    stats = await getStats().catch(() => null);
    renderContent();
  } catch (e) {
    console.error("Failed to delete transcript:", e);
  }
}

async function handleClearAll(): Promise<void> {
  const confirmed = await showConfirmDialog({
    title: t("history.clearTitle"),
    body: t("history.clearBody"),
    confirmText: t("history.deleteAll"),
    danger: true,
  });
  if (!confirmed) {
    return;
  }

  try {
    await deleteAllTranscripts();
    transcriptOffset = 0;
    await loadTranscripts();
    stats = await getStats().catch(() => null);
    renderContent();
  } catch (e) {
    console.error("Failed to clear all transcripts:", e);
  }
}

function copyToClipboard(text: string): void {
  navigator.clipboard.writeText(text).catch(() => {
    // Silently ignore clipboard errors
  });
}

function handleHistoryClick(e: MouseEvent, el: HTMLElement): void {
  const target = e.target as HTMLElement;

  // Handle load more button
  if (target.closest(".load-more-btn")) {
    const scrollable = el.querySelector(".history-scrollable");
    const scrollTop = scrollable?.scrollTop ?? 0;
    transcriptOffset += PAGE_SIZE;
    loadTranscripts(true).then(() => {
      renderHistory(el, { skipLoad: true });
      const newScrollable = el.querySelector(".history-scrollable");
      if (newScrollable) {
        newScrollable.scrollTop = scrollTop;
      }
    });
    return;
  }

  // Handle clear all button
  if (target.closest(".clear-all-btn")) {
    handleClearAll();
    return;
  }

  // Handle delete button
  const deleteBtn = target.closest(".delete-btn");
  if (deleteBtn) {
    e.stopPropagation();
    const listItem = deleteBtn.closest(".list-item");
    const id = listItem?.getAttribute("data-id");
    if (id) {
      handleDeleteTranscript(Number(id));
    }
    return;
  }

  // Handle copy button
  const copyBtn = target.closest(".copy-btn") as HTMLButtonElement | null;
  if (copyBtn) {
    e.stopPropagation();
    const listItem = copyBtn.closest(".list-item");
    const id = listItem?.getAttribute("data-id");
    const transcript = transcripts.find((t) => t.id === Number(id));
    if (transcript) {
      copyToClipboard(transcript.text);
      const originalIcon = copyBtn.innerHTML;
      copyBtn.innerHTML = createIcon(Check);
      setTimeout(() => {
        copyBtn.innerHTML = originalIcon;
      }, 2000);
    }
  }
}

function handleHistoryInput(e: Event, el: HTMLElement): void {
  const target = e.target as HTMLElement;
  if (!target.classList.contains("search-input")) {
    return;
  }
  const input = target as HTMLInputElement;
  searchQuery = input.value;
  transcriptOffset = 0;
  if (searchTimeout) {
    clearTimeout(searchTimeout);
  }
  searchTimeout = setTimeout(() => {
    renderHistory(el, { restoreFocus: true });
  }, 300);
}

function renderHistory(
  el: HTMLElement,
  options: { restoreFocus?: boolean; skipLoad?: boolean } = {}
): void {
  if (settings?.historyMode === "off") {
    const mockTranscripts = [
      {
        time: "2:34 PM",
        words: 42,
        text: "I think we should focus on the user experience for the next sprint and prioritize the onboarding flow improvements",
      },
      {
        time: "1:15 PM",
        words: 28,
        text: "Can you send me the latest design files for the dashboard? I want to review them before our meeting",
      },
    ];

    const mockListHtml = `
      <div class="date-group-header">${t("history.today")}</div>
      ${mockTranscripts
        .map(
          (m) => `
        <div class="list-item">
          <div class="list-item-header">
            <span class="list-item-time">${m.time}</span>
            <span class="list-item-words">${tp("common.wordOne", "common.wordOther", m.words)}</span>
          </div>
          <div class="list-item-text">${m.text}</div>
          <div class="list-item-actions">
            <button class="copy-btn">${createIcon(Copy)}</button>
            <button class="delete-btn">${createIcon(Trash2)}</button>
          </div>
        </div>`
        )
        .join("")}
    `;

    el.innerHTML = `
      <div class="history-sticky-header">
        <div class="history-header"><h1>${t("history.title")}</h1></div>
      </div>
      <div class="history-scrollable">
        <div class="dashboard-disabled-wrapper">
          <div class="dashboard-disabled-ghost" aria-hidden="true">
            <div class="search-wrapper" style="margin-bottom: 16px;">
              ${createIcon(Search)}
              <input type="text" class="search-input" placeholder="${t("history.search")}" disabled>
            </div>
            <div class="transcript-list">${mockListHtml}</div>
          </div>
          <div class="dashboard-disabled-overlay">
            <div class="dashboard-disabled-card">
              <div class="dashboard-disabled-title">${t("dashboard.historyDisabled")}</div>
              <p>${t("history.enableToSave")}</p>
            </div>
          </div>
        </div>
      </div>
    `;
    return;
  }

  const { restoreFocus = false, skipLoad = false } = options;
  // biome-ignore lint/complexity/noExcessiveCognitiveComplexity: UI rendering with necessary loops
  const doRender = () => {
    const grouped = groupTranscriptsByDate(transcripts);
    const hasTranscripts = transcripts.length > 0;

    let listHtml = "";
    if (hasTranscripts) {
      for (const [group, items] of grouped) {
        listHtml += `<div class="date-group-header">${getDateGroupLabel(group)}</div>`;
        for (const item of items) {
          listHtml += `
            <div class="list-item" data-id="${item.id}">
              <div class="list-item-header">
                <span class="list-item-time">${formatTime(item.createdAt)}</span>
                <span class="list-item-words">${tp("common.wordOne", "common.wordOther", item.wordCount)}</span>
              </div>
              <div class="list-item-text">${escapeHtml(truncateText(item.text, 150))}</div>
              <div class="list-item-actions">
                <button class="copy-btn" title="${t("history.copy")}">${createIcon(Copy)}</button>
                <button class="delete-btn" title="${t("common.delete")}">${createIcon(Trash2)}</button>
              </div>
            </div>
          `;
        }
      }
    }

    el.innerHTML = `
      <div class="history-sticky-header">
        <div class="history-header">
          <h1>${t("history.title")}</h1>
          ${hasTranscripts ? `<button class="btn btn-danger btn-sm clear-all-btn">${t("history.clearAll")}</button>` : ""}
        </div>
        <div class="search-wrapper">
          ${createIcon(Search)}
          <input type="text" class="search-input" placeholder="${t("history.search")}" value="${escapeHtml(searchQuery)}">
        </div>
      </div>
      <div class="history-scrollable">
        ${
          hasTranscripts
            ? `<div class="transcript-list">${listHtml}</div>
               ${hasMoreTranscripts ? `<button class="btn btn-outline load-more-btn">${t("history.loadMore")}</button>` : ""}`
            : `
          <div class="empty-state">
            ${searchQuery ? "" : `<div class="empty-state-icon">${createIcon(Mic)}</div>`}
            <div class="empty-state-title">${searchQuery ? t("history.noResults") : t("history.noTranscripts")}</div>
            ${searchQuery ? `<p>${t("history.differentSearch")}</p>` : ""}
          </div>
        `
        }
      </div>
    `;

    const searchInput = el.querySelector(".search-input") as HTMLInputElement;
    if (restoreFocus && searchInput) {
      searchInput.focus();
      searchInput.setSelectionRange(searchQuery.length, searchQuery.length);
    }

    setupScrollFade(el.querySelector<HTMLElement>(".history-scrollable"));
  };

  if (skipLoad) {
    doRender();
  } else {
    loadTranscripts().then(doRender);
  }
}

function normalizeDictionaryTerm(term: string): string {
  return term.trim().replace(/\s+/g, " ");
}

function dictionaryWordCount(term: string): number {
  if (!term) {
    return 0;
  }
  return term.split(" ").filter(Boolean).length;
}

function getDictionaryTerms(): string[] {
  return settings?.dictionaryTerms ?? [];
}

function validateDictionaryTerm(term: string): string | null {
  if (!term) {
    return t("dictionary.enterTerm");
  }
  if (dictionaryWordCount(term) > MAX_DICTIONARY_WORDS_PER_TERM) {
    return t("dictionary.tooManyWords", {
      count: MAX_DICTIONARY_WORDS_PER_TERM,
    });
  }

  const existing = getDictionaryTerms();
  if (existing.length >= MAX_DICTIONARY_TERMS) {
    return t("dictionary.full", { count: MAX_DICTIONARY_TERMS });
  }
  if (existing.some((value) => value.toLowerCase() === term.toLowerCase())) {
    return t("dictionary.duplicate");
  }

  return null;
}

function focusDictionaryInput(): void {
  requestAnimationFrame(() => {
    if (currentView !== "dictionary") {
      return;
    }

    const dictionaryInput = document.querySelector(
      ".dictionary-input"
    ) as HTMLInputElement | null;
    if (dictionaryInput && !dictionaryInput.disabled) {
      dictionaryInput.focus();
    }
  });
}

async function addDictionaryTerm(input: HTMLInputElement): Promise<void> {
  if (!settings) {
    return;
  }

  const normalized = normalizeDictionaryTerm(input.value);
  const validationError = validateDictionaryTerm(normalized);
  if (validationError) {
    dictionaryError = validationError;
    renderContent();
    focusDictionaryInput();
    return;
  }

  const updatedTerms = [...getDictionaryTerms(), normalized];
  try {
    const updatedSettings = { ...settings, dictionaryTerms: updatedTerms };
    settings = await updateSettings(updatedSettings);
    dictionaryError = null;
    renderContent();
    focusDictionaryInput();
  } catch (error) {
    console.error("Failed to add dictionary term:", error);
    dictionaryError = t("dictionary.saveFailed");
    renderContent();
    focusDictionaryInput();
  }
}

async function removeDictionaryTerm(index: number): Promise<void> {
  if (!(settings && Number.isInteger(index))) {
    return;
  }

  const terms = getDictionaryTerms();
  if (index < 0 || index >= terms.length) {
    return;
  }

  const updatedTerms = terms.filter((_, termIndex) => termIndex !== index);
  try {
    const updatedSettings = { ...settings, dictionaryTerms: updatedTerms };
    settings = await updateSettings(updatedSettings);
    dictionaryError = null;
    renderContent();
  } catch (error) {
    console.error("Failed to remove dictionary term:", error);
    dictionaryError = t("dictionary.updateFailed");
    renderContent();
  }
}

function handleDictionaryClick(e: MouseEvent): void {
  const target = e.target as HTMLElement;

  const removeButton = target.closest(
    ".dictionary-remove-btn"
  ) as HTMLButtonElement | null;
  if (removeButton?.dataset.index) {
    removeDictionaryTerm(Number(removeButton.dataset.index));
    return;
  }

  if (target.closest(".dictionary-add-btn")) {
    const input = document.querySelector(
      ".dictionary-input"
    ) as HTMLInputElement | null;
    if (input) {
      addDictionaryTerm(input);
    }
  }
}

function handleDictionaryKeydown(e: KeyboardEvent): void {
  const target = e.target as HTMLElement;
  if (!(target instanceof HTMLInputElement)) {
    return;
  }
  if (!target.classList.contains("dictionary-input")) {
    return;
  }
  if (e.key !== "Enter") {
    return;
  }

  e.preventDefault();
  addDictionaryTerm(target);
}

const SETTINGS_CACHE_TTL = 5000; // 5 seconds

async function loadSettings(
  options: {
    force?: boolean;
    loadDevices?: boolean;
    refreshDevices?: boolean;
  } = {}
): Promise<void> {
  const { force = false, loadDevices = true, refreshDevices = false } = options;

  // Skip if cache is fresh (within TTL)
  if (
    !force &&
    settings &&
    Date.now() - settingsLoadedAt < SETTINGS_CACHE_TTL
  ) {
    return;
  }

  try {
    let shouldRefreshDevices =
      loadDevices && (refreshDevices || audioDevices.length === 0);
    if (shouldRefreshDevices) {
      shouldRefreshDevices = await canRefreshAudioDevices();
    }

    const loadedModelsPromise = getModels().catch((err) => {
      console.error("Failed to load models:", err);
      return models;
    });
    const [loadedSettings, devices] = await Promise.all([
      getSettings(),
      shouldRefreshDevices
        ? refreshAudioDevices()
        : Promise.resolve(audioDevices),
    ]);
    settings = {
      ...loadedSettings,
      lazyModelLoading: loadedSettings.lazyModelLoading ?? false,
      dictionaryTerms: loadedSettings.dictionaryTerms ?? [],
      uiLanguage: loadedSettings.uiLanguage ?? "en",
    };
    setUiLanguage(settings.uiLanguage);
    audioDevices = devices;

    const autoStart = await getAutoStart().catch(() => null);
    if (autoStart !== null && settings) {
      settings = { ...settings, autoStart };
    }
    models = await loadedModelsPromise;
    settingsLoadedAt = Date.now();
  } catch {
    settings = null;
    audioDevices = [];
    models = [];
  }
}

async function canRefreshAudioDevices(): Promise<boolean> {
  if (document.body.dataset.platform !== "darwin") {
    return true;
  }

  const permissions = await requestPermissions();
  trackPermissionRestartRequirement(permissions);
  return permissions.microphone === "granted";
}

async function setHotkey(hotkey: string): Promise<boolean> {
  if (!settings) {
    return false;
  }

  try {
    const updated = { ...settings, hotkey };
    settings = await updateSettings(updated);

    // Re-register the hotkey in backend
    await invoke("update_hotkey", { hotkey });

    // Update frontend hotkey listener (Windows WebView2 workaround)
    hotkeyConfig = parseHotkeyString(hotkey);
    console.log("[hotkey] Frontend listener updated to:", hotkey);

    return true;
  } catch (err) {
    console.error("Failed to update hotkey:", err);
    return false;
  }
}

function showHotkeyModal(): void {
  // Remove existing modal if any
  closeHotkeyModal();

  let capturedHotkey: string | null = null;
  // Currently held keys; once this is empty, the next keypress starts a fresh chord.
  const pressedTokens = new Set<string>();
  // The chord being built. Releases do NOT remove from this — only a fresh
  // press session (after full release) clears and restarts it.
  const capturedTokens: string[] = [];
  const currentHotkey = normalizeStoredHotkey(settings?.hotkey);

  setHotkeySuppressed(true).catch(console.error);

  const modal = document.createElement("div");
  modal.className = "dialog-overlay";

  const syncCapturedHotkey = () => {
    capturedHotkey =
      capturedTokens.length > 0 ? capturedTokens.join("+") : null;
    renderModal();
  };

  const renderModal = () => {
    modal.innerHTML = `
      <div class="dialog hotkey-dialog">
        <button class="dialog-close">${createIcon(X)}</button>
        <div class="dialog-header">
          <div class="dialog-title">${t("dialogs.hotkeyTitle")}</div>
          <div class="hotkey-dialog-desc">${t("dialogs.hotkeyPrompt")}</div>
        </div>
        <div class="dialog-body hotkey-dialog-body">
          <div class="hotkey-modal-preview">
            ${renderHotkeyChips(capturedHotkey ?? currentHotkey, { chipClass: "hotkey-modal-key", extraChipClass: capturedHotkey ? "captured" : "" })}
          </div>
        </div>
        <div class="dialog-footer hotkey-dialog-footer">
          <div class="hotkey-fn-pick">
            <button class="hotkey-modal-key hotkey-fn-chip" type="button" aria-label="${t("dialogs.useFn")}">fn</button>
          </div>
          <button class="btn btn-accent hotkey-confirm-btn" ${capturedHotkey ? "" : "disabled"}>${t("settings.setHotkey")}</button>
        </div>
      </div>
    `;

    // Re-attach event listeners after re-render
    modal
      .querySelector(".dialog-close")
      ?.addEventListener("click", closeHotkeyModal);

    modal.querySelector(".hotkey-fn-chip")?.addEventListener("click", () => {
      capturedHotkey = "Function";
      pressedTokens.clear();
      capturedTokens.length = 0;
      capturedTokens.push("Function");
      renderModal();
    });

    modal
      .querySelector(".hotkey-confirm-btn")
      ?.addEventListener("click", async () => {
        if (!capturedHotkey) {
          return;
        }
        const btn = modal.querySelector(
          ".hotkey-confirm-btn"
        ) as HTMLButtonElement;
        btn.disabled = true;
        btn.textContent = t("dialogs.setting");

        if (await setHotkey(capturedHotkey)) {
          closeHotkeyModal();
          renderContent();
        } else {
          const desc = modal.querySelector(".hotkey-dialog-desc");
          if (desc) {
            desc.textContent = t("dialogs.hotkeyFailed");
            desc.classList.add("error");
          }
          btn.disabled = false;
          btn.textContent = t("settings.setHotkey");
        }
      });
  };

  renderModal();
  document.body.appendChild(modal);

  const keydownHandler = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();

    if (e.key === "Escape") {
      closeHotkeyModal();
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
    // so the new keypress begins a new binding attempt.
    if (pressedTokens.size === 0) {
      capturedTokens.length = 0;
    }
    pressedTokens.add(token);
    if (!capturedTokens.includes(token)) {
      capturedTokens.push(token);
    }
    syncCapturedHotkey();
  };

  const keyupHandler = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();

    const token = eventToHotkeyToken(e);
    if (!token) {
      return;
    }

    // Release order doesn't matter; capturedTokens stays put until the next
    // fresh press session (when pressedTokens is empty on keydown).
    pressedTokens.delete(token);
  };

  const blurHandler = () => {
    pressedTokens.clear();
  };

  const clickHandler = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("dialog-overlay")) {
      closeHotkeyModal();
    }
  };

  document.addEventListener("keydown", keydownHandler);
  document.addEventListener("keyup", keyupHandler);
  window.addEventListener("blur", blurHandler);
  modal.addEventListener("click", clickHandler);

  hotkeyModalCleanup = () => {
    document.removeEventListener("keydown", keydownHandler);
    document.removeEventListener("keyup", keyupHandler);
    window.removeEventListener("blur", blurHandler);
    modal.removeEventListener("click", clickHandler);
    modal.remove();
    hotkeyModalCleanup = null;
    setHotkeySuppressed(false).catch(console.error);
  };
}

function closeHotkeyModal(): void {
  if (hotkeyModalCleanup) {
    hotkeyModalCleanup();
  }
}

async function showMicTestModal(): Promise<void> {
  await closeMicTestModal();

  let micTestInterval: number | null = null;
  let currentMicTest: MicrophoneTest | null = null;
  let audioDetected = false;
  let selectedDeviceId = settings?.selectedMicrophoneId ?? null;
  let deviceMatchResult: MicTestStartResult | null = null;
  let localDevices = [...audioDevices];

  const modal = document.createElement("div");
  modal.className = "dialog-overlay";

  const getMicOptions = () =>
    localDevices
      .map(
        (d) =>
          `<option value="${escapeHtml(d.id)}" ${selectedDeviceId === d.id || (selectedDeviceId === null && d.isDefault) ? "selected" : ""}>${escapeHtml(d.name)}</option>`
      )
      .join("");

  const renderModalContent = () => {
    const level = currentMicTest?.peakLevel ?? 0;
    const levelPercent = Math.min(Math.sqrt(level) * 150, 100);
    const showMismatchWarning =
      deviceMatchResult && !deviceMatchResult.deviceMatched;

    modal.innerHTML = `
      <div class="dialog mic-test-dialog">
        <button class="dialog-close">${createIcon(X)}</button>
        <div class="dialog-header">
          <div class="dialog-title">${t("dialogs.micTitle")}</div>
        </div>
        <div class="dialog-body">
          <div class="mic-test-container">
            <div class="mic-select-row">
              <label for="modal-mic-select">${t("dialogs.device")}</label>
              <div class="mic-select-wrapper">
                <select id="modal-mic-select" class="settings-select">
                  ${getMicOptions()}
                </select>
                <button class="btn btn-icon mic-refresh-btn" title="${t("settings.refreshDevices")}">${createIcon(RefreshCw)}</button>
              </div>
            </div>

            ${
              showMismatchWarning
                ? `<div class="mic-mismatch-warning">
                ${t("dialogs.deviceFallback")}
              </div>`
                : ""
            }

            <div class="audio-level-container">
              <div class="audio-level-label">${t("dialogs.audioLevel")}</div>
              <div class="audio-level-bar">
                <div class="audio-level-fill ${levelPercent > 10 ? "active" : ""}" style="width: ${levelPercent}%"></div>
              </div>
            </div>

            <div class="mic-test-prompt ${audioDetected ? "success" : ""}">
              ${
                audioDetected
                  ? `${createIcon(CheckCircle)} ${t("dialogs.audioDetected")}`
                  : `${createIcon(Mic)} ${t("dialogs.saySomething")}`
              }
            </div>
          </div>
        </div>
        <div class="dialog-footer mic-test-footer">
          <button class="btn btn-accent mic-test-done-btn">${t("common.done")}</button>
        </div>
      </div>
    `;

    const closeBtn = modal.querySelector(".dialog-close");
    closeBtn?.addEventListener("click", closeMicTestModal);

    const doneBtn = modal.querySelector(".mic-test-done-btn");
    doneBtn?.addEventListener("click", closeMicTestModal);

    const refreshBtn = modal.querySelector(".mic-refresh-btn");
    refreshBtn?.addEventListener("click", async () => {
      const btn = refreshBtn as HTMLButtonElement;
      btn.disabled = true;
      btn.classList.add("spinning");
      const minSpinTime = new Promise((r) => setTimeout(r, 1000));
      try {
        if (!(await canRefreshAudioDevices())) {
          return;
        }

        const [devices] = await Promise.all([
          refreshAudioDevices(),
          minSpinTime,
        ]);
        localDevices = devices;
        audioDevices = localDevices;
        renderModalContent();
      } catch (err) {
        console.error("Failed to refresh devices:", err);
      } finally {
        btn.disabled = false;
        btn.classList.remove("spinning");
      }
    });

    const micSelect = modal.querySelector(
      "#modal-mic-select"
    ) as HTMLSelectElement;
    micSelect?.addEventListener("change", async () => {
      selectedDeviceId = micSelect.value || null;
      audioDetected = false;
      try {
        await stopMicTest();
        deviceMatchResult = await startMicTest(selectedDeviceId);
        renderModalContent();
      } catch (err) {
        console.error("Failed to switch mic:", err);
      }
    });
  };

  const updateAudioLevel = () => {
    const level = currentMicTest?.peakLevel ?? 0;
    const levelPercent = Math.min(Math.sqrt(level) * 150, 100);

    const levelFill = modal.querySelector(".audio-level-fill") as HTMLElement;
    if (levelFill) {
      levelFill.style.width = `${levelPercent}%`;
      if (levelPercent > 10) {
        levelFill.classList.add("active");
      } else {
        levelFill.classList.remove("active");
      }
    }

    const prompt = modal.querySelector(".mic-test-prompt") as HTMLElement;
    if (prompt && audioDetected && !prompt.classList.contains("success")) {
      prompt.classList.add("success");
      prompt.innerHTML = `${createIcon(CheckCircle)} ${t("dialogs.audioDetected")}`;
    }
  };

  renderModalContent();
  document.body.appendChild(modal);

  // Start mic test
  try {
    deviceMatchResult = await startMicTest(selectedDeviceId);
    renderModalContent();
  } catch (err) {
    console.error("Failed to start mic test:", err);
  }

  // Poll for audio levels
  micTestInterval = window.setInterval(async () => {
    try {
      currentMicTest = await getMicTestLevel();
      if (currentMicTest.isReceivingAudio && currentMicTest.peakLevel > 0.1) {
        audioDetected = true;
      }
      updateAudioLevel();
    } catch (err) {
      console.error("Mic test error:", err);
    }
  }, 150);

  const clickHandler = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("dialog-overlay")) {
      closeMicTestModal();
    }
  };

  const keyHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      closeMicTestModal();
    }
  };

  modal.addEventListener("click", clickHandler);
  document.addEventListener("keydown", keyHandler);

  micTestModalCleanup = async () => {
    if (micTestInterval) {
      clearInterval(micTestInterval);
      micTestInterval = null;
    }
    try {
      await stopMicTest();
    } catch (err) {
      console.error("Error stopping mic test:", err);
    }
    modal.removeEventListener("click", clickHandler);
    document.removeEventListener("keydown", keyHandler);
    modal.remove();
    micTestModalCleanup = null;
  };
}

async function closeMicTestModal(): Promise<void> {
  if (micTestModalCleanup) {
    await micTestModalCleanup();
  }
}

function startModelDownloadPolling(variant: ModelVariant): void {
  if (modelDownloadPollInterval) {
    clearInterval(modelDownloadPollInterval);
  }

  modelDownloadProgress = { variant, percentage: 0, status: "downloading" };

  // Immediately update UI to show 0%
  const modelList = document.querySelector(".model-list");
  if (modelList && currentView === "settings") {
    modelList.innerHTML = renderModelList();
  }

  modelDownloadPollInterval = window.setInterval(async () => {
    const progress = await getDownloadProgress();

    if (progress.status === "complete" || progress.status === "failed") {
      if (modelDownloadPollInterval) {
        clearInterval(modelDownloadPollInterval);
        modelDownloadPollInterval = null;
      }
      modelDownloadProgress = null;
      // Refresh models and re-render
      await loadSettings({ force: true, refreshDevices: false });
      if (currentView === "settings") {
        renderContent();
      }
      return;
    }

    const nextProgress = toModelDownloadProgress(variant, progress);
    if (!nextProgress) {
      return;
    }

    if (!hasModelDownloadProgressChanged(modelDownloadProgress, nextProgress)) {
      return;
    }

    const previousProgress = modelDownloadProgress;
    modelDownloadProgress = nextProgress;

    if (
      previousProgress?.status === "downloading" &&
      nextProgress.status === "downloading" &&
      updateInlineModelDownloadProgress(nextProgress)
    ) {
      return;
    }

    const modelList = document.querySelector(".model-list");
    if (modelList && currentView === "settings") {
      modelList.innerHTML = renderModelList();
    }
  }, 500);
}

function showRestartDialog(
  previousVariant?: ModelVariant,
  body = t("dialogs.restartBody")
): void {
  const modal = document.createElement("div");
  modal.className = "dialog-overlay";
  modal.innerHTML = `
    <div class="dialog">
      <div class="dialog-header">
        <div class="dialog-title">${t("dialogs.restartTitle")}</div>
      </div>
      <div class="dialog-body">${body}</div>
      <div class="dialog-footer">
        <button class="btn btn-ghost" id="restart-later-btn">${t("common.later")}</button>
        <button class="btn btn-accent" id="restart-now-btn">${t("dialogs.restartNow")}</button>
      </div>
    </div>
  `;

  document.body.appendChild(modal);

  document
    .getElementById("restart-later-btn")
    ?.addEventListener("click", async () => {
      modal.remove();
      if (previousVariant) {
        await setActiveModel(previousVariant);
      }
      loadSettings({ force: true, refreshDevices: false }).then(() =>
        renderContent()
      );
    });

  document.getElementById("restart-now-btn")?.addEventListener("click", () => {
    relaunchApp();
  });
}

interface ConfirmDialogOptions {
  body: string;
  cancelText?: string;
  confirmText?: string;
  danger?: boolean;
  title: string;
}

function showConfirmDialog(options: ConfirmDialogOptions): Promise<boolean> {
  const {
    title,
    body,
    confirmText = t("common.confirm"),
    cancelText = t("common.cancel"),
    danger = false,
  } = options;
  return new Promise((resolve) => {
    const modal = document.createElement("div");
    modal.className = "dialog-overlay";
    modal.innerHTML = `
      <div class="dialog">
        <button class="dialog-close" id="dialog-close-btn" aria-label="${t("common.close")}">${createIcon(X)}</button>
        <div class="dialog-header">
          <div class="dialog-title">${title}</div>
        </div>
        <div class="dialog-body">${body}</div>
        <div class="dialog-footer">
          <button class="btn btn-ghost" id="dialog-cancel-btn">${cancelText}</button>
          <button class="btn ${danger ? "btn-danger" : "btn-accent"}" id="dialog-confirm-btn">${confirmText}</button>
        </div>
      </div>
    `;

    document.body.appendChild(modal);

    const dismiss = () => {
      modal.remove();
      resolve(false);
    };

    document
      .getElementById("dialog-close-btn")
      ?.addEventListener("click", dismiss);

    document
      .getElementById("dialog-cancel-btn")
      ?.addEventListener("click", dismiss);

    document
      .getElementById("dialog-confirm-btn")
      ?.addEventListener("click", () => {
        modal.remove();
        resolve(true);
      });
  });
}

function formatErrorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message;
  }

  if (typeof error === "string" && error.trim().length > 0) {
    return error;
  }

  return t("errors.unknown");
}

function getUpdateNotesPreview(
  notes: string | null | undefined
): string | null {
  if (!(notes && notes.trim().length > 0)) {
    return null;
  }

  const trimmed = notes.trim();
  if (trimmed.length <= 320) {
    return trimmed;
  }

  return `${trimmed.slice(0, 320)}...`;
}

function getUpdateButtonLabel(): string {
  return updateStatus.updateAvailable
    ? t("updates.available")
    : t("updates.check");
}

function setUpdateButtonBusy(isBusy: boolean): void {
  const button = document.querySelector(
    ".check-updates-btn"
  ) as HTMLButtonElement | null;
  if (!button) {
    return;
  }

  button.disabled = isBusy;
  button.textContent = isBusy ? t("common.checking") : getUpdateButtonLabel();
}

async function promptForAvailableUpdate(
  status: UpdateCheckResult
): Promise<boolean> {
  const { ask } = await import("@tauri-apps/plugin-dialog");
  const notesPreview = getUpdateNotesPreview(status.availableBody);
  const version = status.availableVersion ?? t("updates.newerVersion");
  const prompt = notesPreview
    ? t("updates.versionAvailableWithNotes", {
        version,
        notes: notesPreview,
      })
    : t("updates.versionAvailable", { version });

  return await ask(prompt, {
    title: t("updates.available"),
    kind: "info",
    okLabel: t("updates.install"),
    cancelLabel: t("common.later"),
  });
}

async function installAvailableUpdate(): Promise<void> {
  const [{ message }, { check }] = await Promise.all([
    import("@tauri-apps/plugin-dialog"),
    import("@tauri-apps/plugin-updater"),
  ]);
  const update = await check();

  if (!update) {
    updateStatus = await clearUpdateStatus();
    if (currentView === "settings") {
      renderContent();
    }
    await message(t("updates.upToDate"), {
      title: t("updates.noneTitle"),
      kind: "info",
    });
    return;
  }

  await update.downloadAndInstall();
  updateStatus = await clearUpdateStatus();
  await message(t("updates.installed"), {
    title: t("updates.installedTitle"),
    kind: "info",
  });
  await relaunchApp();
}

async function runManualUpdateCheck(): Promise<void> {
  if (updateCheckInProgress) {
    return;
  }

  updateCheckInProgress = true;
  setUpdateButtonBusy(true);

  try {
    const { message } = await import("@tauri-apps/plugin-dialog");
    const latestUpdate = await checkForUpdatesNow();

    updateStatus = {
      ...updateStatus,
      updateAvailable: latestUpdate.updateAvailable,
      checking: false,
    };

    if (!latestUpdate.updateAvailable) {
      await message(t("updates.upToDate"), {
        title: t("updates.noneTitle"),
        kind: "info",
      });
      return;
    }

    const shouldInstall = await promptForAvailableUpdate(latestUpdate);
    if (!shouldInstall) {
      return;
    }

    await installAvailableUpdate();
  } catch (error) {
    console.error("Failed to check for updates:", error);
    const { message } = await import("@tauri-apps/plugin-dialog");
    await message(t("updates.failed", { error: formatErrorMessage(error) }), {
      title: t("updates.failedTitle"),
      kind: "error",
    });
  } finally {
    updateCheckInProgress = false;
    setUpdateButtonBusy(false);
  }
}

async function handleSettingChange(
  key: keyof SettingsType,
  value: unknown
): Promise<void> {
  if (!settings) {
    return;
  }
  try {
    const updated = { ...settings, [key]: value };
    settings = await updateSettings(updated);
  } catch (e) {
    console.error("Failed to update settings:", e);
  }
}

async function handleAutoStartToggle(
  toggle: HTMLElement,
  enabled: boolean
): Promise<void> {
  if (!settings) {
    return;
  }

  const previous = settings.autoStart;
  toggle.classList.toggle("active", enabled);

  try {
    await setAutoStart(enabled);
    const updated = { ...settings, autoStart: enabled };
    settings = await updateSettings(updated);
  } catch (e) {
    toggle.classList.toggle("active", previous);
    throw e;
  }
}

async function handleLazyModelLoadingToggle(
  toggle: HTMLElement,
  enabled: boolean
): Promise<void> {
  if (!settings || lazyModelToggleBusy) {
    return;
  }

  const previous = settings.lazyModelLoading;
  lazyModelToggleBusy = true;
  toggle.classList.add("disabled");
  toggle.classList.toggle("active", enabled);

  try {
    const updated = { ...settings, lazyModelLoading: enabled };
    settings = await updateSettings(updated);
  } catch (e) {
    console.error("Failed to update lazy model loading:", e);
    toggle.classList.toggle("active", previous);
  } finally {
    lazyModelToggleBusy = false;
    const currentToggle = document.querySelector(
      '.toggle[data-setting="lazyModelLoading"]'
    ) as HTMLElement | null;
    if (currentToggle) {
      currentToggle.classList.remove("disabled");
      currentToggle.classList.toggle(
        "active",
        settings?.lazyModelLoading ?? previous
      );
    }
  }
}

async function handleHistoryModeOff(): Promise<void> {
  const confirmed = await showConfirmDialog({
    title: t("settings.turnOffHistoryTitle"),
    body: t("settings.turnOffHistoryBody"),
    confirmText: t("common.confirm"),
    danger: true,
  });
  if (!confirmed) {
    return;
  }

  await deleteAllTranscripts();
  await handleSettingChange("historyMode", "off");
  stats = await getStats().catch(() => null);
  renderContent();
}

async function changeUiLanguage(language: UiLanguage): Promise<void> {
  if (!(settings && settings.uiLanguage !== language)) {
    return;
  }

  const previous = settings.uiLanguage;
  setUiLanguage(language);
  settings = { ...settings, uiLanguage: language };
  renderSidebar();
  renderContent();

  try {
    settings = await updateSettings(settings);
    settingsLoadedAt = Date.now();
  } catch (error) {
    console.error("Failed to update UI language:", error);
    setUiLanguage(previous);
    settings = { ...settings, uiLanguage: previous };
    renderSidebar();
    renderContent();
  }
}

// biome-ignore lint/complexity/noExcessiveCognitiveComplexity: UI event handler with necessary branches
function handleSettingsClick(e: MouseEvent): void {
  const target = e.target as HTMLElement;

  // Handle theme selection
  const themeOption = target.closest(
    ".appearance-option"
  ) as HTMLElement | null;
  if (themeOption) {
    const uiLanguage = themeOption.dataset.uiLanguage as UiLanguage | undefined;
    if (uiLanguage) {
      void changeUiLanguage(uiLanguage);
      return;
    }

    // History mode segmented control
    const historyMode = themeOption.dataset.historyMode as
      | HistoryMode
      | undefined;
    if (historyMode && settings) {
      if (historyMode === "off" && settings.historyMode !== "off") {
        handleHistoryModeOff();
        return;
      }
      const selector = themeOption.closest(".appearance-selector");
      if (selector) {
        for (const opt of selector.querySelectorAll(".appearance-option")) {
          opt.classList.toggle(
            "selected",
            (opt as HTMLElement).dataset.historyMode === historyMode
          );
        }
      }
      handleSettingChange("historyMode", historyMode);
      return;
    }

    // Theme selector
    const theme = themeOption.dataset.theme as Theme;
    if (theme && settings) {
      for (const opt of document.querySelectorAll(
        ".appearance-option[data-theme]"
      )) {
        opt.classList.toggle(
          "selected",
          (opt as HTMLElement).dataset.theme === theme
        );
      }
      applyTheme(theme);
      handleSettingChange("theme", theme);
    }
    return;
  }

  // Handle toggle clicks
  const toggle = target.closest(".toggle") as HTMLElement | null;
  if (toggle) {
    if (toggle.classList.contains("disabled")) {
      return;
    }

    const setting = toggle.getAttribute("data-setting") as keyof SettingsType;
    if (!(setting && settings)) {
      return;
    }

    const newValue = !settings[setting];
    if (setting === "autoStart") {
      handleAutoStartToggle(toggle, newValue as boolean).catch((err) => {
        console.error("Failed to update auto-start:", err);
      });
      return;
    }
    if (setting === "lazyModelLoading") {
      handleLazyModelLoadingToggle(toggle, newValue as boolean).catch((err) => {
        console.error("Failed to update lazy model loading:", err);
      });
      return;
    }

    toggle.classList.toggle("active", newValue as boolean);
    handleSettingChange(setting, newValue);
    return;
  }

  // Handle hotkey button
  if (target.closest(".hotkey-btn")) {
    showHotkeyModal();
    return;
  }

  // Handle mic test button
  if (target.closest(".mic-test-btn")) {
    canRefreshAudioDevices()
      .then((canUseMicrophone) => {
        if (!canUseMicrophone) {
          updatePermissionStatus();
          return;
        }

        showMicTestModal();
      })
      .catch((err) => console.error("Failed to check microphone access:", err));
    return;
  }

  if (target.closest(".check-updates-btn")) {
    runManualUpdateCheck().catch((err) => {
      console.error("Update check flow failed:", err);
    });
    return;
  }

  // Handle mic refresh button in settings
  const refreshBtn = target.closest(".mic-refresh-btn") as HTMLButtonElement;
  if (refreshBtn && !refreshBtn.closest(".mic-test-modal")) {
    refreshBtn.disabled = true;
    refreshBtn.classList.add("spinning");
    const minSpinTime = new Promise((r) => setTimeout(r, 1000));
    Promise.all([
      canRefreshAudioDevices().then((canRefresh) =>
        canRefresh ? refreshAudioDevices() : audioDevices
      ),
      minSpinTime,
    ])
      .then(([devices]) => {
        audioDevices = devices;
        renderContent();
      })
      .catch((err) => console.error("Failed to refresh devices:", err))
      .finally(() => {
        refreshBtn.disabled = false;
        refreshBtn.classList.remove("spinning");
      });
    return;
  }

  // Handle model download button
  const downloadModelBtn = target.closest(
    ".download-model-btn"
  ) as HTMLButtonElement | null;
  if (downloadModelBtn) {
    const variant = downloadModelBtn.dataset.variant as ModelVariant;

    downloadModel(variant).catch((err) => {
      console.error("Download error:", err);
    });

    // Start polling for progress
    startModelDownloadPolling(variant);
    return;
  }

  // Handle model activate button
  const activateModelBtn = target.closest(
    ".activate-model-btn"
  ) as HTMLButtonElement | null;
  if (activateModelBtn) {
    const variant = activateModelBtn.dataset.variant as ModelVariant;
    const previousVariant = settings?.activeModelVariant;
    activateModelBtn.disabled = true;
    activateModelBtn.textContent = t("models.activating");

    setActiveModel(variant)
      .then((needsRestart) => {
        if (needsRestart) {
          showRestartDialog(previousVariant);
        } else {
          loadSettings({ force: true, refreshDevices: false }).then(() =>
            renderContent()
          );
        }
      })
      .catch((err) => {
        console.error("Activate error:", err);
        activateModelBtn.disabled = false;
        activateModelBtn.textContent = t("models.activate");
      });
    return;
  }

  // Handle model delete button
  const deleteModelBtn = target.closest(
    ".delete-model-btn"
  ) as HTMLButtonElement | null;
  if (deleteModelBtn) {
    const variant = deleteModelBtn.dataset.variant as ModelVariant;
    const model = models.find((m) => m.variant === variant);
    const modelName = model?.displayName ?? variant;

    showConfirmDialog({
      title: t("models.deleteTitle", { model: modelName }),
      body: t("models.deleteBody"),
      confirmText: t("common.delete"),
      danger: true,
    }).then((confirmed) => {
      if (!confirmed) {
        return;
      }

      deleteModelBtn.disabled = true;
      deleteModelBtn.textContent = t("models.deleting");

      deleteModel(variant)
        .then(() => loadSettings({ force: true, refreshDevices: false }))
        .then(() => renderContent())
        .catch((err) => {
          console.error("Delete error:", err);
          deleteModelBtn.disabled = false;
          deleteModelBtn.textContent = t("common.delete");
        });
    });
  }
}

function handleSettingsChange(e: Event): void {
  const target = e.target as HTMLElement;

  if (target.classList.contains("inference-select") && settings) {
    const select = target as HTMLSelectElement;
    const preference: InferenceDevicePreference =
      select.value === "auto"
        ? { mode: "auto" }
        : select.value === "cpu"
          ? { mode: "cpu" }
          : { mode: "vulkan", deviceId: select.value };
    const updated = { ...settings, inferenceDevice: preference };
    updateSettings(updated)
      .then(async (saved) => {
        settings = saved;
        await refreshInferenceRuntimeInfo(true);
        renderContent();
        if (inferenceRuntimeInfo?.restartRequired) {
          showRestartDialog(undefined, t("dialogs.inferenceRestartBody"));
        }
      })
      .catch((error) => {
        console.error("Failed to update inference device:", error);
        renderContent();
      });
    return;
  }

  // Handle mic select change
  if (target.classList.contains("mic-select")) {
    const select = target as HTMLSelectElement;
    const value = select.value || null;
    handleSettingChange("selectedMicrophoneId", value);
  }
}

function inferencePreferenceValue(
  preference: InferenceDevicePreference | undefined
): string {
  if (preference?.mode === "vulkan") {
    return preference.deviceId;
  }
  return preference?.mode ?? "auto";
}

function inferenceBackendLabel(backend: string): string {
  if (backend === "vulkan") {
    return "Vulkan";
  }
  if (backend === "metal") {
    return "Metal";
  }
  return "CPU";
}

function inferenceFallbackLabel(reason: string): string {
  switch (reason) {
    case "no_vulkan_device":
      return t("settings.inferenceFallbackNoVulkan");
    case "insufficient_gpu_memory":
      return t("settings.inferenceFallbackMemory");
    case "preferred_device_not_found":
      return t("settings.inferenceFallbackMissing");
    case "device_initialization_failed":
      return t("settings.inferenceFallbackInitialization");
    case "execution_fell_back_to_cpu":
      return t("settings.inferenceFallbackExecution");
    default:
      return t("settings.inferenceFallbackGeneric");
  }
}

function renderInferenceSettings(): string {
  if (document.body.dataset.platform !== "windows") {
    return "";
  }
  if (!(settings && inferenceRuntimeInfo)) {
    return `
      <div class="settings-section">
        <div class="settings-section-title">${t("settings.performance")}</div>
        <div class="settings-card">
          <div class="settings-row">
            <div>
              <div class="settings-row-label">${t("settings.inferenceDevice")}</div>
              <div class="settings-row-desc">${t("settings.analyzingHardware")}</div>
            </div>
            <span class="loading-spinner" aria-hidden="true">${createIcon(LoaderCircle)}</span>
          </div>
        </div>
      </div>`;
  }

  const info = inferenceRuntimeInfo;
  const currentValue = inferencePreferenceValue(settings.inferenceDevice);
  const recommended = info.devices.find(
    (device) => device.id === info.recommendedDeviceId
  );
  const options = info.devices
    .map((device) => {
      const selected = currentValue === device.id ? "selected" : "";
      const memory = device.memoryTotalMb
        ? ` · ${Math.round(device.memoryTotalMb)} MB`
        : "";
      return `<option value="${escapeHtml(device.id)}" ${selected}>${escapeHtml(device.name)} · ${inferenceBackendLabel(device.backend)}${memory}</option>`;
    })
    .join("");
  const autoSelected = currentValue === "auto" ? "selected" : "";
  const autoLabel = recommended
    ? t("settings.inferenceAutoWithDevice", { device: recommended.name })
    : t("settings.inferenceAuto");
  const status = escapeHtml(
    info.restartRequired
      ? t("settings.inferenceRestartRequired")
      : info.selectionVerified
        ? t("settings.inferenceSelectedVerified", {
            backend: inferenceBackendLabel(info.resolvedBackend),
            device: info.resolvedDeviceName,
          })
        : t("settings.inferenceSelectedPredicted", {
            backend: inferenceBackendLabel(info.resolvedBackend),
            device: info.resolvedDeviceName,
          })
  );
  const lastExecution = info.lastExecutionBackend
    ? `<div class="inference-runtime-detail">${escapeHtml(
        t(
          info.lastExecutionVerified
            ? "settings.inferenceLastUsed"
            : "settings.inferenceLastSelected",
          {
            backend: inferenceBackendLabel(info.lastExecutionBackend),
            device: info.lastExecutionDeviceName ?? "CPU",
          }
        )
      )}</div>`
    : "";
  const fallback = info.fallbackReason
    ? `<div class="inference-runtime-detail inference-runtime-warning">${inferenceFallbackLabel(info.fallbackReason)}</div>`
    : "";

  return `
    <div class="settings-section">
      <div class="settings-section-title">${t("settings.performance")}</div>
      <div class="settings-card">
        <div class="settings-row inference-settings-row">
          <div>
            <div class="settings-row-label">${t("settings.inferenceDevice")}</div>
            <div class="settings-row-desc">${t("settings.inferenceDeviceDescription")}</div>
            <div class="inference-runtime-detail">${status}</div>
            ${lastExecution}
            ${fallback}
          </div>
          <select class="settings-select inference-select">
            <option value="auto" ${autoSelected}>${escapeHtml(autoLabel)}</option>
            ${options}
          </select>
        </div>
      </div>
    </div>`;
}

async function refreshInferenceRuntimeInfo(force = false): Promise<void> {
  if (!(settings && document.body.dataset.platform === "windows")) {
    return;
  }
  if (
    !force &&
    (inferenceRuntimeLoading ||
      (inferenceRuntimeInfo &&
        inferenceRuntimeVariant === settings.activeModelVariant))
  ) {
    return;
  }

  const request = ++inferenceRuntimeRequest;
  const variant = settings.activeModelVariant;
  inferenceRuntimeLoading = true;
  try {
    const info = await getInferenceRuntimeInfo(variant, force);
    if (request === inferenceRuntimeRequest) {
      inferenceRuntimeInfo = info;
      inferenceRuntimeVariant = variant;
    }
  } catch (error) {
    console.error("Failed to analyze inference hardware:", error);
  } finally {
    if (request === inferenceRuntimeRequest) {
      inferenceRuntimeLoading = false;
    }
  }
}

function formatModelSize(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

function renderModelList(): string {
  const lazyModelRow = `
    <div class="model-lazy-row">
      <div>
        <div class="settings-row-label model-lazy-label">
          <span>${t("models.lazyLoading")}</span>
          <span class="settings-inline-badge experimental">${t("models.experimental")}</span>
        </div>
        <div class="settings-row-desc">${t("models.lazyLoadingDescription")}</div>
      </div>
      <div class="toggle ${settings?.lazyModelLoading ? "active" : ""} ${lazyModelToggleBusy ? "disabled" : ""}" data-setting="lazyModelLoading"></div>
    </div>
  `;

  if (models.length === 0) {
    return `<div class="model-empty">${t("models.loading")}</div>${lazyModelRow}`;
  }

  const header = `
    <div class="model-header">
      <span class="model-col-name">${t("models.model")}</span>
      <span class="model-col-desc">${t("models.accuracy")}</span>
      <span class="model-col-size">${t("models.diskRam")}</span>
      <span class="model-col-actions"></span>
    </div>
  `;

  const rows = models
    .map((model) => {
      const modelProgress =
        modelDownloadProgress?.variant === model.variant
          ? modelDownloadProgress
          : null;
      let actions = "";

      if (model.isActive) {
        actions = `<span class="model-status-badge active">${t("models.inUse")}</span>`;
      } else if (modelProgress) {
        if (modelProgress.status === "verifying") {
          actions = `<span class="model-download-progress verifying"><span class="loading-spinner" aria-hidden="true">${createIcon(LoaderCircle)}</span><span class="model-download-progress-value">${t("models.verifying")}</span></span>`;
        } else {
          const pct = Math.round(modelProgress.percentage);
          actions = `<span class="model-download-progress"><span class="loading-spinner" aria-hidden="true">${createIcon(LoaderCircle)}</span><span class="model-download-progress-value">${pct}%</span></span>`;
        }
      } else if (model.isDownloaded) {
        actions = `
          <button class="btn btn-outline btn-sm activate-model-btn" data-variant="${model.variant}">${t("models.activate")}</button>
          <button class="btn btn-outline btn-sm delete-model-btn" data-variant="${model.variant}">${t("common.delete")}</button>
        `;
      } else {
        actions = `<button class="btn btn-outline btn-sm download-model-btn" data-variant="${model.variant}">${t("common.download")}</button>`;
      }

      return `
        <div class="model-row" data-variant="${model.variant}">
          <span class="model-col-name">${model.displayName}</span>
          <span class="model-col-desc">${getModelQualityLabel(model.variant)}</span>
          <span class="model-col-size">~${formatModelSize(model.sizeBytes)} / ~${model.memoryEstimateMb} MB</span>
          <span class="model-col-actions">${actions}</span>
        </div>
      `;
    })
    .join("");

  return header + rows + lazyModelRow;
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

function renderDictionary(el: HTMLElement): void {
  const terms = getDictionaryTerms();
  const atCapacity = terms.length >= MAX_DICTIONARY_TERMS;
  const termsHtml = terms
    .map(
      (term, index) => `
      <div class="dictionary-item">
        <span class="dictionary-term">${escapeHtml(term)}</span>
        <button class="dictionary-remove-btn" data-index="${index}" title="${t("common.remove")}" aria-label="${t("common.remove")}">${createIcon(Trash2)}</button>
      </div>
    `
    )
    .join("");

  el.innerHTML = `
    <div class="dictionary-sticky-header">
      <h1>${t("dictionary.title")}</h1>
      <div class="dictionary-subtitle">
        ${t("dictionary.subtitle")}
      </div>

      <div class="dictionary-card">
        <div class="dictionary-input-row">
          <input
            type="text"
            class="dictionary-input"
            placeholder="${t("dictionary.placeholder")}"
            autocomplete="off"
            ${atCapacity ? "disabled" : ""}
          >
          <button class="btn btn-accent dictionary-add-btn" ${atCapacity ? "disabled" : ""} title="${t("common.add")}" aria-label="${t("common.add")}">${createIcon(Plus)}</button>
        </div>
        <div class="dictionary-hint">
          ${t("dictionary.usage", {
            used: terms.length,
            maximum: MAX_DICTIONARY_TERMS,
            words: MAX_DICTIONARY_WORDS_PER_TERM,
          })}
        </div>
        ${
          dictionaryError
            ? `<div class="dictionary-error">${escapeHtml(dictionaryError)}</div>`
            : ""
        }
      </div>
    </div>

    <div class="dictionary-scrollable">
      <div class="dictionary-list">
        ${
          terms.length > 0
            ? termsHtml
            : `<div class="empty-state">
                 <div class="empty-state-icon">${createIcon(BookOpen)}</div>
                 <div class="empty-state-title">${t("dictionary.empty")}</div>
               </div>`
        }
      </div>
    </div>
  `;
}

function renderSettingsUI(el: HTMLElement): void {
  const isMac = document.body.dataset.platform === "darwin";
  const micOptions = audioDevices
    .map(
      (d) =>
        `<option value="${escapeHtml(d.id)}" ${settings?.selectedMicrophoneId === d.id || (settings?.selectedMicrophoneId == null && d.isDefault) ? "selected" : ""}>${escapeHtml(d.name)}</option>`
    )
    .join("");

  const currentTheme = settings?.theme ?? "system";
  const currentUiLanguage = settings?.uiLanguage ?? "en";

  el.innerHTML = `
    <h1>${t("settings.title")}</h1>
    <div class="settings-section">
      <div class="settings-section-title">${t("settings.general")}</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.appearance")}</div>
            <div class="settings-row-desc">${t("settings.appearanceDescription")}</div>
          </div>
          <div class="appearance-selector">
            <button class="appearance-option ${currentTheme === "system" ? "selected" : ""}" data-theme="system">
              ${createIcon(Monitor)}
              <span>${t("settings.systemTheme")}</span>
            </button>
            <button class="appearance-option ${currentTheme === "light" ? "selected" : ""}" data-theme="light">
              ${createIcon(Sun)}
              <span>${t("settings.lightTheme")}</span>
            </button>
            <button class="appearance-option ${currentTheme === "dark" ? "selected" : ""}" data-theme="dark">
              ${createIcon(Moon)}
              <span>${t("settings.darkTheme")}</span>
            </button>
          </div>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.appLanguage")}</div>
            <div class="settings-row-desc">${t("settings.appLanguageDescription")}</div>
          </div>
          <div class="appearance-selector">
            <button class="appearance-option ${currentUiLanguage === "en" ? "selected" : ""}" data-ui-language="en">
              <span>English</span>
            </button>
            <button class="appearance-option ${currentUiLanguage === "de" ? "selected" : ""}" data-ui-language="de">
              <span>Deutsch</span>
            </button>
          </div>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.hotkey")}</div>
            <div class="settings-row-desc">${t("settings.hotkeyDescription")}</div>
          </div>
          <button class="hotkey-btn">${settings?.hotkey && parseHotkeyString(settings.hotkey) ? renderHotkeyChips(settings.hotkey, { chipClass: "hotkey-key-inline", separator: "plus" }) : t("settings.setHotkey")}</button>
        </div>
        <div class="settings-row lang-row">
          <div>
            <div class="settings-row-label">${t("settings.transcriptionLanguages")}</div>
            <div class="settings-row-desc">${t("settings.transcriptionLanguagesDescription")}</div>
          </div>
          <div id="transcription-langs"></div>
        </div>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">${t("settings.audio")}</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.microphone")}</div>
            <div class="settings-row-desc">${t("settings.microphoneDescription")}</div>
          </div>
          <div class="mic-select-wrapper">
            <select class="settings-select mic-select">
              ${micOptions}
            </select>
            <button class="btn btn-icon mic-refresh-btn" title="${t("settings.refreshDevices")}">${createIcon(RefreshCw)}</button>
          </div>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.soundFeedback")}</div>
            <div class="settings-row-desc">${t("settings.soundFeedbackDescription")}</div>
          </div>
          <div class="toggle ${settings?.soundEnabled ? "active" : ""}" data-setting="soundEnabled"></div>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.testMicrophone")}</div>
            <div class="settings-row-desc">${t("settings.testMicrophoneDescription")}</div>
          </div>
          <button class="btn btn-outline mic-test-btn">${t("settings.test")}</button>
        </div>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">${t("settings.models")}</div>
      <div class="model-list">
        ${renderModelList()}
      </div>
    </div>
    ${renderInferenceSettings()}
    <div class="settings-section">
      <div class="settings-section-title">${t("settings.permissions")}</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.microphone")}</div>
            <div class="settings-row-desc">${t("settings.voiceRecordingRequired")}</div>
          </div>
          <span class="permission-badge" data-permission="microphone">${t("common.checking")}</span>
        </div>
        ${
          isMac
            ? `<div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.accessibility")}</div>
            <div class="settings-row-desc">${t("settings.accessibilityDescription")}</div>
          </div>
          <span class="permission-badge" data-permission="accessibility">${t("common.checking")}</span>
        </div>`
            : ""
        }
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">${t("settings.data")}</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.transcriptHistory")}</div>
            <div class="settings-row-desc">${t("settings.transcriptHistoryDescription")}</div>
          </div>
          <div class="appearance-selector" data-setting="historyMode">
            <button class="appearance-option ${settings?.historyMode === "off" ? "selected" : ""}" data-history-mode="off">
              <span>${t("common.off")}</span>
            </button>
            <button class="appearance-option ${settings?.historyMode === "off" ? "" : "selected"}" data-history-mode="30d">
              <span>${t("settings.last30Days")}</span>
            </button>
          </div>
        </div>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">${t("settings.system")}</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.applicationUpdates")}</div>
            <div class="settings-row-desc">${t("settings.applicationUpdatesDescription")}</div>
          </div>
          <button class="btn btn-outline check-updates-btn" ${updateCheckInProgress ? "disabled" : ""}>
            ${updateCheckInProgress ? t("common.checking") : getUpdateButtonLabel()}
          </button>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">${t("settings.startOnLogin")}</div>
            <div class="settings-row-desc">${t("settings.startOnLoginDescription")}</div>
          </div>
          <div class="toggle ${settings?.autoStart ? "active" : ""}" data-setting="autoStart"></div>
        </div>
      </div>
    </div>
  `;

  const langMount = el.querySelector("#transcription-langs") as HTMLElement;
  if (langMount) {
    mountLanguageSelect(langMount, {
      selected: settings?.languages ?? ["en"],
      onChange: (next) => {
        void handleSettingChange("languages", next);
      },
    });
  }

  updatePermissionStatus();
  setupScrollFade(el);
}

function renderSettings(el: HTMLElement): void {
  const cacheFresh =
    settings && Date.now() - settingsLoadedAt < SETTINGS_CACHE_TTL;

  // Render immediately to keep navigation snappy.
  renderSettingsUI(el);
  refreshInferenceRuntimeInfo().then(() => {
    if (currentView === "settings") {
      renderSettingsUI(el);
    }
  });

  if (cacheFresh) {
    canRefreshAudioDevices()
      .then((canRefresh) => {
        if (!canRefresh) {
          return audioDevices;
        }

        return refreshAudioDevices();
      })
      .then((devices) => {
        audioDevices = devices;
        if (currentView === "settings") {
          renderSettingsUI(el);
        }
      })
      .catch((err) => console.error("Failed to refresh devices:", err));
    return;
  }

  // Refresh in the background and re-render when ready.
  loadSettings({ refreshDevices: true }).then(() => {
    if (currentView === "settings") {
      renderSettingsUI(el);
    }
  });
}

function updatePermissionStatus(grantedPermission?: SettingsPermission): void {
  refreshPermissionStatus(grantedPermission).catch((err) => {
    console.error("Failed to update permission status:", err);
  });
}

async function refreshPermissionStatus(
  grantedPermission?: SettingsPermission
): Promise<void> {
  const micBadge = document.querySelector(
    '[data-permission="microphone"]'
  ) as HTMLElement;
  const accBadge = document.querySelector(
    '[data-permission="accessibility"]'
  ) as HTMLElement;

  if (!micBadge) {
    return;
  }

  const isMac = document.body.dataset.platform === "darwin";

  if (!isMac) {
    updateBadge(micBadge, "granted", "microphone");
    return;
  }

  if (!accBadge) {
    return;
  }

  const status = await requestPermissions();
  trackPermissionRestartRequirement(status);

  if (grantedPermission && status[grantedPermission] === "granted") {
    markPermissionRestartRequired(grantedPermission);
  }

  updateBadge(micBadge, status.microphone, "microphone");
  updateBadge(accBadge, status.accessibility, "accessibility");
}

function updateBadge(
  badge: HTMLElement,
  status: PermissionStatus[SettingsPermission] | "not-applicable",
  type: SettingsPermission
): void {
  badge.className = "permission-badge";
  badge.onclick = null;

  if (status === "granted" && permissionRestartRequired.has(type)) {
    badge.textContent = t("permissions.restart");
    badge.classList.add("action", "restart");
    badge.onclick = () => {
      relaunchApp().catch((err) => {
        console.error("Failed to restart app:", err);
      });
    };
  } else if (status === "granted") {
    badge.textContent = t("permissions.granted");
    badge.classList.add("granted");
  } else if (status === "not-applicable") {
    badge.textContent = t("common.na");
    badge.classList.add("na");
  } else {
    badge.textContent = t("permissions.allow");
    badge.classList.add("action");
    badge.onclick = async () => {
      badge.textContent = t("permissions.opening");
      badge.classList.remove("action");

      if (type === "microphone") {
        await requestMicrophonePermission();
      } else {
        await requestAccessibilityPermission();
      }

      pollPermissionStatusAfterRequest(type);
    };
  }
}

function pollPermissionStatusAfterRequest(type: SettingsPermission): void {
  const startedAt = Date.now();
  const timeoutMs = 15_000;
  const intervalMs = 500;

  const refresh = async (): Promise<void> => {
    await refreshPermissionStatus(type);

    if (
      permissionRestartRequired.has(type) ||
      Date.now() - startedAt >= timeoutMs
    ) {
      return;
    }

    window.setTimeout(() => {
      refresh().catch((err) => {
        console.error("Failed to refresh permission status:", err);
      });
    }, intervalMs);
  };

  window.setTimeout(() => {
    refresh().catch((err) => {
      console.error("Failed to refresh permission status:", err);
    });
  }, intervalMs);
}

function trackPermissionRestartRequirement(status: PermissionStatus): void {
  if (lastPermissionStatus) {
    markRestartIfNewlyGranted("microphone", lastPermissionStatus, status);
    markRestartIfNewlyGranted("accessibility", lastPermissionStatus, status);
  }

  lastPermissionStatus = status;
}

function markRestartIfNewlyGranted(
  type: SettingsPermission,
  previousStatus: PermissionStatus,
  currentStatus: PermissionStatus
): void {
  if (previousStatus[type] !== "granted" && currentStatus[type] === "granted") {
    markPermissionRestartRequired(type);
  }
}

function markPermissionRestartRequired(type: SettingsPermission): void {
  if (permissionRestartRequired.has(type)) {
    return;
  }

  permissionRestartRequired.add(type);
  armPermissionRestart().catch((err) => {
    console.error("Failed to arm permission restart:", err);
  });
}

function renderAbout(el: HTMLElement): void {
  const info = appInfo;
  el.innerHTML = `
    <div class="about-center">
<img class="about-icon" src="/icon.png" alt="Fing" />
      <h1>Fing</h1>
      <p class="about-tagline">${t("about.tagline")}</p>
      <div class="about-backend">v${info?.version ?? "0.1.0"} · ${info?.commit ?? t("common.unknown")} · ${info?.inferenceBackend ?? t("common.unknown")}</div>
      <div class="about-actions">
        <div class="about-actions-row">
          <a href="https://getfing.com" target="_blank" rel="noreferrer" class="btn btn-outline">${t("about.homepage")} ${createIcon(ArrowUpRight)}</a>
          <a href="https://getfing.com/privacy" target="_blank" rel="noreferrer" class="btn btn-outline">${t("about.privacy")} ${createIcon(ArrowUpRight)}</a>
        </div>
        <div class="about-actions-row">
          <a href="https://github.com/jamdaniels/fing" target="_blank" rel="noreferrer" class="btn btn-outline">GitHub ${createIcon(ArrowUpRight)}</a>
          <a href="mailto:contact@getfing.com" class="btn btn-outline" data-contact-link>${t("about.contact")} ${createIcon(ArrowUpRight)}</a>
        </div>
      </div>
    </div>
  `;

  const contactLink = el.querySelector<HTMLAnchorElement>(
    "[data-contact-link]"
  );
  if (contactLink) {
    contactLink.addEventListener("click", (event) => {
      event.preventDefault();
      invoke("plugin:shell|open", { path: "mailto:contact@getfing.com" }).catch(
        (err) => console.error("Failed to open contact email:", err)
      );
    });
  }
}

async function showOnboarding(reason: BootstrapReason): Promise<void> {
  const app = document.getElementById("app");
  if (!app) {
    return;
  }

  app.innerHTML = `
    <div class="titlebar"></div>
    <div id="onboarding-container"></div>
  `;
  setupTitlebarDrag();
  const container = document.getElementById("onboarding-container");
  if (container) {
    await renderOnboarding(container, {
      modelRepairReason:
        reason === "model_missing" || reason === "model_invalid"
          ? reason
          : undefined,
    });
  }
}

function setupTitlebarDrag(): void {
  const titlebar = document.querySelector(".titlebar");
  if (!titlebar) {
    return;
  }

  titlebar.addEventListener("mousedown", (e) => {
    const event = e as MouseEvent;
    if (event.buttons === 1) {
      getCurrentWindow().startDragging();
    }
  });
}

function setupContentListener(): void {
  const content = document.getElementById("content");
  if (!content || contentListenerAttached) {
    return;
  }
  contentListenerAttached = true;

  content.addEventListener("click", (e) => {
    if (currentView === "history") {
      handleHistoryClick(e, content);
    } else if (currentView === "dictionary") {
      handleDictionaryClick(e);
    } else if (currentView === "settings") {
      handleSettingsClick(e);
    }
  });

  content.addEventListener("input", (e) => {
    if (currentView === "history") {
      handleHistoryInput(e, content);
    }
  });

  content.addEventListener("change", (e) => {
    if (currentView === "settings") {
      handleSettingsChange(e);
    }
  });

  content.addEventListener("keydown", (e) => {
    if (currentView === "dictionary") {
      handleDictionaryKeydown(e as KeyboardEvent);
    }
  });
}

function showMainUI(): void {
  const app = document.getElementById("app");
  if (!app) {
    return;
  }

  // Reset listener flags since we're rebuilding the DOM
  sidebarListenerAttached = false;
  contentListenerAttached = false;

  app.innerHTML = `
    <div class="titlebar"></div>
    <div class="app-body">
      <aside id="sidebar" class="sidebar"></aside>
      <main id="content" class="content"></main>
    </div>
  `;

  setupTitlebarDrag();
  setupContentListener();
  renderSidebar();
  renderContent();
}

// Windows WebView2 hotkey workaround
// WebView2 doesn't propagate keyboard events to low-level hooks when focused
// so we handle hotkeys via JavaScript when the window is focused
let hotkeyConfig: ParsedHotkeyConfig | null = null;
let hotkeyPressed = false;
const pressedHotkeyTokens = new Set<string>();

async function setupHotkeyListener(): Promise<void> {
  if (navigator.userAgent.includes("Mac")) {
    return;
  }

  // Only needed on Windows
  try {
    // Get hotkey from settings (more reliable than backend config which may not be initialized)
    const currentSettings = await getSettings();
    hotkeyConfig = parseHotkeyString(currentSettings.hotkey);
    console.log(
      "[hotkey] Frontend listener configured for:",
      currentSettings.hotkey
    );

    document.addEventListener("keydown", (e) => {
      if (!hotkeyConfig) {
        return;
      }
      if (hotkeyModalCleanup) {
        return;
      }
      if (e.repeat) {
        return;
      }
      const token = eventToHotkeyToken(e);
      if (!token) {
        return;
      }
      pressedHotkeyTokens.add(token);
      if (hotkeyPressed) {
        return;
      }
      if (matchesHotkey(pressedHotkeyTokens, hotkeyConfig)) {
        e.preventDefault();
        e.stopPropagation();
        hotkeyPressed = true;
        hotkeyPress().catch(console.error);
      }
    });

    document.addEventListener("keyup", (e) => {
      if (hotkeyModalCleanup) {
        return;
      }
      if (!(hotkeyConfig && hotkeyPressed)) {
        const token = eventToHotkeyToken(e);
        if (token) {
          pressedHotkeyTokens.delete(token);
        }
        return;
      }
      const token = eventToHotkeyToken(e);
      if (matchesHotkeyRelease(e, hotkeyConfig)) {
        e.preventDefault();
        e.stopPropagation();
        hotkeyPressed = false;
        hotkeyRelease().catch(console.error);
      }
      if (token) {
        pressedHotkeyTokens.delete(token);
      }
    });

    // Also release on blur (window loses focus)
    window.addEventListener("blur", () => {
      if (hotkeyPressed) {
        hotkeyPressed = false;
        hotkeyRelease().catch(console.error);
      }
      pressedHotkeyTokens.clear();
    });
  } catch (err) {
    console.error("Failed to setup hotkey listener:", err);
  }
}

async function showMissingMacPermissionsDialog(
  onboardingCompleted: boolean
): Promise<void> {
  if (document.body.dataset.platform !== "darwin" || !onboardingCompleted) {
    return;
  }

  try {
    const permissions = await requestPermissions();
    trackPermissionRestartRequirement(permissions);

    const missingPermissions: string[] = [];

    if (permissions.microphone !== "granted") {
      missingPermissions.push(t("startup.microphone"));
    }
    if (permissions.accessibility !== "granted") {
      missingPermissions.push(t("startup.accessibility"));
    }

    if (missingPermissions.length === 0) {
      return;
    }

    const { message } = await import("@tauri-apps/plugin-dialog");
    await presentMainWindow(true);
    try {
      await message(
        t("startup.permissionsBody", {
          permissions: missingPermissions.join(t("startup.and")),
        }),
        {
          title: t("startup.permissionsTitle"),
          kind: "warning",
        }
      );
    } finally {
      await presentMainWindow(false);
    }
  } catch (err) {
    console.error("Failed to check permissions on startup:", err);
  }
}

async function showInvalidHotkeyDialog(
  onboardingCompleted: boolean
): Promise<void> {
  if (!onboardingCompleted) {
    return;
  }

  const savedHotkey = settings?.hotkey;
  if (savedHotkey && parseHotkeyString(savedHotkey)) {
    return;
  }

  try {
    const { message } = await import("@tauri-apps/plugin-dialog");
    await presentMainWindow(true);
    try {
      await message(t("startup.newHotkeyBody"), {
        title: t("startup.newHotkeyTitle"),
        kind: "warning",
      });
    } finally {
      await presentMainWindow(false);
    }
  } catch (err) {
    console.error("Failed to show invalid hotkey dialog:", err);
  }
}

async function init(): Promise<void> {
  // Platform detection for platform-specific UI (e.g., hide custom titlebar on Windows)
  const isMac = navigator.userAgent.includes("Mac");
  document.body.dataset.platform = isMac ? "darwin" : "windows";

  await loadSettings({ loadDevices: false });
  const settingsHasCompletedOnboarding = settings?.onboardingCompleted === true;
  let completedByEitherSource = settingsHasCompletedOnboarding;
  let shouldShowOnboarding =
    settings !== null && !settingsHasCompletedOnboarding;
  let bootstrapReason: BootstrapReason = "incomplete_onboarding";

  try {
    const bootstrapStatus = await getBootstrapStatus();
    completedByEitherSource =
      settingsHasCompletedOnboarding ||
      bootstrapStatus.onboardingCompleted === true;
    currentAppState = bootstrapStatus.appState;
    shouldShowOnboarding = bootstrapStatus.shouldShowOnboarding;
    bootstrapReason = bootstrapStatus.reason;
    appInfo = await getAppInfo();
    stats = await getStats().catch(() => null);
    updateStatus = await getUpdateStatus();
  } catch (error) {
    console.error("Failed to load bootstrap status:", error);
  }

  if (!shouldShowOnboarding) {
    await showMissingMacPermissionsDialog(completedByEitherSource);
    await showInvalidHotkeyDialog(completedByEitherSource);
    await loadSettings({ force: true, refreshDevices: true });
  }

  window.addEventListener("setup-completion-started", () => {
    onboardingCompletionInFlight = true;
  });

  window.addEventListener("setup-completion-failed", () => {
    onboardingCompletionInFlight = false;
  });

  await listen("app-state-changed", (event) => {
    const newState = event.payload as AppState;
    if (newState !== "needs-setup" && currentAppState === "needs-setup") {
      if (!onboardingCompletionInFlight) {
        cleanupOnboarding();
        showMainUI();
      }
    }
    currentAppState = newState;
  });

  await listen<{ language: UiLanguage }>("ui-language-changed", (event) => {
    const language = event.payload.language === "de" ? "de" : "en";
    setUiLanguage(language);
    if (settings) {
      settings = { ...settings, uiLanguage: language };
    }
    if (currentAppState !== "needs-setup") {
      renderSidebar();
      renderContent();
    }
  });

  if (shouldShowOnboarding) {
    await showOnboarding(bootstrapReason);
    // Don't set up frontend hotkey listener during onboarding
    // The onboarding flow has its own temporary listener for the test step
  } else {
    showMainUI();
    if (settings?.theme) {
      applyTheme(settings.theme);
    }
    // Setup frontend hotkey handling (Windows WebView2 workaround)
    // Only after main UI is ready and settings are loaded
    setupHotkeyListener().catch(console.error);
  }

  window.addEventListener("setup-complete", async () => {
    cleanupOnboarding();
    currentAppState = "ready";
    await loadSettings({ force: true, refreshDevices: true });
    stats = await getStats().catch(() => null);
    updateStatus = await getUpdateStatus().catch(() => updateStatus);
    showMainUI();
    setupHotkeyListener().catch(console.error);
    onboardingCompletionInFlight = false;
  });

  window.addEventListener("focus", () => {
    if (currentView === "settings") {
      updatePermissionStatus();
    }
  });

  listen("transcript-added", () => {
    const shouldRerender = currentView === "home" || currentView === "history";

    getStats()
      .then((s) => {
        stats = s;
        if (shouldRerender) {
          renderContent();
        }
      })
      .catch(() => {
        // Ignore stats fetch errors
      });
  });

  listen<WindowPresentationRequest>(
    "main-window-presentation-request",
    (event) => {
      void handleMainWindowPresentationRequest(event.payload);
    }
  );

  listen("main-window-hidden", () => {
    if (currentAppState !== "needs-setup") {
      document.documentElement.classList.add("window-route-preparing");
    }
  });

  listen<UpdateStatus>("update-status-changed", (event) => {
    updateStatus = event.payload;
    if (currentView === "settings") {
      renderContent();
    }
  });

  listen("check-for-updates", () => {
    if (currentView !== "settings") {
      navigateToTab("settings");
    }
    runManualUpdateCheck().catch((err) => {
      console.error("Update check flow failed:", err);
    });
  });
}

init();
