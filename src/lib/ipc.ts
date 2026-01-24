import { invoke } from "@tauri-apps/api/core";
import type {
  AppInfo,
  AppState,
  AudioDevice,
  DownloadProgress,
  MicrophoneTest,
  ModelVerification,
  PermissionStatus,
  Settings,
  Stats,
  Transcript,
  UpdateInfo,
} from "./types";

export async function getAppState(): Promise<AppState> {
  return invoke<string>("get_app_state") as Promise<AppState>;
}

export async function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("get_app_info");
}

export async function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

export async function updateSettings(settings: Settings): Promise<void> {
  return invoke("update_settings", { settings });
}

export async function getStats(): Promise<Stats> {
  return invoke<Stats>("get_stats");
}

export async function getRecentTranscripts(limit: number, offset = 0): Promise<Transcript[]> {
  return invoke<Transcript[]>("db_get_recent", { limit, offset });
}

export async function searchTranscripts(query: string, limit = 50): Promise<Transcript[]> {
  return invoke<Transcript[]>("db_search", { query, limit });
}

export async function deleteTranscript(id: number): Promise<void> {
  return invoke("db_delete", { id });
}

export async function deleteAllTranscripts(): Promise<number> {
  return invoke<number>("db_delete_all");
}

export async function getAudioDevices(): Promise<AudioDevice[]> {
  return invoke<AudioDevice[]>("get_audio_devices");
}

export async function setAudioDevice(deviceId: string | null): Promise<void> {
  return invoke("set_audio_device", { deviceId });
}

export async function checkForUpdates(): Promise<UpdateInfo> {
  return invoke<UpdateInfo>("check_for_updates");
}

export async function startModelDownload(): Promise<void> {
  return invoke("start_model_download");
}

export async function getDownloadProgress(): Promise<DownloadProgress> {
  return invoke<DownloadProgress>("get_download_progress");
}

export async function selectModelFile(): Promise<string | null> {
  return invoke<string | null>("select_model_file");
}

export async function verifyModel(path: string): Promise<ModelVerification> {
  return invoke<ModelVerification>("verify_model", { path });
}

export async function completeSetup(): Promise<void> {
  return invoke("complete_setup");
}

export async function testMicrophone(): Promise<MicrophoneTest> {
  return invoke<MicrophoneTest>("test_microphone");
}

export async function requestPermissions(): Promise<PermissionStatus> {
  return invoke<PermissionStatus>("request_permissions");
}

export async function openMainWindow(tab?: string): Promise<void> {
  return invoke("open_main_window", { tab });
}

export async function quitApp(): Promise<void> {
  return invoke("quit_app");
}
