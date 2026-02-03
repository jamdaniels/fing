export type AppState =
  | "needs-setup"
  | "initializing"
  | "ready"
  | "recording"
  | "processing";

export type Theme = "system" | "light" | "dark";

export type ModelVariant = "small_q5" | "small" | "large_turbo_q5";

export interface ModelInfo {
  variant: ModelVariant;
  filename: string;
  displayName: string;
  description: string;
  sizeBytes: number;
  memoryEstimateMb: number;
  isDownloaded: boolean;
  isActive: boolean;
}

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

export type HistoryMode = "off" | "30d";

export interface Settings {
  hotkey: string;
  modelPath: string;
  selectedMicrophoneId: string | null;
  autoStart: boolean;
  soundEnabled: boolean;
  pasteEnabled: boolean;
  historyMode: HistoryMode;
  onboardingCompleted: boolean;
  languages: string[];
  onboardingStep: number | null;
  activeModelVariant: ModelVariant;
  theme: Theme;
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

export interface WordCount {
  word: string;
  count: number;
}

export interface Stats {
  totalTranscriptions: number;
  totalWords: number;
  transcriptionsToday: number;
  wordsToday: number;
  averageWordsPerTranscription: number;
  averageWpm: number;
  topWords: WordCount[];
}

export type SidebarItem = "home" | "history" | "settings" | "about";

export interface DownloadProgress {
  variant: ModelVariant | null;
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
  formatValid: boolean;
  isValid: boolean;
}

export interface MicrophoneTest {
  deviceName: string;
  peakLevel: number;
  isReceivingAudio: boolean;
}

export interface MicTestStartResult {
  requestedDevice: string | null;
  actualDevice: string;
  deviceMatched: boolean;
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
