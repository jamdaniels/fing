import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Copy,
  History,
  Home,
  type IconNode,
  Info,
  Power,
  Search,
  Settings,
  Trash2,
} from "lucide";
import { cleanupOnboarding, renderOnboarding } from "./components/onboarding";
import { createIcon } from "./lib/icons";
import {
  deleteAllTranscripts,
  deleteTranscript,
  getAppInfo,
  getAppState,
  getAudioDevices,
  getRecentTranscripts,
  getSettings,
  getStats,
  searchTranscripts,
  updateSettings,
} from "./lib/ipc";
import type {
  AppInfo,
  AppState,
  AudioDevice,
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
let settings: SettingsType | null = null;
let audioDevices: AudioDevice[] = [];

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
      <div class="sidebar-item ${currentView === item.id ? "active" : ""}" data-view="${item.id}">
        ${createIcon(item.icon)}
        <span>${item.label}</span>
      </div>
    `
      )
      .join("")}
    <div class="sidebar-spacer"></div>
    <div class="sidebar-divider"></div>
    <div class="sidebar-item" data-action="quit">
      ${createIcon(Power)}
      <span>Quit</span>
    </div>
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

async function loadTranscripts(): Promise<void> {
  try {
    if (searchQuery.trim()) {
      transcripts = await searchTranscripts(searchQuery);
    } else {
      transcripts = await getRecentTranscripts(50);
    }
  } catch {
    transcripts = [];
  }
}

async function handleDeleteTranscript(id: number): Promise<void> {
  try {
    await deleteTranscript(id);
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

function renderHistory(el: HTMLElement): void {
  // biome-ignore lint/complexity/noExcessiveCognitiveComplexity: UI rendering with necessary loops
  loadTranscripts().then(() => {
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
          ? `<div class="transcript-list">${listHtml}</div>`
          : `
        <div class="empty-state">
          <div class="empty-state-title">${searchQuery ? "No results found" : "No transcripts yet"}</div>
          <p>${searchQuery ? "Try a different search term" : "Press and hold F8 to start recording"}</p>
        </div>
      `
      }
    `;

    const searchInput = el.querySelector(".search-input") as HTMLInputElement;
    let searchTimeout: ReturnType<typeof setTimeout>;
    searchInput?.addEventListener("input", (e) => {
      clearTimeout(searchTimeout);
      searchTimeout = setTimeout(() => {
        searchQuery = (e.target as HTMLInputElement).value;
        renderHistory(el);
      }, 300);
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
        const listItem = (e.target as HTMLElement).closest(".list-item");
        const id = listItem?.getAttribute("data-id");
        const transcript = transcripts.find((t) => t.id === Number(id));
        if (transcript) {
          copyToClipboard(transcript.text);
        }
      });
    }
  });
}

async function loadSettings(): Promise<void> {
  try {
    settings = await getSettings();
    audioDevices = await getAudioDevices();
  } catch {
    settings = null;
    audioDevices = [];
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

function renderSettings(el: HTMLElement): void {
  loadSettings().then(() => {
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
          <button class="btn btn-outline">${settings?.hotkey ?? "F8"}</button>
        </div>
      </div>
      <div class="settings-section">
        <div class="settings-section-title">Audio</div>
        <div class="settings-row">
          <div>
            <div class="settings-row-label">Microphone</div>
            <div class="settings-row-desc">Select audio input device</div>
          </div>
          <select class="btn btn-outline mic-select">
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
    `;

    for (const toggle of el.querySelectorAll(".toggle")) {
      toggle.addEventListener("click", () => {
        const setting = toggle.getAttribute(
          "data-setting"
        ) as keyof SettingsType;
        if (setting && settings) {
          const newValue = !settings[setting];
          toggle.classList.toggle("active", newValue as boolean);
          handleSettingChange(setting, newValue);
        }
      });
    }

    const micSelect = el.querySelector(".mic-select") as HTMLSelectElement;
    micSelect?.addEventListener("change", () => {
      const value = micSelect.value || null;
      handleSettingChange("selectedMicrophoneId", value);
    });
  });
}

function renderAbout(el: HTMLElement): void {
  const info = appInfo;
  el.innerHTML = `
    <div class="about-center">
      <svg class="about-icon" xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M2 10v3"/>
        <path d="M6 6v11"/>
        <path d="M10 3v18"/>
        <path d="M14 8v7"/>
        <path d="M18 5v13"/>
        <path d="M22 10v3"/>
      </svg>
      <h1>Fing</h1>
      <div class="about-version">
        Version ${info?.version ?? "0.1.0"}<br/>
        Commit: ${info?.commit ?? "dev"}<br/>
        Built: ${info?.buildDate ?? "unknown"}
      </div>
      <p class="about-tagline">Fast, private, local speech-to-text</p>
      <div class="about-backend">Backend: ${info?.inferenceBackend ?? "Unknown"}</div>
      <br/><br/>
      <a href="${info?.repository ?? "#"}" target="_blank" class="btn btn-outline">View on GitHub</a>
    </div>
  `;
}

function showOnboarding(): void {
  const app = document.getElementById("app");
  if (!app) {
    return;
  }

  app.innerHTML = `<div id="onboarding-container"></div>`;
  const container = document.getElementById("onboarding-container");
  if (container) {
    renderOnboarding(container);
  }
}

function showMainUI(): void {
  const app = document.getElementById("app");
  if (!app) {
    return;
  }

  app.innerHTML = `
    <aside id="sidebar" class="sidebar"></aside>
    <main id="content" class="content"></main>
  `;

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
    showOnboarding();
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
      currentView = tab;
      renderSidebar();
      renderContent();
    }
  });
}

init();
