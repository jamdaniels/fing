export type AppState =
  | "needs-setup"
  | "initializing"
  | "ready"
  | "recording"
  | "processing";

export type Theme = "system" | "light" | "dark";

export type ModelVariant = "small_q5" | "small" | "large_turbo_q5";

export interface ModelInfo {
  description: string;
  displayName: string;
  filename: string;
  isActive: boolean;
  isDownloaded: boolean;
  memoryEstimateMb: number;
  sizeBytes: number;
  variant: ModelVariant;
}

export interface Transcript {
  appContext: string | null;
  createdAt: string;
  durationMs: number;
  id: number;
  text: string;
  wordCount: number;
}

export interface AudioDevice {
  id: string;
  isDefault: boolean;
  name: string;
}

export type HistoryMode = "off" | "30d";

export interface Settings {
  activeModelVariant: ModelVariant;
  autoStart: boolean;
  dictionaryTerms: string[];
  historyMode: HistoryMode;
  hotkey: string;
  languages: string[];
  lazyModelLoading: boolean;
  modelPath: string;
  onboardingCompleted: boolean;
  onboardingStep: number | null;
  pasteEnabled: boolean;
  selectedMicrophoneId: string | null;
  soundEnabled: boolean;
  theme: Theme;
}

export interface AppInfo {
  buildDate: string;
  commit: string;
  inferenceBackend: "Metal" | "Vulkan" | "CPU";
  name: string;
  repository: string;
  version: string;
}

export interface UpdateStatus {
  checking: boolean;
  updateAvailable: boolean;
}

export interface UpdateCheckResult {
  availableBody: string | null;
  availableVersion: string | null;
  updateAvailable: boolean;
}

export interface WordCount {
  count: number;
  word: string;
}

export interface Stats {
  averageWordsPerTranscription: number;
  averageWpm: number;
  topWords: WordCount[];
  totalTranscriptions: number;
  totalWords: number;
  transcriptionsToday: number;
  wordsToday: number;
}

export type SidebarItem =
  | "home"
  | "history"
  | "dictionary"
  | "settings"
  | "about";

export interface DownloadProgress {
  bytesDownloaded: number;
  errorMessage?: string;
  percentage: number;
  status: "not-started" | "downloading" | "verifying" | "complete" | "failed";
  totalBytes: number;
  variant: ModelVariant | null;
}

export interface ModelVerification {
  exists: boolean;
  formatValid: boolean;
  hashValid: boolean;
  isValid: boolean;
  path: string;
  sizeValid: boolean;
}

export interface MicrophoneTest {
  deviceName: string;
  isReceivingAudio: boolean;
  peakLevel: number;
}

export interface MicTestStartResult {
  actualDevice: string;
  deviceMatched: boolean;
  requestedDevice: string | null;
}

export interface PermissionStatus {
  accessibility: "unknown" | "granted" | "denied" | "not-applicable";
  microphone: "unknown" | "prompt" | "granted" | "denied";
}

export interface HotkeyRegistrationResult {
  error?: "conflict-system" | "conflict-app" | "permission-denied" | "unknown";
  message?: string;
  success: boolean;
}
