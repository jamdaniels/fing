import { describe, expect, it } from "bun:test";

import {
  activeMicrophone,
  findAudioDevice,
  microphonePicker,
} from "./microphone";
import type { AudioDevice } from "./types";

const builtin: AudioDevice = {
  id: "builtin",
  name: "MacBook Pro Microphone",
  isDefault: true,
  legacyId: "MacBook Pro Microphone",
};
const hyperx: AudioDevice = {
  id: "hyperx",
  name: "HyperX QuadCast",
  isDefault: false,
  legacyId: "HyperX QuadCast",
};
const devices = [builtin, hyperx];

const noMicrophone = {
  preferredMicrophoneId: null,
  preferredMicrophoneName: null,
  selectedMicrophoneId: null,
  selectedMicrophoneName: null,
};

describe("findAudioDevice", () => {
  it("matches by id, then name, then legacy id, never partially", () => {
    expect(findAudioDevice(devices, "hyperx")).toBe(hyperx);
    expect(findAudioDevice(devices, "other-port", "HyperX QuadCast")).toBe(
      hyperx
    );
    expect(findAudioDevice(devices, "hyperx quadcast")).toBe(hyperx);
    expect(findAudioDevice(devices, "HyperX")).toBeNull();
    expect(findAudioDevice(devices, null)).toBeNull();
  });
});

describe("microphonePicker", () => {
  it("shows the system default without a selection", () => {
    const picker = microphonePicker(devices, noMicrophone);
    expect(picker.device).toBeNull();
    expect(picker.missing).toBeNull();
    expect(picker.isPreferred).toBe(false);
    expect(picker.systemDefault).toBe(builtin);
  });

  it("marks the preferred microphone, also while unplugged", () => {
    const settings = {
      preferredMicrophoneId: "hyperx",
      preferredMicrophoneName: "HyperX QuadCast",
      selectedMicrophoneId: "hyperx",
      selectedMicrophoneName: "HyperX QuadCast",
    };
    expect(microphonePicker(devices, settings).isPreferred).toBe(true);

    const unplugged = microphonePicker([builtin], settings);
    expect(unplugged.isPreferred).toBe(true);
    expect(unplugged.missing).toEqual({
      id: "hyperx",
      name: "HyperX QuadCast",
    });
    expect(activeMicrophone([builtin], settings)).toBe(builtin);
  });

  it("uses the connected preferred microphone when the pick is missing", () => {
    const settings = {
      preferredMicrophoneId: "hyperx",
      preferredMicrophoneName: "HyperX QuadCast",
      selectedMicrophoneId: "usb",
      selectedMicrophoneName: "USB Microphone",
    };
    expect(activeMicrophone(devices, settings)).toBe(hyperx);
  });

  it("names the preferred microphone while another one is picked", () => {
    const picker = microphonePicker(devices, {
      preferredMicrophoneId: "hyperx",
      preferredMicrophoneName: "HyperX QuadCast",
      selectedMicrophoneId: "builtin",
      selectedMicrophoneName: "MacBook Pro Microphone",
    });
    expect(picker.device).toBe(builtin);
    expect(picker.isPreferred).toBe(false);
    expect(picker.preferredName).toBe("HyperX QuadCast");
  });
});
