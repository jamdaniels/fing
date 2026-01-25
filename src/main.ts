import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ArrowUpRight,
  Check,
  CheckCircle,
  Copy,
  History,
  Home,
  type IconNode,
  Info,
  Keyboard,
  Mic,
  Power,
  Search,
  Settings,
  Trash2,
  X,
} from "lucide";
import { cleanupOnboarding, renderOnboarding } from "./components/onboarding";
import { createIcon } from "./lib/icons";
import {
  deleteAllTranscripts,
  deleteTranscript,
  getAppInfo,
  getAppState,
  getAudioDevices,
  getAutoStart,
  getMicTestLevel,
  getRecentTranscripts,
  getSettings,
  getStats,
  requestAccessibilityPermission,
  requestMicrophonePermission,
  requestPermissions,
  searchTranscripts,
  setAutoStart,
  startMicTest,
  stopMicTest,
  updateSettings,
} from "./lib/ipc";
import type {
  AppInfo,
  AppState,
  AudioDevice,
  MicrophoneTest,
  Settings as SettingsType,
  SidebarItem,
  Stats,
  Transcript,
} from "./lib/types";

let currentView: SidebarItem = "home";
let appInfo: AppInfo | null = null;
let stats: Stats | null = null;
let currentAppState: AppState = "initializing";
let transcripts: Transcript[] = [];
let searchQuery = "";
let transcriptOffset = 0;
let hasMoreTranscripts = true;
const PAGE_SIZE = 50;
let settings: SettingsType | null = null;
let audioDevices: AudioDevice[] = [];
let hotkeyModalCleanup: (() => void) | null = null;
let micTestModalCleanup: (() => void) | null = null;

function renderSidebar(): void {
  const sidebar = document.getElementById("sidebar");
  if (!sidebar) {
    return;
  }

  const items: { id: SidebarItem; label: string; icon: IconNode }[] = [
    { id: "home", label: "Home", icon: Home },
    { id: "history", label: "History", icon: History },
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

  for (const el of sidebar.querySelectorAll(".sidebar-item")) {
    el.addEventListener("click", () => {
      const view = el.getAttribute("data-view") as SidebarItem | null;
      const action = el.getAttribute("data-action");
      if (view) {
        currentView = view;
        renderSidebar();
        renderContent();
      } else if (action === "quit") {
        invoke("quit_app");
      }
    });
  }
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
  el.innerHTML = `
    <h1>Dashboard</h1>
    <div class="stats-grid">
      <div class="stat-card">
        <div class="stat-label">Total Transcriptions</div>
        <div class="stat-value">${stats?.totalTranscriptions ?? 0}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Today</div>
        <div class="stat-value">${stats?.transcriptionsToday ?? 0}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Total Words</div>
        <div class="stat-value">${stats?.totalWords ?? 0}</div>
      </div>
      <div class="stat-card">
        <div class="stat-label">Words Today</div>
        <div class="stat-value">${stats?.wordsToday ?? 0}</div>
      </div>
    </div>
    <div class="card">
      <div class="stat-label">Average words per transcription</div>
      <div class="stat-value">${stats?.averageWordsPerTranscription?.toFixed(0) ?? 0}</div>
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
  if (
    // biome-ignore lint/suspicious/noAlert: confirm is acceptable for destructive actions
    !confirm(
      "Are you sure you want to delete all transcripts? This cannot be undone."
    )
  ) {
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

function renderHistory(
  el: HTMLElement,
  options: { restoreFocus?: boolean; skipLoad?: boolean } = {}
): void {
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
              <div class="list-item-text">${truncateText(item.text, 150)}</div>
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
      <div class="history-header">
        <h1>History</h1>
        ${hasTranscripts ? `<button class="btn btn-danger btn-sm clear-all-btn">Clear All</button>` : ""}
      </div>
      <div class="search-wrapper">
        ${createIcon(Search)}
        <input type="text" class="search-input" placeholder="Search transcripts..." value="${searchQuery}">
      </div>
      ${
        hasTranscripts
          ? `<div class="transcript-list">${listHtml}</div>
             ${hasMoreTranscripts ? `<button class="btn btn-secondary load-more-btn">Load more</button>` : ""}`
          : `
        <div class="empty-state">
          <div class="empty-state-title">${searchQuery ? "No results found" : "No transcripts yet"}</div>
          <p>${searchQuery ? "Try a different search term" : "Press and hold F8 to start recording"}</p>
        </div>
      `
      }
    `;

    const searchInput = el.querySelector(".search-input") as HTMLInputElement;
    if (restoreFocus && searchInput) {
      searchInput.focus();
      searchInput.setSelectionRange(searchQuery.length, searchQuery.length);
    }

    let searchTimeout: ReturnType<typeof setTimeout>;
    searchInput?.addEventListener("input", (e) => {
      const input = e.target as HTMLInputElement;
      searchQuery = input.value;
      transcriptOffset = 0;
      clearTimeout(searchTimeout);
      searchTimeout = setTimeout(() => {
        renderHistory(el, { restoreFocus: true });
      }, 300);
    });

    el.querySelector(".load-more-btn")?.addEventListener("click", async () => {
      transcriptOffset += PAGE_SIZE;
      await loadTranscripts(true);
      renderHistory(el, { skipLoad: true });
    });

    el.querySelector(".clear-all-btn")?.addEventListener(
      "click",
      handleClearAll
    );

    for (const btn of el.querySelectorAll(".delete-btn")) {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const listItem = (e.target as HTMLElement).closest(".list-item");
        const id = listItem?.getAttribute("data-id");
        if (id) {
          handleDeleteTranscript(Number(id));
        }
      });
    }

    for (const btn of el.querySelectorAll(".copy-btn")) {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const button = (e.target as HTMLElement).closest(
          ".copy-btn"
        ) as HTMLButtonElement;
        const listItem = button?.closest(".list-item");
        const id = listItem?.getAttribute("data-id");
        const transcript = transcripts.find((t) => t.id === Number(id));
        if (transcript && button) {
          copyToClipboard(transcript.text);
          const originalIcon = button.innerHTML;
          button.innerHTML = createIcon(Check);
          setTimeout(() => {
            button.innerHTML = originalIcon;
          }, 2000);
        }
      });
    }
  };

  if (skipLoad) {
    doRender();
  } else {
    loadTranscripts().then(doRender);
  }
}

async function loadSettings(): Promise<void> {
  try {
    const [loadedSettings, devices] = await Promise.all([
      getSettings(),
      getAudioDevices(),
    ]);
    settings = loadedSettings;
    audioDevices = devices;

    const autoStart = await getAutoStart().catch(() => null);
    if (autoStart !== null && settings) {
      settings = { ...settings, autoStart };
    }
  } catch {
    settings = null;
    audioDevices = [];
  }
}

function formatKeyForDisplay(key: string): string {
  // Convert key codes to display-friendly names
  const keyMap: Record<string, string> = {
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
  return keyMap[key] || key;
}

const FUNCTION_KEY_REGEX = /^F\d+$/;

function keyEventToHotkey(e: KeyboardEvent): string | null {
  // Get the base key
  let key = e.key;

  // Skip modifier-only presses
  if (["Control", "Alt", "Shift", "Meta"].includes(key)) {
    return null;
  }

  // Normalize function keys
  if (FUNCTION_KEY_REGEX.test(key)) {
    return key;
  }

  // Normalize other keys
  if (key === " ") {
    key = "Space";
  } else if (key === "Escape") {
    return null; // Escape cancels
  } else if (key.length === 1) {
    key = key.toUpperCase();
  }

  return key;
}

async function setHotkey(hotkey: string): Promise<boolean> {
  if (!settings) {
    return false;
  }

  try {
    const updated = { ...settings, hotkey };
    await updateSettings(updated);
    settings = updated;

    // Re-register the hotkey in backend
    await invoke("update_hotkey", { hotkey });
    return true;
  } catch (err) {
    console.error("Failed to update hotkey:", err);
    return false;
  }
}

function showHotkeyModal(): void {
  // Remove existing modal if any
  closeHotkeyModal();

  const modal = document.createElement("div");
  modal.className = "hotkey-modal-overlay";
  modal.innerHTML = `
    <div class="hotkey-modal">
      <button class="hotkey-modal-close">${createIcon(X)}</button>
      <div class="hotkey-modal-icon">${createIcon(Keyboard)}</div>
      <div class="hotkey-modal-title">Press the new hotkey</div>
      <div class="hotkey-modal-desc">Press any key to set as your recording hotkey</div>
      <div class="hotkey-modal-current">Current: <span class="hotkey-modal-key">${formatKeyForDisplay(settings?.hotkey ?? "F8")}</span></div>
      <button class="btn btn-outline hotkey-fn-btn" style="margin-top: 16px;">Use Fn key</button>
      <div class="hotkey-modal-hint">Press Escape to cancel</div>
    </div>
  `;

  document.body.appendChild(modal);

  // Handle Fn key button click
  const fnBtn = modal.querySelector(".hotkey-fn-btn");
  fnBtn?.addEventListener("click", async () => {
    if (await setHotkey("Fn")) {
      closeHotkeyModal();
      renderContent();
    } else {
      const desc = modal.querySelector(".hotkey-modal-desc");
      if (desc) {
        desc.textContent = "Failed to set hotkey. Try another key.";
        desc.classList.add("error");
      }
    }
  });

  const keyHandler = async (e: KeyboardEvent) => {
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

    if (await setHotkey(hotkey)) {
      closeHotkeyModal();
      renderContent();
    } else {
      const desc = modal.querySelector(".hotkey-modal-desc");
      if (desc) {
        desc.textContent = "Failed to set hotkey. Try another key.";
        desc.classList.add("error");
      }
    }
  };

  const clickHandler = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("hotkey-modal-overlay")) {
      closeHotkeyModal();
    }
  };

  const closeBtn = modal.querySelector(".hotkey-modal-close");
  closeBtn?.addEventListener("click", closeHotkeyModal);

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
  closeMicTestModal();

  let micTestInterval: number | null = null;
  let currentMicTest: MicrophoneTest | null = null;
  let audioDetected = false;
  let selectedDeviceId = settings?.selectedMicrophoneId ?? null;

  const modal = document.createElement("div");
  modal.className = "hotkey-modal-overlay";

  const micOptions = audioDevices
    .map(
      (d) =>
        `<option value="${d.id}" ${selectedDeviceId === d.id || (selectedDeviceId === null && d.isDefault) ? "selected" : ""}>${d.name}${d.isDefault ? " (Default)" : ""}</option>`
    )
    .join("");

  const renderModalContent = () => {
    const level = currentMicTest?.peakLevel ?? 0;
    const levelPercent = Math.min(level * 100, 100);

    modal.innerHTML = `
      <div class="hotkey-modal mic-test-modal">
        <button class="hotkey-modal-close">${createIcon(X)}</button>
        <div class="hotkey-modal-icon">${createIcon(Mic)}</div>
        <div class="hotkey-modal-title">Test Microphone</div>
        <div class="hotkey-modal-desc">Make sure your microphone is working</div>

        <div class="mic-test-container">
          <div class="mic-select-row">
            <label for="modal-mic-select">Device:</label>
            <select id="modal-mic-select" class="settings-select">
              <option value="" ${selectedDeviceId === null ? "selected" : ""}>System Default</option>
              ${micOptions}
            </select>
          </div>

          <div class="audio-level-container">
            <div class="audio-level-label">Audio Level</div>
            <div class="audio-level-bar">
              <div class="audio-level-fill ${levelPercent > 10 ? "active" : ""}" style="width: ${levelPercent}%"></div>
            </div>
          </div>

          <div class="mic-test-prompt ${audioDetected ? "success" : ""}">
            ${
              audioDetected
                ? `${createIcon(CheckCircle)} Audio detected! Your microphone is working.`
                : `${createIcon(Mic)} Say something to test...`
            }
          </div>
        </div>

        <div class="mic-test-actions">
          <button class="btn btn-primary mic-test-done-btn">Done</button>
        </div>
      </div>
    `;

    const closeBtn = modal.querySelector(".hotkey-modal-close");
    closeBtn?.addEventListener("click", closeMicTestModal);

    const doneBtn = modal.querySelector(".mic-test-done-btn");
    doneBtn?.addEventListener("click", closeMicTestModal);

    const micSelect = modal.querySelector(
      "#modal-mic-select"
    ) as HTMLSelectElement;
    micSelect?.addEventListener("change", async () => {
      selectedDeviceId = micSelect.value || null;
      audioDetected = false;
      try {
        await stopMicTest();
        await startMicTest(selectedDeviceId);
      } catch (err) {
        console.error("Failed to switch mic:", err);
      }
    });
  };

  const updateAudioLevel = () => {
    const level = currentMicTest?.peakLevel ?? 0;
    const levelPercent = Math.min(level * 100, 100);

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
      prompt.innerHTML = `${createIcon(CheckCircle)} Audio detected! Your microphone is working.`;
    }
  };

  renderModalContent();
  document.body.appendChild(modal);

  // Start mic test
  try {
    await startMicTest(selectedDeviceId);
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
  }, 100);

  const clickHandler = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("hotkey-modal-overlay")) {
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

function closeMicTestModal(): void {
  if (micTestModalCleanup) {
    micTestModalCleanup();
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
    await updateSettings(updated);
    settings = updated;
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
    await updateSettings(updated);
    settings = updated;
  } catch (e) {
    toggle.classList.toggle("active", previous);
    throw e;
  }
}

async function renderSettings(el: HTMLElement): Promise<void> {
  await loadSettings();

  const micOptions = audioDevices
    .map(
      (d) =>
        `<option value="${d.id}" ${settings?.selectedMicrophoneId === d.id ? "selected" : ""}>${d.name}${d.isDefault ? " (Default)" : ""}</option>`
    )
    .join("");

  el.innerHTML = `
    <h1>Settings</h1>
    <div class="settings-section">
      <div class="settings-section-title">Recording</div>
      <div class="settings-row">
        <div>
          <div class="settings-row-label">Hotkey</div>
          <div class="settings-row-desc">Press and hold to record</div>
        </div>
        <button class="btn btn-outline hotkey-btn">${formatKeyForDisplay(settings?.hotkey ?? "F8")}</button>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">Audio</div>
      <div class="settings-row">
        <div>
          <div class="settings-row-label">Microphone</div>
          <div class="settings-row-desc">Select audio input device</div>
        </div>
        <select class="settings-select mic-select">
          <option value="" ${settings?.selectedMicrophoneId ? "" : "selected"}>System Default</option>
          ${micOptions}
        </select>
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
        <button class="btn btn-outline mic-test-btn">Test</button>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">Permissions</div>
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
    <div class="settings-section">
      <div class="settings-section-title">Privacy</div>
      <div class="settings-row">
        <div>
          <div class="settings-row-label">Save transcript history</div>
          <div class="settings-row-desc">Store transcripts locally for search</div>
        </div>
        <div class="toggle ${settings?.historyEnabled ? "active" : ""}" data-setting="historyEnabled"></div>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">System</div>
      <div class="settings-row">
        <div>
          <div class="settings-row-label">Start on login</div>
          <div class="settings-row-desc">Launch Fing when you log in</div>
        </div>
        <div class="toggle ${settings?.autoStart ? "active" : ""}" data-setting="autoStart"></div>
      </div>
    </div>
    <div class="settings-section">
      <div class="settings-section-title">Setup</div>
      <div class="settings-row">
        <div>
          <div class="settings-row-label">Reset onboarding</div>
          <div class="settings-row-desc">Go through the setup process again</div>
        </div>
        <button class="btn btn-outline reset-onboarding-btn">Reset</button>
      </div>
    </div>
  `;

  for (const toggle of el.querySelectorAll(".toggle")) {
    toggle.addEventListener("click", () => {
      const setting = toggle.getAttribute("data-setting") as keyof SettingsType;
      if (!(setting && settings)) {
        return;
      }

      const newValue = !settings[setting];
      if (setting === "autoStart") {
        handleAutoStartToggle(toggle as HTMLElement, newValue as boolean).catch(
          (err) => {
            console.error("Failed to update auto-start:", err);
          }
        );
        return;
      }

      toggle.classList.toggle("active", newValue as boolean);
      handleSettingChange(setting, newValue);
    });
  }

  const micSelect = el.querySelector(".mic-select") as HTMLSelectElement;
  micSelect?.addEventListener("change", () => {
    const value = micSelect.value || null;
    handleSettingChange("selectedMicrophoneId", value);
  });

  const hotkeyBtn = el.querySelector(".hotkey-btn");
  hotkeyBtn?.addEventListener("click", showHotkeyModal);

  const micTestBtn = el.querySelector(".mic-test-btn");
  micTestBtn?.addEventListener("click", showMicTestModal);

  const resetBtn = el.querySelector(".reset-onboarding-btn");
  resetBtn?.addEventListener("click", async () => {
    if (!settings) {
      return;
    }
    await updateSettings({ ...settings, onboardingCompleted: false });
    sessionStorage.setItem("onboarding-reset", "true");
    window.location.reload();
  });

  updatePermissionStatus();
}

async function updatePermissionStatus(): Promise<void> {
  const micBadge = document.querySelector(
    '[data-permission="microphone"]'
  ) as HTMLElement;
  const accBadge = document.querySelector(
    '[data-permission="accessibility"]'
  ) as HTMLElement;

  if (!(micBadge && accBadge)) {
    return;
  }

  try {
    const status = await requestPermissions();

    updateBadge(micBadge, status.microphone, "microphone");
    updateBadge(accBadge, status.accessibility, "accessibility");
  } catch (e) {
    console.error("Failed to check permissions:", e);
    micBadge.textContent = "Error";
    accBadge.textContent = "Error";
  }
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
        Version ${info?.version ?? "0.1.0"}<br/>
        Commit: ${info?.commit ?? "dev"}<br/>
        Built: ${info?.buildDate ?? "unknown"}
      </div>
      <p class="about-tagline">Fast, private, local speech-to-text</p>
      <div class="about-backend">Backend: ${info?.inferenceBackend ?? "Unknown"}</div>
      <br/><br/>
      <a href="${info?.repository ?? "#"}" target="_blank" class="btn btn-outline">GitHub ${createIcon(ArrowUpRight)}</a>
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

function showMainUI(): void {
  const app = document.getElementById("app");
  if (!app) {
    return;
  }

  app.innerHTML = `
    <div class="titlebar"></div>
    <div class="app-body">
      <aside id="sidebar" class="sidebar"></aside>
      <main id="content" class="content"></main>
    </div>
  `;

  setupTitlebarDrag();
  renderSidebar();
  renderContent();
}

async function init(): Promise<void> {
  try {
    currentAppState = await getAppState();
    appInfo = await getAppInfo();
    stats = await getStats().catch(() => null);
  } catch {
    // Commands may not be registered yet
  }

  if (currentAppState === "needs-setup") {
    await showOnboarding();
  } else {
    showMainUI();
  }

  window.addEventListener("setup-complete", () => {
    cleanupOnboarding();
    currentAppState = "ready";
    showMainUI();
  });

  listen("app-state-changed", (event) => {
    const newState = event.payload as AppState;
    if (newState !== "needs-setup" && currentAppState === "needs-setup") {
      cleanupOnboarding();
      showMainUI();
    }
    currentAppState = newState;
    renderContent();
  });

  listen("transcript-added", () => {
    getStats()
      .then((s) => {
        stats = s;
      })
      .catch(() => {
        // Ignore stats fetch errors
      });
    renderContent();
  });

  // Listen for navigation events from tray menu
  listen<string>("navigate-to-tab", (event) => {
    const tab = event.payload as SidebarItem;
    if (["home", "history", "settings", "about"].includes(tab)) {
      // Clear content immediately to prevent flash of old view
      const content = document.getElementById("content");
      if (content) {
        content.innerHTML = "";
      }
      currentView = tab;
      renderSidebar();
      renderContent();
    }
  });
}

init();
