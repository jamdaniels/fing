import { describe, expect, it } from "bun:test";
import {
  eventToHotkeyToken,
  formatHotkeyForDisplay,
  matchesHotkey,
  matchesHotkeyRelease,
  normalizeStoredHotkey,
  parseHotkeyString,
} from "./hotkey";

interface KeyboardEventShape {
  code: string;
  key: string;
  repeat: boolean;
}

function createKeyboardEvent(
  overrides: Partial<KeyboardEventShape> & Pick<KeyboardEventShape, "key">
): KeyboardEvent {
  return {
    code: overrides.code ?? "",
    key: overrides.key,
    repeat: overrides.repeat ?? false,
  } as KeyboardEvent;
}

describe("parseHotkeyString", () => {
  it("accepts canonical physical key tokens", () => {
    expect(parseHotkeyString("F9")?.keys).toEqual(["F9"]);
    expect(parseHotkeyString("Space")?.keys).toEqual(["Space"]);
    expect(parseHotkeyString("ControlLeft")?.keys).toEqual(["ControlLeft"]);
    expect(parseHotkeyString("ControlLeft+KeyK")?.keys).toEqual([
      "ControlLeft",
      "KeyK",
    ]);
    expect(parseHotkeyString("KeyA+KeyS")?.keys).toEqual(["KeyA", "KeyS"]);
  });

  it("rejects old-format, duplicate, empty, and Escape hotkeys", () => {
    expect(parseHotkeyString("Ctrl+Option")).toBeNull();
    expect(parseHotkeyString("Cmd+Space")).toBeNull();
    expect(parseHotkeyString("A")).toBeNull();
    expect(parseHotkeyString("Fn")).toBeNull();
    expect(parseHotkeyString("Escape")).toBeNull();
    expect(parseHotkeyString("ControlLeft+Escape")).toBeNull();
    expect(parseHotkeyString("KeyA+KeyA")).toBeNull();
    expect(parseHotkeyString("")).toBeNull();
  });
});

describe("eventToHotkeyToken", () => {
  it("normalizes representative physical key events", () => {
    expect(
      eventToHotkeyToken(createKeyboardEvent({ code: "KeyA", key: "a" }))
    ).toBe("KeyA");
    expect(
      eventToHotkeyToken(createKeyboardEvent({ code: "Digit1", key: "1" }))
    ).toBe("Num1");
    expect(
      eventToHotkeyToken(
        createKeyboardEvent({ code: "ControlLeft", key: "Control" })
      )
    ).toBe("ControlLeft");
    expect(
      eventToHotkeyToken(createKeyboardEvent({ code: "AltRight", key: "Alt" }))
    ).toBe("AltGr");
    expect(
      eventToHotkeyToken(createKeyboardEvent({ code: "Space", key: " " }))
    ).toBe("Space");
  });

  it("excludes Escape from capture", () => {
    expect(
      eventToHotkeyToken(createKeyboardEvent({ code: "Escape", key: "Escape" }))
    ).toBeNull();
  });
});

describe("hotkey matching", () => {
  it("matches exact arbitrary key sets", () => {
    const config = parseHotkeyString("ControlLeft+KeyK");
    if (!config) {
      throw new Error("Expected ControlLeft+KeyK to parse");
    }

    expect(matchesHotkey(new Set(["ControlLeft", "KeyK"]), config)).toBe(true);
    expect(matchesHotkey(new Set(["ControlLeft"]), config)).toBe(false);
    expect(
      matchesHotkey(new Set(["ControlLeft", "KeyK", "Space"]), config)
    ).toBe(false);
  });

  it("releases when any configured key is released", () => {
    const config = parseHotkeyString("KeyA+KeyS");
    if (!config) {
      throw new Error("Expected KeyA+KeyS to parse");
    }

    expect(
      matchesHotkeyRelease(
        createKeyboardEvent({ code: "KeyA", key: "a" }),
        config
      )
    ).toBe(true);
    expect(
      matchesHotkeyRelease(
        createKeyboardEvent({ code: "Space", key: " " }),
        config
      )
    ).toBe(false);
  });
});

describe("formatHotkeyForDisplay", () => {
  it("formats canonical tokens for display", () => {
    expect(formatHotkeyForDisplay("ControlLeft+KeyK")).toBe("Ctrl Left + K");
    expect(formatHotkeyForDisplay("KeyA+KeyS")).toBe("A + S");
    expect(formatHotkeyForDisplay("AltGr+Space")).toBe("Option Right + Space");
    expect(formatHotkeyForDisplay("Kp1+KpPlus")).toBe("Numpad 1 + Numpad +");
  });
});

describe("normalizeStoredHotkey", () => {
  it("preserves valid values and falls back for invalid ones", () => {
    expect(normalizeStoredHotkey("ControlLeft+KeyK")).toBe("ControlLeft+KeyK");
    expect(normalizeStoredHotkey("Cmd+Space")).toBe("F9");
    expect(normalizeStoredHotkey(null)).toBe("F9");
  });
});
