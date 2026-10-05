// Microphone picker helpers (no DOM). Matching mirrors src-tauri/src/microphone.rs.

import { t } from "./i18n";
import type { AudioDevice, MicrophoneRef, Settings } from "./types";

type MicrophoneSettings = Pick<
  Settings,
  | "preferredMicrophoneId"
  | "preferredMicrophoneName"
  | "selectedMicrophoneId"
  | "selectedMicrophoneName"
>;

/** A remembered microphone: exact ID first, then name, then legacy IDs. */
export function findAudioDevice(
  devices: AudioDevice[],
  id: string | null | undefined,
  name?: string | null
): AudioDevice | null {
  if (!id) {
    return null;
  }
  const legacyId = id.trim().toLowerCase();
  return (
    devices.find((device) => device.id === id) ??
    (name ? devices.find((device) => device.name === name) : undefined) ??
    devices.find(
      (device) =>
        device.legacyId.trim().toLowerCase() === legacyId ||
        device.name.trim().toLowerCase() === legacyId
    ) ??
    null
  );
}

export interface MicrophonePicker {
  /** Connected device chosen in the dropdown; null for system default or missing. */
  device: AudioDevice | null;
  /** The chosen device is not connected (shown with its remembered name). */
  missing: MicrophoneRef | null;
  /** The dropdown shows the preferred (hearted) microphone. */
  isPreferred: boolean;
  /** Connected preferred (hearted) microphone. */
  preferred: AudioDevice | null;
  /** Name of the preferred microphone, if one is set. */
  preferredName: string | null;
  systemDefault: AudioDevice | null;
}

export function microphonePicker(
  devices: AudioDevice[],
  settings: MicrophoneSettings | null
): MicrophonePicker {
  const selectedId = settings?.selectedMicrophoneId ?? null;
  const selectedName = settings?.selectedMicrophoneName ?? null;
  const preferredId = settings?.preferredMicrophoneId ?? null;
  const preferredName = settings?.preferredMicrophoneName ?? null;

  const device = findAudioDevice(devices, selectedId, selectedName);
  const preferred = findAudioDevice(devices, preferredId, preferredName);
  const missing =
    selectedId && !device ? { id: selectedId, name: selectedName } : null;

  return {
    device,
    missing,
    isPreferred:
      preferredId !== null &&
      (device
        ? device === preferred
        : missing !== null && missing.id === preferredId),
    preferred,
    preferredName: preferredId ? (preferred?.name ?? preferredName) : null,
    systemDefault: devices.find((d) => d.isDefault) ?? null,
  };
}

/** Device a recording would use right now; null if there is no input. */
export function activeMicrophone(
  devices: AudioDevice[],
  settings: MicrophoneSettings | null
): AudioDevice | null {
  const picker = microphonePicker(devices, settings);
  // A missing pick hands over to the connected preferred mic.
  return (
    picker.device ??
    (picker.missing ? picker.preferred : null) ??
    picker.systemDefault
  );
}

/** Label of the "System default" dropdown entry, naming the current default. */
export function systemDefaultMicrophoneLabel(devices: AudioDevice[]): string {
  const systemDefault = devices.find((d) => d.isDefault);
  return systemDefault
    ? t("settings.systemDefaultMicrophone", { device: systemDefault.name })
    : t("settings.systemDefaultMicrophoneUnknown");
}
