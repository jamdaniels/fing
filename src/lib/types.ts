export type AppState =
  | "needs-setup"
  | "initializing"
  | "ready"
  | "recording"
  | "processing";

export interface Transcript {
  id: number;
  text: string;
  createdAt: string;
  durationMs: number;
  appContext: string | null;
  wordCount: number;
}

export interface AudioDevice {
  id: string;
  name: string;
  isDefault: boolean;
}

export type HistoryLimit = 100 | 500 | 1000 | 5000 | -1;

export interface Settings {
  hotkey: string;
  modelPath: string;
  selectedMicrophoneId: string | null;
  autoStart: boolean;
  soundEnabled: boolean;
  pasteEnabled: boolean;
  historyEnabled: boolean;
  historyLimit: HistoryLimit;
  onboardingCompleted: boolean;
}

export interface UpdateInfo {
  currentVersion: string;
  latestVersion: string;
  updateAvailable: boolean;
  downloadUrl?: string;
  releaseNotes?: string;
}

export interface AppInfo {
  name: string;
  version: string;
  commit: string;
  buildDate: string;
  repository: string;
  inferenceBackend: "Metal" | "Vulkan" | "CPU";
}

export interface Stats {
  totalTranscriptions: number;
  totalWords: number;
  transcriptionsToday: number;
  wordsToday: number;
  averageWordsPerTranscription: number;
}

export type SidebarItem = "home" | "history" | "settings" | "about";

export interface DownloadProgress {
  bytesDownloaded: number;
  totalBytes: number;
  percentage: number;
  status: "not-started" | "downloading" | "verifying" | "complete" | "failed";
  errorMessage?: string;
}

export interface ModelVerification {
  path: string;
  exists: boolean;
  sizeValid: boolean;
  hashValid: boolean;
  isValid: boolean;
}

export interface MicrophoneTest {
  deviceName: string;
  peakLevel: number;
  isReceivingAudio: boolean;
}

export interface PermissionStatus {
  microphone: "unknown" | "granted" | "denied";
  accessibility: "unknown" | "granted" | "denied" | "not-applicable";
}

export interface HotkeyRegistrationResult {
  success: boolean;
  error?: "conflict-system" | "conflict-app" | "permission-denied" | "unknown";
  message?: string;
}
