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
  Power,
  RefreshCw,
  Search,
  Settings,
  Sun,
  Trash2,
  X,
} from "lucide";
import { cleanupOnboarding, renderOnboarding } from "./components/onboarding";
import type { ParsedHotkeyConfig } from "./lib/hotkey";
import {
  keyEventToHotkey,
  matchesHotkey,
  matchesHotkeyRelease,
  normalizeStoredHotkey,
  parseHotkeyString,
  shouldReleaseOnKeydown,
} from "./lib/hotkey";
import { createIcon, escapeHtml } from "./lib/icons";
import {
  checkForUpdatesNow,
  clearUpdateStatus,
  deleteAllTranscripts,
  deleteModel,
  deleteTranscript,
  downloadModel,
  getAppInfo,
  getAppState,
  getAutoStart,
  getDownloadProgress,
  getMicTestLevel,
  getModels,
  getRecentTranscripts,
  getSettings,
  getStats,
  getUpdateStatus,
  hotkeyPress,
  hotkeyRelease,
  refreshAudioDevices,
  relaunchApp,
  requestAccessibilityPermission,
  requestMicrophonePermission,
  searchTranscripts,
  setActiveModel,
  setAutoStart,
  startMicTest,
  stopMicTest,
  updateSettings,
} from "./lib/ipc";
import type {
  AppInfo,
  AppState,
  AudioDevice,
  HistoryMode,
  MicrophoneTest,
  MicTestStartResult,
  ModelInfo,
  ModelVariant,
  Settings as SettingsType,
  SidebarItem,
  Stats,
  Theme,
  Transcript,
  UpdateCheckResult,
  UpdateStatus,
} from "./lib/types";

declare global {
  interface Window {
    __navigateTo?: (tab: SidebarItem) => void;
  }
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
const ONBOARDING_STATE_RETRY_DELAY_MS = 150;
let settings: SettingsType | null = null;
let settingsLoadedAt = 0;
let audioDevices: AudioDevice[] = [];
let hotkeyModalCleanup: (() => void) | null = null;
let micTestModalCleanup: (() => void) | null = null;
let searchTimeout: ReturnType<typeof setTimeout> | null = null;
let sidebarListenerAttached = false;
let contentListenerAttached = false;
let models: ModelInfo[] = [];
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
    { id: "home", label: "Home", icon: Home },
    { id: "history", label: "History", icon: History },
    { id: "dictionary", label: "Dictionary", icon: BookOpen },
    { id: "settings", label: "Settings", icon: Settings },
    { id: "about", label: "About", icon: Info },
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
      <span>Quit</span>
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
      <h1>Dashboard</h1>
      <div class="dashboard-disabled-wrapper">
        <div class="dashboard-disabled-ghost" aria-hidden="true">
          <div class="stats-grid">
            <div class="stat-card">
              <div class="stat-label">Transcriptions Today</div>
              <div class="stat-value">12</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">Words Today</div>
              <div class="stat-value">847</div>
            </div>
          </div>
          <div class="stat-card" style="margin-bottom: 24px;">
            <div class="stat-label">Most Used Words</div>
            <div class="word-list">${mockWordHtml}</div>
          </div>
          <div class="stats-grid">
            <div class="stat-card">
              <div class="stat-label">Average words per transcription</div>
              <div class="stat-value">71</div>
            </div>
            <div class="stat-card">
              <div class="stat-label">Average speaking speed</div>
              <div class="stat-value">142 <span class="stat-unit">wpm</span></div>
            </div>
          </div>
        </div>
        <div class="dashboard-disabled-overlay">
          <div class="dashboard-disabled-card">
            <div class="dashboard-disabled-title">History is disabled</div>
            <p>Enable history in settings to see your stats</p>
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
      : '<div class="stat-empty">No data yet</div>';

  el.innerHTML = `
    <h1>Dashboard</h1>
    <div class="stats-grid">
      <div class="stat-card">
        <div class="stat-label">Transcriptions Today</div>
        <div class="stat-value">${stats?.transcriptionsToday ?? 0}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Words Today</div>
        <div class="stat-value">${stats?.wordsToday ?? 0}</div>
      </div>
    </div>
    <div class="stat-card" style="margin-bottom: 24px;">
      <div class="stat-label">Most Used Words</div>
      <div class="word-list">${wordListHtml}</div>
    </div>
    <div class="stats-grid">
      <div class="stat-card">
        <div class="stat-label">Average words per transcription</div>
        <div class="stat-value">${stats?.averageWordsPerTranscription?.toFixed(0) ?? 0}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Average speaking speed</div>
        <div class="stat-value">${stats?.averageWpm?.toFixed(0) ?? 0} <span class="stat-unit">wpm</span></div>
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
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function getDateGroup(createdAt: string): string {
  const now = new Date();
  const date = parseCreatedAt(createdAt);
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterday = new Date(today.getTime() - 86_400_000);
  const weekAgo = new Date(today.getTime() - 7 * 86_400_000);

  if (date >= today) {
    return "Today";
  }
  if (date >= yesterday) {
    return "Yesterday";
  }
  if (date >= weekAgo) {
    return "This Week";
  }
  return "Older";
}

function groupTranscriptsByDate(
  items: Transcript[]
): Map<string, Transcript[]> {
  const groups = new Map<string, Transcript[]>();
  const order = ["Today", "Yesterday", "This Week", "Older"];

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
    title: "Clear All Transcripts",
    body: "Are you sure you want to delete all transcripts?",
    confirmText: "Delete All",
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
      <div class="date-group-header">Today</div>
      ${mockTranscripts
        .map(
          (m) => `
        <div class="list-item">
          <div class="list-item-header">
            <span class="list-item-time">${m.time}</span>
            <span class="list-item-words">${m.words} words</span>
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
        <div class="history-header"><h1>History</h1></div>
      </div>
      <div class="history-scrollable">
        <div class="dashboard-disabled-wrapper">
          <div class="dashboard-disabled-ghost" aria-hidden="true">
            <div class="search-wrapper" style="margin-bottom: 16px;">
              ${createIcon(Search)}
              <input type="text" class="search-input" placeholder="Search transcripts..." disabled>
            </div>
            <div class="transcript-list">${mockListHtml}</div>
          </div>
          <div class="dashboard-disabled-overlay">
            <div class="dashboard-disabled-card">
              <div class="dashboard-disabled-title">History is disabled</div>
              <p>Enable history in settings to save transcripts</p>
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
        listHtml += `<div class="date-group-header">${group}</div>`;
        for (const item of items) {
          listHtml += `
            <div class="list-item" data-id="${item.id}">
              <div class="list-item-header">
                <span class="list-item-time">${formatTime(item.createdAt)}</span>
                <span class="list-item-words">${item.wordCount} words</span>
              </div>
              <div class="list-item-text">${escapeHtml(truncateText(item.text, 150))}</div>
              <div class="list-item-actions">
                <button class="copy-btn" title="Copy to clipboard">${createIcon(Copy)}</button>
                <button class="delete-btn" title="Delete">${createIcon(Trash2)}</button>
              </div>
            </div>
          `;
        }
      }
    }

    el.innerHTML = `
      <div class="history-sticky-header">
        <div class="history-header">
          <h1>History</h1>
          ${hasTranscripts ? `<button class="btn btn-danger btn-sm clear-all-btn">Clear All</button>` : ""}
        </div>
        <div class="search-wrapper">
          ${createIcon(Search)}
          <input type="text" class="search-input" placeholder="Search transcripts..." value="${escapeHtml(searchQuery)}">
        </div>
      </div>
      <div class="history-scrollable">
        ${
          hasTranscripts
            ? `<div class="transcript-list">${listHtml}</div>
               ${hasMoreTranscripts ? `<button class="btn btn-secondary load-more-btn">Load more</button>` : ""}`
            : `
          <div class="empty-state">
            ${searchQuery ? "" : `<div class="empty-state-icon">${createIcon(Mic)}</div>`}
            <div class="empty-state-title">${searchQuery ? "No results found" : "No transcripts yet"}</div>
            ${searchQuery ? "<p>Try a different search term</p>" : ""}
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
    return "Enter a word or short phrase";
  }
  if (dictionaryWordCount(term) > MAX_DICTIONARY_WORDS_PER_TERM) {
    return `Terms can be up to ${MAX_DICTIONARY_WORDS_PER_TERM} words`;
  }

  const existing = getDictionaryTerms();
  if (existing.length >= MAX_DICTIONARY_TERMS) {
    return `Dictionary is full (${MAX_DICTIONARY_TERMS} terms)`;
  }
  if (existing.some((value) => value.toLowerCase() === term.toLowerCase())) {
    return "That term is already in your dictionary";
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
    dictionaryError = "Failed to save term";
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
    dictionaryError = "Failed to update dictionary";
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
  options: { force?: boolean; refreshDevices?: boolean } = {}
): Promise<void> {
  const { force = false, refreshDevices = false } = options;

  // Skip if cache is fresh (within TTL)
  if (
    !force &&
    settings &&
    Date.now() - settingsLoadedAt < SETTINGS_CACHE_TTL
  ) {
    return;
  }

  try {
    const shouldRefreshDevices = refreshDevices || audioDevices.length === 0;
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
    };
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

function formatKeyForDisplay(key: string): string {
  // Handle combination strings like "Option+Space"
  const parts = key.split("+");
  const formatted = parts.map((part) => {
    const keyMap: Record<string, string> = {
      Ctrl: "Ctrl",
      Control: "Ctrl",
      Option: "Option",
      Alt: "Option",
      Shift: "Shift",
      Cmd: "Cmd",
      Meta: "Cmd",
      Fn: "Fn",
      F1: "F1",
      F2: "F2",
      F3: "F3",
      F4: "F4",
      F5: "F5",
      F6: "F6",
      F7: "F7",
      F8: "F8",
      F9: "F9",
      F10: "F10",
      F11: "F11",
      F12: "F12",
      Space: "Space",
      Escape: "Esc",
      " ": "Space",
    };
    return keyMap[part] || part;
  });
  return formatted.join(" + ");
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
  const currentHotkey = normalizeStoredHotkey(settings?.hotkey);

  const modal = document.createElement("div");
  modal.className = "dialog-overlay";

  const renderModal = () => {
    modal.innerHTML = `
      <div class="dialog hotkey-dialog">
        <button class="dialog-close">${createIcon(X)}</button>
        <div class="dialog-header">
          <div class="dialog-title">Set recording hotkey</div>
        </div>
        <div class="dialog-body">
          <div class="hotkey-dialog-desc">Press a key or combination</div>
          <div class="hotkey-modal-preview">
            <span class="hotkey-modal-key ${capturedHotkey ? "captured" : ""}">${formatKeyForDisplay(capturedHotkey ?? currentHotkey)}</span>
            ${capturedHotkey && capturedHotkey !== currentHotkey ? '<span class="hotkey-modal-new">New</span>' : ""}
          </div>
          <button class="hotkey-fn-link">Use Fn key instead</button>
        </div>
        <div class="dialog-footer hotkey-dialog-footer">
          <span class="hotkey-dialog-hint">Press Escape to cancel</span>
          <button class="btn btn-accent hotkey-confirm-btn" ${capturedHotkey ? "" : "disabled"}>Set hotkey</button>
        </div>
      </div>
    `;

    // Re-attach event listeners after re-render
    modal
      .querySelector(".dialog-close")
      ?.addEventListener("click", closeHotkeyModal);

    modal.querySelector(".hotkey-fn-link")?.addEventListener("click", () => {
      capturedHotkey = "Fn";
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
        btn.textContent = "Setting...";

        if (await setHotkey(capturedHotkey)) {
          closeHotkeyModal();
          renderContent();
        } else {
          const desc = modal.querySelector(".hotkey-dialog-desc");
          if (desc) {
            desc.textContent = "Failed to set hotkey. Try another key.";
            desc.classList.add("error");
          }
          btn.disabled = false;
          btn.textContent = "Set hotkey";
        }
      });
  };

  renderModal();
  document.body.appendChild(modal);

  const keyHandler = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();

    if (e.key === "Escape") {
      closeHotkeyModal();
      return;
    }

    const hotkey = keyEventToHotkey(e);
    if (!hotkey) {
      return;
    }

    capturedHotkey = hotkey;
    renderModal();
  };

  const clickHandler = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("dialog-overlay")) {
      closeHotkeyModal();
    }
  };

  document.addEventListener("keydown", keyHandler);
  modal.addEventListener("click", clickHandler);

  hotkeyModalCleanup = () => {
    document.removeEventListener("keydown", keyHandler);
    modal.removeEventListener("click", clickHandler);
    modal.remove();
    hotkeyModalCleanup = null;
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
          <div class="dialog-title">Test Microphone</div>
        </div>
        <div class="dialog-body">
          <div class="mic-test-container">
            <div class="mic-select-row">
              <label for="modal-mic-select">Device:</label>
              <div class="mic-select-wrapper">
                <select id="modal-mic-select" class="settings-select">
                  ${getMicOptions()}
                </select>
                <button class="btn btn-icon mic-refresh-btn" title="Refresh devices">${createIcon(RefreshCw)}</button>
              </div>
            </div>

            ${
              showMismatchWarning
                ? `<div class="mic-mismatch-warning">
                Selected device not found. Using fallback device.
              </div>`
                : ""
            }

            <div class="audio-level-container">
              <div class="audio-level-label">Audio Level</div>
              <div class="audio-level-bar">
                <div class="audio-level-fill ${levelPercent > 10 ? "active" : ""}" style="width: ${levelPercent}%"></div>
              </div>
            </div>

            <div class="mic-test-prompt ${audioDetected ? "success" : ""}">
              ${
                audioDetected
                  ? `${createIcon(CheckCircle)} Audio detected`
                  : `${createIcon(Mic)} Say something to test...`
              }
            </div>
          </div>
        </div>
        <div class="dialog-footer mic-test-footer">
          <button class="btn btn-accent mic-test-done-btn">Done</button>
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
      prompt.innerHTML = `${createIcon(CheckCircle)} Audio detected`;
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

function showRestartDialog(previousVariant?: ModelVariant): void {
  const modal = document.createElement("div");
  modal.className = "dialog-overlay";
  modal.innerHTML = `
    <div class="dialog">
      <div class="dialog-header">
        <div class="dialog-title">Restart Required</div>
      </div>
      <div class="dialog-body">Fing needs to restart to load the new model.</div>
      <div class="dialog-footer">
        <button class="btn btn-outline" id="restart-later-btn">Later</button>
        <button class="btn btn-accent" id="restart-now-btn">Restart Now</button>
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
    confirmText = "Confirm",
    cancelText = "Cancel",
    danger = false,
  } = options;
  return new Promise((resolve) => {
    const modal = document.createElement("div");
    modal.className = "dialog-overlay";
    modal.innerHTML = `
      <div class="dialog">
        <div class="dialog-header">
          <div class="dialog-title">${title}</div>
        </div>
        <div class="dialog-body">${body}</div>
        <div class="dialog-footer">
          <button class="btn btn-outline" id="dialog-cancel-btn">${cancelText}</button>
          <button class="btn ${danger ? "btn-danger" : "btn-accent"}" id="dialog-confirm-btn">${confirmText}</button>
        </div>
      </div>
    `;

    document.body.appendChild(modal);

    document
      .getElementById("dialog-cancel-btn")
      ?.addEventListener("click", () => {
        modal.remove();
        resolve(false);
      });

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

  return "Unknown error";
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
    ? "Update Available"
    : "Check for Updates";
}

function setUpdateButtonBusy(isBusy: boolean): void {
  const button = document.querySelector(
    ".check-updates-btn"
  ) as HTMLButtonElement | null;
  if (!button) {
    return;
  }

  button.disabled = isBusy;
  button.textContent = isBusy ? "Checking..." : getUpdateButtonLabel();
}

async function promptForAvailableUpdate(
  status: UpdateCheckResult
): Promise<boolean> {
  const { ask } = await import("@tauri-apps/plugin-dialog");
  const notesPreview = getUpdateNotesPreview(status.availableBody);
  const prompt = notesPreview
    ? `Version ${status.availableVersion ?? "a newer version"} is available.\n\nRelease notes:\n${notesPreview}\n\nInstall now?`
    : `Version ${status.availableVersion ?? "a newer version"} is available.\n\nInstall now?`;

  return await ask(prompt, {
    title: "Update Available",
    kind: "info",
    okLabel: "Install Update",
    cancelLabel: "Later",
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
    await message("Fing is up to date.", {
      title: "No Updates Available",
      kind: "info",
    });
    return;
  }

  await update.downloadAndInstall();
  updateStatus = await clearUpdateStatus();
  await message("The update was installed. Fing will now restart.", {
    title: "Update Installed",
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
      await message("Fing is up to date.", {
        title: "No Updates Available",
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
    await message(
      `Could not complete update check.\n\n${formatErrorMessage(error)}`,
      {
        title: "Update Check Failed",
        kind: "error",
      }
    );
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
    title: "Turn Off History?",
    body: "All saved transcripts will be permanently deleted.",
    confirmText: "Confirm",
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

// biome-ignore lint/complexity/noExcessiveCognitiveComplexity: UI event handler with necessary branches
function handleSettingsClick(e: MouseEvent): void {
  const target = e.target as HTMLElement;

  // Handle theme selection
  const themeOption = target.closest(
    ".appearance-option"
  ) as HTMLElement | null;
  if (themeOption) {
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
    showMicTestModal();
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
    Promise.all([refreshAudioDevices(), minSpinTime])
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
    activateModelBtn.textContent = "Activating...";

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
        activateModelBtn.textContent = "Activate";
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
      title: `Delete ${modelName}`,
      body: "You can download it again later.",
      confirmText: "Delete",
      danger: true,
    }).then((confirmed) => {
      if (!confirmed) {
        return;
      }

      deleteModelBtn.disabled = true;
      deleteModelBtn.textContent = "Deleting...";

      deleteModel(variant)
        .then(() => loadSettings({ force: true, refreshDevices: false }))
        .then(() => renderContent())
        .catch((err) => {
          console.error("Delete error:", err);
          deleteModelBtn.disabled = false;
          deleteModelBtn.textContent = "Delete";
        });
    });
  }
}

function handleSettingsChange(e: Event): void {
  const target = e.target as HTMLElement;

  // Handle mic select change
  if (target.classList.contains("mic-select")) {
    const select = target as HTMLSelectElement;
    const value = select.value || null;
    handleSettingChange("selectedMicrophoneId", value);
  }

  // Handle language checkbox change
  if (target.classList.contains("lang-check")) {
    const checkboxes = document.querySelectorAll(
      ".lang-check"
    ) as NodeListOf<HTMLInputElement>;
    const selected = Array.from(checkboxes)
      .filter((cb) => cb.checked)
      .map((cb) => cb.dataset.lang as string);

    // Require at least one language
    if (selected.length === 0) {
      (target as HTMLInputElement).checked = true;
      return;
    }

    handleSettingChange("languages", selected);
  }
}

const SUPPORTED_LANGUAGES = [
  { code: "en", name: "English" },
  { code: "de", name: "German" },
  { code: "es", name: "Spanish" },
  { code: "fr", name: "French" },
];

function formatModelSize(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} MB`;
}

function renderModelList(): string {
  const lazyModelRow = `
    <div class="model-lazy-row">
      <div>
        <div class="settings-row-label model-lazy-label">
          <span>Lazy model loading</span>
          <span class="settings-inline-badge experimental">Experimental</span>
        </div>
        <div class="settings-row-desc">Save memory and load model on hotkey press; unloads after 10s idle</div>
      </div>
      <div class="toggle ${settings?.lazyModelLoading ? "active" : ""} ${lazyModelToggleBusy ? "disabled" : ""}" data-setting="lazyModelLoading"></div>
    </div>
  `;

  if (models.length === 0) {
    return `<div class="model-empty">Loading models...</div>${lazyModelRow}`;
  }

  const header = `
    <div class="model-header">
      <span class="model-col-name">Model</span>
      <span class="model-col-desc">Accuracy</span>
      <span class="model-col-size">Disk / RAM</span>
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
        actions = `<span class="model-status-badge active">In Use</span>`;
      } else if (modelProgress) {
        if (modelProgress.status === "verifying") {
          actions = `<span class="model-download-progress verifying"><span class="loading-spinner" aria-hidden="true">${createIcon(LoaderCircle)}</span><span class="model-download-progress-value">Verifying...</span></span>`;
        } else {
          const pct = Math.round(modelProgress.percentage);
          actions = `<span class="model-download-progress"><span class="loading-spinner" aria-hidden="true">${createIcon(LoaderCircle)}</span><span class="model-download-progress-value">${pct}%</span></span>`;
        }
      } else if (model.isDownloaded) {
        actions = `
          <button class="btn btn-secondary btn-sm activate-model-btn" data-variant="${model.variant}">Activate</button>
          <button class="btn btn-secondary btn-sm delete-model-btn" data-variant="${model.variant}">Delete</button>
        `;
      } else {
        actions = `<button class="btn btn-secondary btn-sm download-model-btn" data-variant="${model.variant}">Download</button>`;
      }

      return `
        <div class="model-row" data-variant="${model.variant}">
          <span class="model-col-name">${model.displayName}</span>
          <span class="model-col-desc">${model.description}</span>
          <span class="model-col-size">~${formatModelSize(model.sizeBytes)} / ~${model.memoryEstimateMb} MB</span>
          <span class="model-col-actions">${actions}</span>
        </div>
      `;
    })
    .join("");

  return header + rows + lazyModelRow;
}

function renderDictionary(el: HTMLElement): void {
  const terms = getDictionaryTerms();
  const atCapacity = terms.length >= MAX_DICTIONARY_TERMS;
  const termsHtml = terms
    .map(
      (term, index) => `
      <div class="dictionary-item">
        <span class="dictionary-term">${escapeHtml(term)}</span>
        <button class="btn btn-secondary btn-sm dictionary-remove-btn" data-index="${index}">Remove</button>
      </div>
    `
    )
    .join("");

  el.innerHTML = `
    <div class="dictionary-sticky-header">
      <h1>Dictionary</h1>
      <div class="dictionary-subtitle">
        Add words and short phrases you use often. Fing will prefer these when transcribing.
      </div>

      <div class="dictionary-card">
        <div class="dictionary-input-row">
          <input
            type="text"
            class="dictionary-input"
            placeholder="Add a term"
            autocomplete="off"
            ${atCapacity ? "disabled" : ""}
          >
          <button class="btn btn-accent dictionary-add-btn" ${atCapacity ? "disabled" : ""}>Add</button>
        </div>
        <div class="dictionary-hint">
          ${terms.length}/${MAX_DICTIONARY_TERMS} terms used · Up to ${MAX_DICTIONARY_WORDS_PER_TERM} words each
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
                 <div class="empty-state-title">No dictionary terms yet</div>
               </div>`
        }
      </div>
    </div>
  `;
}

function renderSettingsUI(el: HTMLElement): void {
  const micOptions = audioDevices
    .map(
      (d) =>
        `<option value="${escapeHtml(d.id)}" ${settings?.selectedMicrophoneId === d.id || (settings?.selectedMicrophoneId == null && d.isDefault) ? "selected" : ""}>${escapeHtml(d.name)}</option>`
    )
    .join("");

  const selectedLangs = settings?.languages ?? ["en"];
  const langCheckboxes = SUPPORTED_LANGUAGES.map(
    (lang) => `
      <label class="lang-checkbox">
        <input type="checkbox" class="lang-check" data-lang="${lang.code}" ${selectedLangs.includes(lang.code) ? "checked" : ""}>
        <span>${lang.name}</span>
      </label>
    `
  ).join("");

  const currentTheme = settings?.theme ?? "system";

  el.innerHTML = `
    <h1>Settings</h1>
    <div class="settings-section">
      <div class="settings-section-title">General</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Appearance</div>
            <div class="settings-row-desc">Choose your preferred theme</div>
          </div>
          <div class="appearance-selector">
            <button class="appearance-option ${currentTheme === "system" ? "selected" : ""}" data-theme="system">
              ${createIcon(Monitor)}
              <span>System</span>
            </button>
            <button class="appearance-option ${currentTheme === "light" ? "selected" : ""}" data-theme="light">
              ${createIcon(Sun)}
              <span>Light</span>
            </button>
            <button class="appearance-option ${currentTheme === "dark" ? "selected" : ""}" data-theme="dark">
              ${createIcon(Moon)}
              <span>Dark</span>
            </button>
          </div>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Hotkey</div>
            <div class="settings-row-desc">Press and hold to record</div>
          </div>
          <button class="btn btn-secondary hotkey-btn">${formatKeyForDisplay(normalizeStoredHotkey(settings?.hotkey))}</button>
        </div>
        <div class="settings-row lang-row">
          <div>
            <div class="settings-row-label">Language</div>
            <div class="settings-row-desc">Select one for best accuracy, or multiple for auto-detection</div>
          </div>
          <div class="lang-checkboxes">${langCheckboxes}</div>
        </div>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">Audio</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Microphone</div>
            <div class="settings-row-desc">Select audio input device</div>
          </div>
          <div class="mic-select-wrapper">
            <select class="settings-select mic-select">
              ${micOptions}
            </select>
            <button class="btn btn-icon mic-refresh-btn" title="Refresh devices">${createIcon(RefreshCw)}</button>
          </div>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Sound feedback</div>
            <div class="settings-row-desc">Play sounds for recording start/stop</div>
          </div>
          <div class="toggle ${settings?.soundEnabled ? "active" : ""}" data-setting="soundEnabled"></div>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Test microphone</div>
            <div class="settings-row-desc">Check if your microphone is working</div>
          </div>
          <button class="btn btn-secondary mic-test-btn">Test</button>
        </div>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">Models</div>
      <div class="model-list">
        ${renderModelList()}
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">Permissions</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Microphone</div>
            <div class="settings-row-desc">Required for voice recording</div>
          </div>
          <span class="permission-badge" data-permission="microphone">Checking...</span>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Accessibility</div>
            <div class="settings-row-desc">Required for global hotkey and paste</div>
          </div>
          <span class="permission-badge" data-permission="accessibility">Checking...</span>
        </div>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">Data</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Transcript history</div>
            <div class="settings-row-desc">Store transcripts locally for search</div>
          </div>
          <div class="appearance-selector" data-setting="historyMode">
            <button class="appearance-option ${settings?.historyMode === "off" ? "selected" : ""}" data-history-mode="off">
              <span>Off</span>
            </button>
            <button class="appearance-option ${settings?.historyMode !== "off" ? "selected" : ""}" data-history-mode="30d">
              <span>Last 30 days</span>
            </button>
          </div>
        </div>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">System</div>
      <div class="settings-card">
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Application updates</div>
            <div class="settings-row-desc">Check for and install the latest version</div>
          </div>
          <button class="btn btn-secondary check-updates-btn" ${updateCheckInProgress ? "disabled" : ""}>
            ${updateCheckInProgress ? "Checking..." : getUpdateButtonLabel()}
          </button>
        </div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Start on login</div>
            <div class="settings-row-desc">Launch Fing when you log in</div>
          </div>
          <div class="toggle ${settings?.autoStart ? "active" : ""}" data-setting="autoStart"></div>
        </div>
      </div>
    </div>
  `;

  updatePermissionStatus();
}

function renderSettings(el: HTMLElement): void {
  const cacheFresh =
    settings && Date.now() - settingsLoadedAt < SETTINGS_CACHE_TTL;

  // Render immediately to keep navigation snappy.
  renderSettingsUI(el);

  if (cacheFresh) {
    refreshAudioDevices()
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

function updatePermissionStatus(): void {
  const micBadge = document.querySelector(
    '[data-permission="microphone"]'
  ) as HTMLElement;
  const accBadge = document.querySelector(
    '[data-permission="accessibility"]'
  ) as HTMLElement;

  if (!(micBadge && accBadge)) {
    return;
  }

  const isMac = document.body.dataset.platform === "darwin";
  const hasOnboarded = settings?.onboardingCompleted ?? false;

  if (isMac) {
    const status = hasOnboarded ? "granted" : "denied";
    updateBadge(micBadge, status, "microphone");
    updateBadge(accBadge, status, "accessibility");
    return;
  }

  updateBadge(micBadge, "granted", "microphone");
  updateBadge(accBadge, "not-applicable", "accessibility");
}

function updateBadge(
  badge: HTMLElement,
  status: string,
  type: "microphone" | "accessibility"
): void {
  badge.className = "permission-badge";

  if (status === "granted") {
    badge.textContent = "Granted";
    badge.classList.add("granted");
  } else if (status === "not-applicable") {
    badge.textContent = "N/A";
    badge.classList.add("na");
  } else {
    badge.textContent = "Grant";
    badge.classList.add("action");
    badge.style.cursor = "pointer";
    badge.onclick = async () => {
      badge.textContent = "Opening...";
      badge.classList.remove("action");
      badge.style.cursor = "default";

      if (type === "microphone") {
        await requestMicrophonePermission();
      } else {
        await requestAccessibilityPermission();
      }

      setTimeout(updatePermissionStatus, 1500);
    };
  }
}

function renderAbout(el: HTMLElement): void {
  const info = appInfo;
  el.innerHTML = `
    <div class="about-center">
<img class="about-icon" src="/icon.png" alt="Fing" />
      <h1>Fing</h1>
      <div class="about-version">
        Version: ${info?.version ?? "0.1.0"} (${info?.commit ?? "unknown"})
      </div>
      <p class="about-tagline">Fast, private, local speech-to-text</p>
      <div class="about-backend">Backend: ${info?.inferenceBackend ?? "Unknown"}</div>
      <div class="about-actions">
        <a href="https://github.com/jamdaniels/fing" target="_blank" rel="noreferrer" class="btn btn-secondary">GitHub ${createIcon(ArrowUpRight)}</a>
        <a href="https://x.com/coding_jam" target="_blank" rel="noreferrer" class="btn btn-secondary">Feedback ${createIcon(ArrowUpRight)}</a>
      </div>
    </div>
  `;
}

async function showOnboarding(): Promise<void> {
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
    await renderOnboarding(container);
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

async function setupHotkeyListener(): Promise<void> {
  if (navigator.userAgent.includes("Mac")) {
    return;
  }

  // Only needed on Windows
  try {
    // Get hotkey from settings (more reliable than backend config which may not be initialized)
    const currentSettings = await getSettings();
    const hotkeyStr = normalizeStoredHotkey(currentSettings.hotkey);
    hotkeyConfig = parseHotkeyString(hotkeyStr);
    console.log("[hotkey] Frontend listener configured for:", hotkeyStr);

    document.addEventListener("keydown", (e) => {
      if (!hotkeyConfig) {
        return;
      }
      if (hotkeyPressed) {
        if (shouldReleaseOnKeydown(e, hotkeyConfig)) {
          hotkeyPressed = false;
          hotkeyRelease().catch(console.error);
        }
        return;
      }
      if (matchesHotkey(e, hotkeyConfig)) {
        e.preventDefault();
        e.stopPropagation();
        hotkeyPressed = true;
        hotkeyPress().catch(console.error);
      }
    });

    document.addEventListener("keyup", (e) => {
      if (!(hotkeyConfig && hotkeyPressed)) {
        return;
      }
      if (matchesHotkeyRelease(e, hotkeyConfig)) {
        e.preventDefault();
        e.stopPropagation();
        hotkeyPressed = false;
        hotkeyRelease().catch(console.error);
      }
    });

    // Also release on blur (window loses focus)
    window.addEventListener("blur", () => {
      if (hotkeyPressed) {
        hotkeyPressed = false;
        hotkeyRelease().catch(console.error);
      }
    });
  } catch (err) {
    console.error("Failed to setup hotkey listener:", err);
  }
}

async function init(): Promise<void> {
  // Platform detection for platform-specific UI (e.g., hide custom titlebar on Windows)
  const isMac = navigator.userAgent.includes("Mac");
  document.body.dataset.platform = isMac ? "darwin" : "windows";

  await loadSettings({ refreshDevices: true });

  try {
    currentAppState = await getAppState();
    appInfo = await getAppInfo();
    stats = await getStats().catch(() => null);
    updateStatus = await getUpdateStatus();
  } catch {
    // Commands may not be registered yet
  }

  await listen("app-state-changed", (event) => {
    const newState = event.payload as AppState;
    if (newState !== "needs-setup" && currentAppState === "needs-setup") {
      cleanupOnboarding();
      showMainUI(); // Rebuild DOM so tray menu can navigate properly
    }
    currentAppState = newState;
  });

  const hasCompletedOnboarding = settings?.onboardingCompleted ?? false;
  const hasSavedOnboardingStep = settings?.onboardingStep !== null;
  let shouldShowOnboarding = currentAppState === "needs-setup";

  if (
    shouldShowOnboarding &&
    hasCompletedOnboarding &&
    !hasSavedOnboardingStep
  ) {
    await new Promise((resolve) =>
      window.setTimeout(resolve, ONBOARDING_STATE_RETRY_DELAY_MS)
    );

    try {
      currentAppState = await getAppState();
    } catch {
      // Keep the previously observed state
    }

    shouldShowOnboarding = currentAppState === "needs-setup";
  }

  if (shouldShowOnboarding) {
    await showOnboarding();
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
    showMainUI(); // Rebuild DOM to main UI structure before hiding
    // Now set up the frontend hotkey listener with the user's chosen hotkey
    setupHotkeyListener().catch(console.error);
    getCurrentWindow().hide();
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

  // Listen for navigation events from tray menu
  listen<string>("navigate-to-tab", (event) => {
    navigateToTab(event.payload as SidebarItem);
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

// Allow backend to navigate before showing the window.
window.__navigateTo = (tab: SidebarItem) => {
  navigateToTab(tab);
};

init();
