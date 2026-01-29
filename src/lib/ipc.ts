import { invoke } from "@tauri-apps/api/core";
import type {
  AppInfo,
  AppState,
  AudioDevice,
  DownloadProgress,
  MicrophoneTest,
  MicTestStartResult,
  ModelVerification,
  PermissionStatus,
  Settings,
  Stats,
  Transcript,
  UpdateInfo,
} from "./types";

export async function getAppState(): Promise<AppState> {
  return (await invoke<string>("get_app_state")) as AppState;
}

export async function getAppInfo(): Promise<AppInfo> {
  return await invoke<AppInfo>("get_app_info");
}

export async function getSettings(): Promise<Settings> {
  return await invoke<Settings>("get_settings");
}

export async function updateSettings(settings: Settings): Promise<void> {
  return await invoke("update_settings", { settings });
}

export async function getStats(): Promise<Stats> {
  return await invoke<Stats>("get_stats");
}

export async function getRecentTranscripts(
  limit: number,
  offset = 0
): Promise<Transcript[]> {
  return await invoke<Transcript[]>("db_get_recent", { limit, offset });
}

export async function searchTranscripts(
  query: string,
  limit = 50,
  offset = 0
): Promise<Transcript[]> {
  return await invoke<Transcript[]>("db_search", { query, limit, offset });
}

export async function deleteTranscript(id: number): Promise<void> {
  return await invoke("db_delete", { id });
}

export async function deleteAllTranscripts(): Promise<number> {
  return await invoke<number>("db_delete_all");
}

export async function getAudioDevices(): Promise<AudioDevice[]> {
  return await invoke<AudioDevice[]>("get_audio_devices");
}

export async function refreshAudioDevices(): Promise<AudioDevice[]> {
  return await invoke<AudioDevice[]>("refresh_audio_devices");
}

export async function setAudioDevice(deviceId: string | null): Promise<void> {
  return await invoke("set_audio_device", { deviceId });
}

export async function checkForUpdates(): Promise<UpdateInfo> {
  return await invoke<UpdateInfo>("check_for_updates");
}

export async function startModelDownload(): Promise<void> {
  return await invoke("start_model_download");
}

export async function getDownloadProgress(): Promise<DownloadProgress> {
  return await invoke<DownloadProgress>("get_download_progress");
}

export async function selectModelFile(): Promise<string | null> {
  return await invoke<string | null>("select_model_file");
}

export async function verifyModel(path: string): Promise<ModelVerification> {
  return await invoke<ModelVerification>("verify_model", { path });
}

export async function checkModelExists(): Promise<ModelVerification> {
  return await invoke<ModelVerification>("check_model_exists");
}

export async function completeSetup(): Promise<void> {
  return await invoke("complete_setup");
}

export async function testMicrophone(
  deviceId?: string | null
): Promise<MicrophoneTest> {
  return await invoke<MicrophoneTest>("test_microphone", {
    deviceId: deviceId ?? null,
  });
}

export async function startMicTest(
  deviceId?: string | null
): Promise<MicTestStartResult> {
  // Tauri 2 converts camelCase to snake_case for Rust parameters
  return await invoke<MicTestStartResult>("start_mic_test", {
    deviceId: deviceId ?? null,
  });
}

export async function getMicTestLevel(): Promise<MicrophoneTest> {
  return await invoke<MicrophoneTest>("get_mic_test_level");
}

export async function stopMicTest(): Promise<void> {
  return await invoke("stop_mic_test");
}

export async function requestPermissions(): Promise<PermissionStatus> {
  return await invoke<PermissionStatus>("request_permissions");
}

export async function checkAccessibilityPermission(): Promise<boolean> {
  return await invoke<boolean>("check_accessibility_permission");
}

export async function requestAccessibilityPermission(): Promise<boolean> {
  return await invoke<boolean>("request_accessibility_permission");
}

export async function requestMicrophonePermission(): Promise<void> {
  return await invoke("request_microphone_permission");
}

export async function openMainWindow(tab?: string): Promise<void> {
  return await invoke("open_main_window", { tab });
}

export async function quitApp(): Promise<void> {
  return await invoke("quit_app");
}

export async function setAutoStart(enabled: boolean): Promise<void> {
  return await invoke("set_auto_start", { enabled });
}

export async function getAutoStart(): Promise<boolean> {
  return await invoke<boolean>("get_auto_start");
}

export async function relaunchApp(): Promise<void> {
  const { relaunch } = await import("@tauri-apps/plugin-process");
  await relaunch();
}

export async function enableOnboardingTestMode(): Promise<void> {
  return await invoke("enable_onboarding_test_mode");
}

export async function disableOnboardingTestMode(): Promise<void> {
  return await invoke("disable_onboarding_test_mode");
}
