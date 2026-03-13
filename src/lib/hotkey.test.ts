import { describe, expect, it } from "bun:test";
import {
  keyEventToHotkey,
  matchesHotkey,
  matchesHotkeyRelease,
  normalizeStoredHotkey,
  parseHotkeyString,
  shouldReleaseOnKeydown,
} from "./hotkey";

interface KeyboardEventShape {
  altKey: boolean;
  code: string;
  ctrlKey: boolean;
  key: string;
  metaKey: boolean;
  shiftKey: boolean;
}

function createKeyboardEvent(
  overrides: Partial<KeyboardEventShape> & Pick<KeyboardEventShape, "key">
): KeyboardEvent {
  return {
    altKey: overrides.altKey ?? false,
    code: overrides.code ?? "",
    ctrlKey: overrides.ctrlKey ?? false,
    key: overrides.key,
    metaKey: overrides.metaKey ?? false,
    shiftKey: overrides.shiftKey ?? false,
  } as KeyboardEvent;
}

describe("parseHotkeyString", () => {
  it("accepts the supported hotkey formats", () => {
    expect(parseHotkeyString("F9")).toEqual({
      alt: false,
      ctrl: false,
      key: "F9",
      meta: false,
      shift: false,
    });
    expect(parseHotkeyString("Ctrl+Option")).toEqual({
      alt: true,
      ctrl: true,
      key: null,
      meta: false,
      shift: false,
    });
    expect(parseHotkeyString("Cmd+Space")).toEqual({
      alt: false,
      ctrl: false,
      key: "Space",
      meta: true,
      shift: false,
    });
  });

  it("rejects unsupported combinations", () => {
    expect(parseHotkeyString("Space")).toBeNull();
    expect(parseHotkeyString("Ctrl+F9")).toBeNull();
    expect(parseHotkeyString("Ctrl+Alt+F9")).toBeNull();
  });
});

describe("keyEventToHotkey", () => {
  it("normalizes representative keydown events", () => {
    expect(keyEventToHotkey(createKeyboardEvent({ key: "f9" }))).toBe("F9");
    expect(
      keyEventToHotkey(
        createKeyboardEvent({
          altKey: true,
          ctrlKey: true,
          key: "Control",
        })
      )
    ).toBe("Ctrl+Option");
    expect(
      keyEventToHotkey(
        createKeyboardEvent({
          code: "Space",
          key: " ",
          metaKey: true,
        })
      )
    ).toBe("Cmd+Space");
    expect(keyEventToHotkey(createKeyboardEvent({ key: "Escape" }))).toBeNull();
  });
});

describe("hotkey matching", () => {
  it("matches press and release events for modifier-plus-space hotkeys", () => {
    const config = parseHotkeyString("Cmd+Space");
    if (!config) {
      throw new Error("Expected Cmd+Space to parse");
    }

    expect(
      matchesHotkey(
        createKeyboardEvent({
          code: "Space",
          key: " ",
          metaKey: true,
        }),
        config
      )
    ).toBe(true);
    expect(
      matchesHotkeyRelease(
        createKeyboardEvent({
          code: "KeyA",
          key: "a",
          metaKey: false,
        }),
        config
      )
    ).toBe(true);
  });

  it("handles modifier-pair release detection on keydown", () => {
    const config = parseHotkeyString("Ctrl+Option");
    if (!config) {
      throw new Error("Expected Ctrl+Option to parse");
    }

    expect(
      matchesHotkey(
        createKeyboardEvent({
          altKey: true,
          ctrlKey: true,
          key: "Alt",
        }),
        config
      )
    ).toBe(true);
    expect(
      shouldReleaseOnKeydown(
        createKeyboardEvent({
          altKey: true,
          ctrlKey: true,
          key: "Control",
        }),
        config
      )
    ).toBe(false);
    expect(
      shouldReleaseOnKeydown(
        createKeyboardEvent({
          altKey: true,
          ctrlKey: true,
          key: "a",
        }),
        config
      )
    ).toBe(true);
  });
});

describe("normalizeStoredHotkey", () => {
  it("preserves valid stored values and falls back for invalid ones", () => {
    expect(normalizeStoredHotkey("Cmd+Space")).toBe("Cmd+Space");
    expect(normalizeStoredHotkey("Fn")).toBe("Fn");
    expect(normalizeStoredHotkey("Ctrl+F9")).toBe("F9");
    expect(normalizeStoredHotkey(null)).toBe("F9");
  });
});
